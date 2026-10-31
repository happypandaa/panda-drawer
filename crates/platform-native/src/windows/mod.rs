//! Windows x64 primitives. Business code calls the platform facade, never Win32.
pub mod files;
pub mod process;
pub mod removal;
pub mod volume;
pub mod workspace;

use std::{
    ffi::{OsStr, OsString},
    io,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Component, Path, PathBuf},
};

pub(crate) fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut result: Vec<_> = value.encode_wide().collect();
    if result.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in native argument",
        ));
    }
    result.push(0);
    Ok(result)
}

pub(crate) fn separator_path(path: &Path) -> PathBuf {
    let units: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .map(|unit| {
            if unit == u16::from(b'/') {
                u16::from(b'\\')
            } else {
                unit
            }
        })
        .collect();
    PathBuf::from(OsString::from_wide(&units))
}

pub(crate) fn normalized_path(path: &Path) -> io::Result<PathBuf> {
    // Normalize before parsing ancestors as well as before adding the extended
    // namespace prefix. In a verbatim Path, '/' otherwise hides intermediates.
    let path = separator_path(path);
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected an absolute path without traversal",
        ));
    }
    Ok(path
        .components()
        .filter(|part| !matches!(part, Component::CurDir))
        .collect())
}

pub(crate) fn native_path(path: &Path) -> io::Result<Vec<u16>> {
    let path = normalized_path(path)?;
    // Extended paths work without LongPathsEnabled and preserve Unicode. Avoid
    // converting an OsStr through UTF-8, which would corrupt unpaired surrogates.
    let raw: Vec<u16> = path.as_os_str().encode_wide().collect();
    let extended: Vec<u16> = if raw.starts_with(&[92, 92, 63, 92]) {
        raw
    } else if raw.starts_with(&[92, 92]) {
        "\\\\?\\UNC\\"
            .encode_utf16()
            .chain(raw.into_iter().skip(2))
            .collect()
    } else {
        "\\\\?\\".encode_utf16().chain(raw).collect()
    };
    if extended.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in native path",
        ));
    }
    Ok(extended.into_iter().chain([0]).collect())
}

pub(crate) fn checked(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write};

    #[test]
    fn mixed_separator_paths_reach_the_same_actual_private_object() {
        let temp = tempfile::Builder::new()
            .prefix("panda-disposable-native-path-中文 space-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mixed_path = |relative: &str| {
            let mut path = root.as_os_str().to_os_string();
            path.push("\\");
            path.push(relative);
            PathBuf::from(path)
        };
        let nested = root.join("nested");
        files::create_private_directory(&nested).unwrap();
        let path = mixed_path("nested/source.txt");
        assert!(path
            .as_os_str()
            .encode_wide()
            .any(|unit| unit == u16::from(b'/')));
        let mut file = files::create_private_file(&path).unwrap();
        file.write_all(b"actual mixed path").unwrap();
        file.sync_all().unwrap();
        let identity = files::identity(&file).unwrap();
        drop(file);
        assert!(identity.same_object(
            &files::open_no_reparse(&nested.join("source.txt"))
                .unwrap()
                .identity
        ));
        let destination = mixed_path("nested/renamed.txt");
        files::rename_file(&path, &destination, false).unwrap();
        assert_eq!(
            fs::read(nested.join("renamed.txt")).unwrap(),
            b"actual mixed path"
        );
        assert!(identity.same_object(&files::open_no_reparse(&destination).unwrap().identity));
        assert!(native_path(&mixed_path("nested/../not-admitted.txt")).is_err());
        assert!(!root.join("not-admitted.txt").exists());
    }

    #[test]
    fn normalization_preserves_native_utf16_and_checks_parent_components_first() {
        let mut units: Vec<u16> = "C:/中文 space/".encode_utf16().collect();
        units.push(0xd800);
        let path = PathBuf::from(OsString::from_wide(&units));
        let native = native_path(&path).unwrap();
        assert!(native.contains(&0xd800));
        assert!(!native.contains(&u16::from(b'/')));
        for value in [
            r"C:\base/../file",
            r"\\?\C:\base/../file",
            r"C:relative",
            r"\rooted",
        ] {
            assert!(native_path(Path::new(value)).is_err(), "{value}");
        }
        assert_eq!(
            native_path(Path::new(r"C:\base\.\file")).unwrap(),
            native_path(Path::new(r"C:\base\file")).unwrap()
        );
    }
}
