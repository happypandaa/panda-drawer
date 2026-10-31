use std::path::{Component, Path};

/// The original lexical rule admits a volume root. This is not filesystem,
/// ownership or local-volume admission; those remain with the caller.
pub(super) fn invalid_absolute(source: &Path) -> bool {
    #[cfg(not(windows))]
    {
        !source.is_absolute()
            || source
                .components()
                .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    }
    #[cfg(windows)]
    {
        !source.is_absolute()
            || source.components().any(|part| {
                !matches!(
                    part,
                    Component::Prefix(_) | Component::RootDir | Component::Normal(_)
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn native_and_canonical_prefixes_preserve_the_original_root_and_traversal_policy() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-node-migration-path-")
            .tempdir()
            .unwrap();
        let canonical = temporary.path().canonicalize().unwrap();
        assert!(!invalid_absolute(&canonical));
        for value in [
            r"C:\",
            r"\\?\C:\",
            r"C:\中文 space\workspace",
            r"\\?\C:\中文 space\workspace",
        ] {
            assert!(!invalid_absolute(Path::new(value)), "{value}");
        }
        for value in [
            r"C:workspace",
            r"\workspace",
            r"workspace",
            r"C:\workspace\..\other",
        ] {
            assert!(invalid_absolute(Path::new(value)), "{value}");
        }
    }
    #[cfg(unix)]
    #[test]
    fn original_unix_root_is_still_an_explicit_absolute_path() {
        assert!(!invalid_absolute(Path::new("/")));
        assert!(!invalid_absolute(Path::new("/tmp/disposable migration")));
        assert!(invalid_absolute(Path::new("relative")));
        assert!(invalid_absolute(Path::new("/tmp/../elsewhere")));
    }
}
