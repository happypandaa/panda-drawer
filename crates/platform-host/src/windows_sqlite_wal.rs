//! Conservative admission for an existing WAL, not SQLite crash recovery.
//! A stale or uncommitted tail is retained and rejected until a later policy
//! can distinguish recoverable history from a database that may be adopted.
use platform_project_format::ApiError;
use std::io::{ErrorKind, Read};

const WAL_MAGIC: u32 = 0x377f_0682;
const WAL_VERSION: u32 = 3_007_000;
const HEADER_BYTES: usize = 32;
const FRAME_HEADER_BYTES: usize = 24;

fn invalid(reason: &str) -> ApiError {
    ApiError::new("DATABASE_WAL_INVALID", "Existing SQLite WAL is invalid")
        .detail("reason", reason)
        .outcome("not_started")
}

fn recovery(reason: &str) -> ApiError {
    ApiError::new(
        "DATABASE_WAL_RECOVERY_REQUIRED",
        "Existing SQLite WAL needs an explicit recovery policy; it was retained",
    )
    .detail("reason", reason)
    .outcome("not_started")
}

fn word(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// SQLite walChecksumBytes: checksum input word order follows the magic LSB;
/// stored header and frame checksum words themselves are always big endian.
fn checksum(bytes: &[u8], big_endian: bool, state: &mut [u32; 2]) {
    debug_assert!(!bytes.is_empty() && bytes.len().is_multiple_of(8));
    for pair in bytes.as_chunks::<8>().0 {
        let a = [pair[0], pair[1], pair[2], pair[3]];
        let b = [pair[4], pair[5], pair[6], pair[7]];
        let (a, b) = if big_endian {
            (u32::from_be_bytes(a), u32::from_be_bytes(b))
        } else {
            (u32::from_le_bytes(a), u32::from_le_bytes(b))
        };
        state[0] = state[0].wrapping_add(a).wrapping_add(state[1]);
        state[1] = state[1].wrapping_add(b).wrapping_add(state[0]);
    }
}

/// Distinguishes an exact EOF from a partial structure without seeking or
/// buffering the file. Interrupted reads do not turn a valid WAL into an error.
fn read_structure(reader: &mut impl Read, target: &mut [u8]) -> Result<bool, ApiError> {
    let mut filled = 0;
    while filled < target.len() {
        match reader.read(&mut target[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err(invalid("truncated_structure")),
            Ok(count) => filled += count,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(invalid("read_failed").detail("ioError", error.to_string()));
            }
        }
    }
    Ok(true)
}

/// Validates exactly the supplied stream using at most one SQLite page of
/// scratch memory. The caller must separately bind the file identity and
/// prevent concurrent mutation; reading is not a filesystem snapshot.
pub(crate) fn validate_wal(reader: &mut impl Read, page_size: u32) -> Result<(), ApiError> {
    if !(512..=65_536).contains(&page_size) || !page_size.is_power_of_two() {
        return Err(invalid("invalid_database_page_size"));
    }
    let mut header = [0u8; HEADER_BYTES];
    if !read_structure(reader, &mut header)? {
        return Ok(());
    }
    let magic = word(&header[0..4]);
    if magic & !1 != WAL_MAGIC {
        return Err(invalid("header_magic"));
    }
    if word(&header[4..8]) != WAL_VERSION {
        return Err(invalid("header_version"));
    }
    if word(&header[8..12]) != page_size {
        return Err(invalid("header_page_size"));
    }
    let big_endian = magic & 1 != 0;
    let mut state = [0u32; 2];
    checksum(&header[..24], big_endian, &mut state);
    if state != [word(&header[24..28]), word(&header[28..32])] {
        return Err(invalid("header_checksum"));
    }

    let mut page = vec![0u8; page_size as usize];
    let mut frame = [0u8; FRAME_HEADER_BYTES];
    let mut last_was_commit = false;
    loop {
        if !read_structure(reader, &mut frame)? {
            return if last_was_commit {
                Ok(())
            } else {
                Err(recovery("no_final_commit_frame"))
            };
        }
        if frame[8..16] != header[16..24] {
            return Err(recovery("frame_salt_changed"));
        }
        if word(&frame[..4]) == 0 {
            return Err(invalid("frame_page_number_zero"));
        }
        if !read_structure(reader, &mut page)? {
            return Err(invalid("missing_frame_page"));
        }
        // walDecodeFrame checks the first eight frame-header bytes, excluding
        // the salt, then the page, continuing the preceding frame checksum.
        checksum(&frame[..8], big_endian, &mut state);
        checksum(&page, big_endian, &mut state);
        if state != [word(&frame[16..20]), word(&frame[20..24])] {
            return Err(invalid("frame_checksum"));
        }
        last_was_commit = word(&frame[4..8]) != 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};

    // A separate word-pair reference also builds endian-specific fixtures.
    fn reference_checksum(bytes: &[u8], big_endian: bool, mut state: [u32; 2]) -> [u32; 2] {
        let words: Vec<_> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| {
                let bytes: [u8; 4] = *bytes;
                if big_endian {
                    u32::from_be_bytes(bytes)
                } else {
                    u32::from_le_bytes(bytes)
                }
            })
            .collect();
        for index in (0..words.len()).step_by(2) {
            let first = state[0].wrapping_add(words[index]).wrapping_add(state[1]);
            let second = state[1].wrapping_add(words[index + 1]).wrapping_add(first);
            state = [first, second];
        }
        state
    }

    fn put(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn fixture(big_endian: bool, page_size: u32, frames: &[(u32, u32)]) -> Vec<u8> {
        let mut wal = vec![0u8; HEADER_BYTES];
        put(&mut wal, 0, WAL_MAGIC | u32::from(big_endian));
        put(&mut wal, 4, WAL_VERSION);
        put(&mut wal, 8, page_size);
        put(&mut wal, 12, 7);
        put(&mut wal, 16, 0x1020_3040);
        put(&mut wal, 20, 0xa1b2_c3d4);
        let mut state = reference_checksum(&wal[..24], big_endian, [0; 2]);
        put(&mut wal, 24, state[0]);
        put(&mut wal, 28, state[1]);
        for &(page_number, commit_size) in frames {
            let mut frame = vec![0u8; FRAME_HEADER_BYTES + page_size as usize];
            put(&mut frame, 0, page_number);
            put(&mut frame, 4, commit_size);
            frame[8..16].copy_from_slice(&wal[16..24]);
            for (index, byte) in frame[FRAME_HEADER_BYTES..].iter_mut().enumerate() {
                *byte = (index as u8).wrapping_mul(13).wrapping_add(29);
            }
            state = reference_checksum(&frame[..8], big_endian, state);
            state = reference_checksum(&frame[FRAME_HEADER_BYTES..], big_endian, state);
            put(&mut frame, 16, state[0]);
            put(&mut frame, 20, state[1]);
            wal.extend(frame);
        }
        wal
    }

    fn assert_error(bytes: &[u8], page_size: u32, code: &str) -> ApiError {
        let error = validate_wal(&mut Cursor::new(bytes), page_size).unwrap_err();
        assert_eq!(error.code, code);
        assert_eq!(error.details["operationOutcome"], "not_started");
        error
    }

    #[test]
    fn checksum_matches_fixed_endian_vectors_and_wraps() {
        let bytes: Vec<_> = (1u32..=6).flat_map(u32::to_le_bytes).collect();
        let mut little = [0; 2];
        checksum(&bytes, false, &mut little);
        assert_eq!(little, [26, 46]);
        let mut big = [0; 2];
        checksum(&bytes, true, &mut big);
        assert_eq!(big, [0x1a00_0000, 0x2e00_0000]);
        let mut wrapping = [u32::MAX, u32::MAX];
        checksum(&[0xff; 8], false, &mut wrapping);
        assert_eq!(wrapping, [u32::MAX - 2, u32::MAX - 4]);
    }

    #[test]
    fn empty_wal_and_committed_frames_accept_both_byte_orders_and_page_extremes() {
        validate_wal(&mut Cursor::new([]), 512).unwrap();
        for endian in [false, true] {
            for page_size in [512, 65_536] {
                let bytes = fixture(endian, page_size, &[(1, 0), (2, 2), (1, 2)]);
                let original = bytes.clone();
                let mut reader = Cursor::new(bytes);
                validate_wal(&mut reader, page_size).unwrap();
                assert_eq!(reader.into_inner(), original);
            }
        }
    }

    #[test]
    fn header_fields_checksum_and_short_header_are_rejected() {
        let valid = fixture(false, 512, &[(1, 1)]);
        for length in 1..HEADER_BYTES {
            assert_error(&valid[..length], 512, "DATABASE_WAL_INVALID");
        }
        for (offset, value) in [(0, 0), (4, WAL_VERSION + 1), (8, 0), (8, 1024)] {
            let mut bytes = valid.clone();
            put(&mut bytes, offset, value);
            assert_error(&bytes, 512, "DATABASE_WAL_INVALID");
        }
        for offset in [12, 16, 24, 28] {
            let mut bytes = valid.clone();
            bytes[offset] ^= 1;
            assert_error(&bytes, 512, "DATABASE_WAL_INVALID");
        }
        for page_size in [0, 1, 256, 513, 131_072] {
            assert_error(&valid, page_size, "DATABASE_WAL_INVALID");
        }
    }

    #[test]
    fn page_number_frame_header_data_and_checksum_corruption_are_rejected() {
        for endian in [false, true] {
            let zero_page = fixture(endian, 512, &[(0, 1)]);
            assert_error(&zero_page, 512, "DATABASE_WAL_INVALID");
            let valid = fixture(endian, 512, &[(1, 1)]);
            for offset in [HEADER_BYTES, HEADER_BYTES + 4, HEADER_BYTES + 16, 100] {
                let mut bytes = valid.clone();
                bytes[offset] ^= 1;
                assert_error(&bytes, 512, "DATABASE_WAL_INVALID");
            }
            let mut wrong_endian = valid;
            wrong_endian[3] ^= 1;
            assert_error(&wrong_endian, 512, "DATABASE_WAL_INVALID");
        }
    }

    #[test]
    fn partial_frames_and_trailing_bytes_are_rejected() {
        let valid = fixture(false, 512, &[(1, 1), (2, 2)]);
        let second = HEADER_BYTES + FRAME_HEADER_BYTES + 512;
        for length in second + 1..valid.len() {
            assert_error(&valid[..length], 512, "DATABASE_WAL_INVALID");
        }
        let mut bytes = fixture(false, 512, &[(1, 1)]);
        bytes.push(0);
        assert_error(&bytes, 512, "DATABASE_WAL_INVALID");
    }

    #[test]
    fn salt_changes_and_uncommitted_tails_require_explicit_recovery() {
        for frames in [&[][..], &[(1, 0)][..], &[(1, 1), (2, 0)][..]] {
            let bytes = fixture(false, 512, frames);
            assert_error(&bytes, 512, "DATABASE_WAL_RECOVERY_REQUIRED");
        }
        let mut bytes = fixture(false, 512, &[(1, 1), (2, 2)]);
        let second = HEADER_BYTES + FRAME_HEADER_BYTES + 512;
        bytes[second + 8] ^= 1;
        let error = assert_error(&bytes, 512, "DATABASE_WAL_RECOVERY_REQUIRED");
        assert_eq!(error.details["reason"], "frame_salt_changed");
    }

    struct InterruptedOnce {
        interrupted: bool,
        bytes: Cursor<Vec<u8>>,
    }
    impl Read for InterruptedOnce {
        fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                Err(io::Error::from(ErrorKind::Interrupted))
            } else {
                let limit = target.len().min(3);
                self.bytes.read(&mut target[..limit])
            }
        }
    }

    #[test]
    fn short_reads_and_interrupted_reads_are_supported() {
        let mut reader = InterruptedOnce {
            interrupted: false,
            bytes: Cursor::new(fixture(true, 512, &[(1, 1)])),
        };
        validate_wal(&mut reader, 512).unwrap();
    }

    #[test]
    fn io_failure_is_not_treated_as_empty_or_success() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::from(ErrorKind::PermissionDenied))
            }
        }
        let error = validate_wal(&mut Broken, 512).unwrap_err();
        assert_eq!(error.code, "DATABASE_WAL_INVALID");
        assert_eq!(error.details["reason"], "read_failed");
        assert_eq!(error.details["operationOutcome"], "not_started");
    }
}
