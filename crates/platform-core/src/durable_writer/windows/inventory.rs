//! Physical inventory followed by complete, locked validation before retirement.
//! Same-boot published writers and material without boot proof stay unknown.
use super::{
    io::{self, blocked, Directory, Result},
    lexical_path, schema, Kind, WriterOwner,
};
use platform_native::windows::{
    boot_session,
    files::{self as native, FileIdentity},
};
use std::{
    ffi::OsString,
    fs::{self, File},
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
};

const MAX_WRITERS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Observation {
    Missing,
    Empty,
    Validated,
}

struct RetainedMarker {
    directory: usize,
    path: PathBuf,
    identity: FileIdentity,
    bytes: Vec<u8>,
    // Kept through the entire inventory and deletion. Its sharing contract
    // excludes live read/write preparation handles and subsequent writers.
    file: File,
}

impl RetainedMarker {
    fn verify(&self) -> Result<()> {
        let (_file, bytes, identity) = io::locked_marker(&self.path)?;
        if !self.identity.same_object(&identity) || self.bytes != bytes {
            return Err(blocked(
                &self.path,
                "Writer identity or bytes changed during inventory",
            ));
        }
        Ok(())
    }

    fn retire(self) -> Result<()> {
        self.verify()?;
        native::remove_file(&self.path, &self.identity)
            .map_err(|error| blocked(&self.path, error))?;
        drop(self.file);
        // SetFileInformationByHandle success is not an absence receipt. Another
        // reader can leave the name delete-pending; preserve that error until
        // a later real observation proves absence.
        io::missing(&self.path)
    }
}

#[derive(Default)]
struct Inventory {
    bound: Vec<Directory>,
    absent: Vec<PathBuf>,
    empty: Vec<(usize, u64)>,
    private: Vec<usize>,
    listings: Vec<(usize, Vec<OsString>, u64)>,
    writer_listings: Vec<(usize, Vec<OsString>, u64)>,
    markers: Vec<RetainedMarker>,
    boot_id: Option<u32>,
}

impl Inventory {
    fn walk(
        &mut self,
        path: &Path,
        observe: &mut impl FnMut(Observation, &Path) -> Result<()>,
    ) -> Result<Option<usize>> {
        // Resolve actual native components, including genuine absence of the
        // data root itself. Never turn an arbitrary I/O error into Missing.
        let mut last = None;
        for component in platform_native::filesystem::path_components(&lexical_path(path)) {
            match fs::symlink_metadata(&component) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if last.is_none() {
                        return Err(blocked(&component, error));
                    }
                    observe(Observation::Missing, &component)?;
                    io::missing(&component)?;
                    self.absent.push(component);
                    return Ok(None);
                }
                Err(error) => return Err(blocked(&component, error)),
                Ok(_) => (),
            }
            let selected = Directory::open(&component)?;
            self.bound.push(selected);
            last = Some(self.bound.len() - 1);
        }
        last.ok_or_else(super::invalid_owner).map(Some)
    }

    fn require_private(&mut self, index: usize) -> Result<()> {
        self.bound[index].verify(true)?;
        self.private.push(index);
        Ok(())
    }

    fn boot(
        &mut self,
        provider: &mut impl FnMut() -> std::io::Result<u32>,
        path: &Path,
    ) -> Result<u32> {
        let observed = provider().map_err(|error| blocked(path, error))?;
        if self.boot_id.is_some_and(|previous| previous != observed) {
            return Err(blocked(path, "Boot observation changed during inventory"));
        }
        self.boot_id = Some(observed);
        Ok(observed)
    }

    fn owner(
        &mut self,
        owner: &WriterOwner,
        provider: &mut impl FnMut() -> std::io::Result<u32>,
        observe: &mut impl FnMut(Observation, &Path) -> Result<()>,
    ) -> Result<()> {
        let path = io::owner_path(owner).join("writers");
        let Some(index) = self.walk(&path, observe)? else {
            return Ok(());
        };
        // Mac checks only the existing selected writers directory here. A
        // genuinely absent leaf adds no owner-record or parent-ACL admission.
        self.require_private(index)?;
        let directory = &self.bound[index];
        let path = directory.path.clone();
        let modified = modified(directory)?;
        let before = writer_names(&path)?;
        if before.is_empty() {
            observe(Observation::Empty, &path)?;
            self.empty.push((index, modified));
            return Ok(());
        }
        for name in &before {
            let path = path.join(name);
            let (file, bytes, identity) = io::locked_marker(&path)?;
            if !schema::preparing_name(&path) {
                let boot = self.boot(provider, &path)?;
                schema::validate_published(owner, &path, &file, &bytes, boot)?;
            }
            // Only a genuine preparation UUID bypasses JSON/boot parsing.
            // The private, ordinary, bounded, single-link file and locked
            // handle checks apply equally to preparations and publications.
            self.markers.push(RetainedMarker {
                directory: index,
                path,
                identity,
                bytes,
                file,
            });
        }
        self.writer_listings.push((index, before, modified));
        observe(Observation::Validated, &path)?;
        Ok(())
    }

    fn finish(&self) -> Result<()> {
        for path in &self.absent {
            io::missing(path)?;
        }
        for (index, before) in &self.empty {
            let directory = &self.bound[*index];
            require_empty(&directory.path)?;
            if modified(directory)? != *before {
                return Err(blocked(
                    &directory.path,
                    "Writer directory changed during inventory",
                ));
            }
        }
        for (index, before, time) in self.listings.iter().chain(&self.writer_listings) {
            let directory = &self.bound[*index];
            if names(&directory.path)? != *before || modified(directory)? != *time {
                return Err(blocked(
                    &directory.path,
                    "Owner directory list changed during inventory",
                ));
            }
        }
        self.verify_directories()?;
        for marker in &self.markers {
            marker.verify()?;
        }
        Ok(())
    }

    fn verify_directories(&self) -> Result<()> {
        for directory in &self.bound {
            directory.verify(false)?;
        }
        for index in &self.private {
            self.bound[*index].verify(true)?;
        }
        Ok(())
    }

    fn recover(mut self, provider: &mut impl FnMut() -> std::io::Result<u32>) -> Result<()> {
        // All owners and every marker must pass before the first mutation.
        self.finish()?;
        if let Some(expected) = self.boot_id {
            let path = &self.markers[0].path;
            if provider().map_err(|error| blocked(path, error))? != expected {
                return Err(blocked(path, "Boot observation changed before retirement"));
            }
        }
        for marker in self.markers.drain(..) {
            self.bound[marker.directory].verify(true)?;
            marker.retire()?;
        }
        // Leave the selected owner and writers directories intact. No recursive
        // deletion or owner record is introduced by recovery.
        for (index, _, _) in &self.writer_listings {
            require_empty(&self.bound[*index].path)?;
        }
        self.writer_listings.clear();
        self.finish()
    }
}

fn modified(directory: &Directory) -> Result<u64> {
    directory
        .pin
        .file
        .metadata()
        .map(|metadata| metadata.last_write_time())
        .map_err(|error| blocked(&directory.path, error))
}

fn require_empty(path: &Path) -> Result<()> {
    match fs::read_dir(path)
        .map_err(|error| blocked(path, error))?
        .next()
    {
        None => Ok(()),
        Some(Ok(_)) => Err(blocked(path, "Retained writer material remains present")),
        Some(Err(error)) => Err(blocked(path, error)),
    }
}

fn collect_names(path: &Path, limit: Option<usize>) -> Result<Vec<OsString>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(path).map_err(|error| blocked(path, error))? {
        if limit.is_some_and(|limit| names.len() >= limit) {
            return Err(blocked(path, "Writer inventory exceeds its marker budget"));
        }
        names.push(entry.map_err(|error| blocked(path, error))?.file_name());
    }
    names.sort();
    Ok(names)
}

fn names(path: &Path) -> Result<Vec<OsString>> {
    collect_names(path, None)
}

fn writer_names(path: &Path) -> Result<Vec<OsString>> {
    collect_names(path, Some(MAX_WRITERS))
}

fn owner_with_boot(
    owner: &WriterOwner,
    provider: &mut impl FnMut() -> std::io::Result<u32>,
    mut observe: impl FnMut(Observation, &Path) -> Result<()>,
) -> Result<()> {
    let mut inventory = Inventory::default();
    inventory.owner(owner, provider, &mut observe)?;
    inventory.recover(provider)
}

fn owner_with(
    owner: &WriterOwner,
    observe: impl FnMut(Observation, &Path) -> Result<()>,
) -> Result<()> {
    owner_with_boot(owner, &mut boot_session::current_boot_id, observe)
}

pub(super) fn require_owner_clear(owner: &WriterOwner) -> Result<()> {
    owner_with(owner, |_, _| Ok(()))
}

fn all_with_boot(
    data: &Path,
    provider: &mut impl FnMut() -> std::io::Result<u32>,
    mut observe: impl FnMut(Observation, &Path) -> Result<()>,
) -> Result<()> {
    let mut inventory = Inventory::default();
    inventory.owner(&WriterOwner::platform(data)?, provider, &mut observe)?;
    for (name, kind) in [
        ("node-project-ownership", Kind::Node),
        ("web-project-ownership", Kind::Web),
    ] {
        let path = lexical_path(data).join(name);
        let Some(index) = inventory.walk(&path, &mut observe)? else {
            continue;
        };
        inventory.require_private(index)?;
        let directory = &inventory.bound[index];
        let path = directory.path.clone();
        let before = names(&path)?;
        let modified = modified(directory)?;
        for name in &before {
            let id = name
                .to_str()
                .ok_or_else(|| blocked(&path, "Owner ID is not UTF-8"))?;
            if id == ".pending-deletions" {
                continue;
            }
            let owner = WriterOwner::new(data, kind, id)?;
            inventory.owner(&owner, provider, &mut observe)?;
        }
        inventory.listings.push((index, before, modified));
    }
    inventory.recover(provider)
}

fn all_with(data: &Path, observe: impl FnMut(Observation, &Path) -> Result<()>) -> Result<()> {
    all_with_boot(data, &mut boot_session::current_boot_id, observe)
}

pub(super) fn require_all_clear(data: &Path) -> Result<()> {
    all_with(data, |_, _| Ok(()))
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod recovery_tests;
#[cfg(test)]
#[path = "inventory_tests.rs"]
mod tests;
