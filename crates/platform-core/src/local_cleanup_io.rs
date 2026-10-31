//! Match the lock provider's native contention result at the cleanup boundary.
use std::io;

pub(super) fn lock_is_contended(error: &io::Error) -> bool {
    #[cfg(unix)]
    {
        error.kind() == io::ErrorKind::WouldBlock
    }
    #[cfg(windows)]
    {
        // fs2 returns LockFileEx's ERROR_LOCK_VIOLATION unchanged. Rust maps
        // that native result to Uncategorized, rather than WouldBlock. Other
        // I/O failures do not prove that the original instance lock is held.
        matches!(
            (error.raw_os_error(), fs2::lock_contended_error().raw_os_error()),
            (Some(observed), Some(expected)) if observed == expected
        )
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn only_the_fs2_native_contention_error_proves_a_held_windows_lock() {
        assert_eq!(fs2::lock_contended_error().raw_os_error(), Some(33));
        assert!(lock_is_contended(&fs2::lock_contended_error()));
        for code in [2, 5, 6, 32, 87] {
            assert!(!lock_is_contended(&io::Error::from_raw_os_error(code)));
        }
        assert!(!lock_is_contended(&io::Error::new(
            io::ErrorKind::WouldBlock,
            "artificial non-native failure"
        )));
    }
}
