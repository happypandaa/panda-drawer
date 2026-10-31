use super::*;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::windows::fs::OpenOptionsExt,
};

const MAX_JOURNAL_BYTES: u64 = 64 * 1024;
const SHARE_DELETE: u32 = 4;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;

pub(super) struct Journal {
    pub(super) transaction: Transaction,
    identity: FileIdentity,
    bytes: Vec<u8>,
    file: File,
}

fn read_bytes(mut file: &File) -> io::Result<Vec<u8>> {
    if file.metadata()?.len() > MAX_JOURNAL_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Exchange journal exceeds its metadata budget",
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(MAX_JOURNAL_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Exchange journal exceeds its metadata budget",
        ));
    }
    Ok(bytes)
}

impl Journal {
    pub(super) fn open(pair: &Pair) -> io::Result<Option<Self>> {
        let path = pair.journal();
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
            Ok(_) => (),
        }
        // Only this independently-created journal requires private single-link
        // storage. Artifact directories and their contents gain no new policy.
        // Deny data read/write sharing to serialize live transactions; allow
        // deletion so this exact journal can be retired through another handle.
        let file = OpenOptions::new()
            .read(true)
            .share_mode(SHARE_DELETE)
            .custom_flags(OPEN_REPARSE_POINT)
            .open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || platform_native::filesystem::is_reparse(&metadata) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Exchange journal must be an ordinary file",
            ));
        }
        let identity = native::identity(&file)?;
        if identity.links != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Exchange journal must be independently owned",
            ));
        }
        native::verify_private(&file)?;
        let bytes = read_bytes(&file)?;
        let transaction: Transaction = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        transaction.validate(pair)?;
        let journal = Self {
            transaction,
            identity,
            bytes,
            file,
        };
        journal.verify(pair)?;
        Ok(Some(journal))
    }

    pub(super) fn publish(pair: &Pair, transaction: Transaction) -> io::Result<Self> {
        transaction.validate(pair)?;
        let bytes = serde_json::to_vec(&transaction).map_err(io::Error::other)?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Exchange journal exceeds its metadata budget",
            ));
        }
        let preparation = pair.staged_parent.path.join(format!(
            ".artifact-directory-journal-{}.preparing",
            transaction.id
        ));
        let mut file = native::create_private_file_for_publication(&preparation)?;
        let identity = native::identity(&file)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        // Bind publication to the original DELETE-capable preparing handle.
        // Native success is already a publication; every later error remains
        // unknown and retains the fixed journal rather than undoing any move.
        native::rename_private_file_handle(&file, &pair.journal(), false)?;
        file.sync_all()?;
        drop(file);
        let journal = Self::open(pair)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Exchange journal disappeared after publication",
            )
        })?;
        if !identity.same_object(&journal.identity) || journal.bytes != bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Published exchange journal changed",
            ));
        }
        Ok(journal)
    }

    pub(super) fn verify(&self, pair: &Pair) -> io::Result<()> {
        self.transaction.validate(pair)?;
        let actual = native::open_no_reparse(&pair.journal())?;
        if actual.identity.directory
            || actual.identity.links != 1
            || !self.identity.same_object(&actual.identity)
            || read_bytes(&self.file)? != self.bytes
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Active exchange journal changed",
            ));
        }
        native::verify_private(&self.file)
    }

    pub(super) fn finish(self, pair: &Pair) -> io::Result<()> {
        self.verify(pair)?;
        if step(pair, &self.transaction)? != Step::Complete {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Exchange is not complete",
            ));
        }
        let path = pair.journal();
        native::remove_file(&path, &self.identity)?;
        drop(self);
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Exchange journal retirement is unconfirmed",
            )),
        }
    }
}
