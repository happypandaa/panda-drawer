//! Paths passed to other applications or APIs that expect conventional Win32
//! paths. A file-operation verbatim prefix is removed only when Windows proves
//! that doing so leaves the complete UTF-16 path unchanged.
use super::wide;
use std::{
    io,
    os::windows::ffi::OsStringExt,
    path::{Component, Path, PathBuf, Prefix},
    ptr::null_mut,
};
use windows_sys::Win32::Storage::FileSystem::GetFullPathNameW;

fn resolves_exactly(value: &[u16]) -> bool {
    let Ok(length) = u32::try_from(value.len()) else {
        return false;
    };
    let mut resolved = vec![0; value.len()];
    let count =
        unsafe { GetFullPathNameW(value.as_ptr(), length, resolved.as_mut_ptr(), null_mut()) };
    count != 0 && count as usize == value.len() - 1 && resolved == value
}

/// Follow the Windows standard-library process-directory rule: convert only
/// standard verbatim drive/UNC prefixes, and only after an exact Win32 lexical
/// round trip. This does not inspect, authorize or create any filesystem object.
pub fn conventional_path(path: &Path) -> io::Result<PathBuf> {
    let original = wide(path.as_os_str())?;
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return Ok(path.to_owned());
    };
    let candidate = match prefix.kind() {
        Prefix::VerbatimDisk(_) => original[4..].to_vec(),
        Prefix::VerbatimUNC(_, _) => {
            let mut candidate = vec![b'\\' as u16; 2];
            candidate.extend_from_slice(&original[8..]);
            candidate
        }
        _ => return Ok(path.to_owned()),
    };
    let conventional = PathBuf::from(std::ffi::OsString::from_wide(
        &candidate[..candidate.len() - 1],
    ));
    if conventional.is_absolute() && resolves_exactly(&candidate) {
        Ok(conventional)
    } else {
        // Stripping a prefix can change a trailing-dot, reserved or other
        // special name. Keep the caller's exact namespace, as std does.
        Ok(path.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsString, os::windows::ffi::OsStrExt};

    #[test]
    fn ordinary_drive_and_unc_paths_strip_only_their_verbatim_prefix() {
        for (input, expected) in [
            (r"\\?\C:\project\中文 space", r"C:\project\中文 space"),
            (
                r"\\?\UNC\server\share\中文 space",
                r"\\server\share\中文 space",
            ),
            (r"C:\project\中文 space", r"C:\project\中文 space"),
            (r"\\server\share\中文 space", r"\\server\share\中文 space"),
        ] {
            assert_eq!(
                conventional_path(Path::new(input)).unwrap(),
                Path::new(expected)
            );
        }
    }

    #[test]
    fn lexical_conversion_preserves_unpaired_utf16_and_adds_no_length_limit() {
        let mut input = r"\\?\C:\".encode_utf16().collect::<Vec<_>>();
        input.extend(std::iter::repeat_n(b'x' as u16, 400));
        input.extend([b'\\' as u16, 0xd800]);
        let expected = input[4..].to_vec();
        let input = PathBuf::from(OsString::from_wide(&input));
        assert_eq!(
            conventional_path(&input)
                .unwrap()
                .as_os_str()
                .encode_wide()
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn names_that_win32_would_change_and_other_namespaces_keep_their_meaning() {
        for input in [
            r"\\?\C:\project\trailing.",
            r"\\?\C:\project\trailing ",
            r"\\?\C:\project\NUL",
            r"\\?\C:\project\..\elsewhere",
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\project",
            r"\\.\device",
            r"relative\path",
        ] {
            let path = Path::new(input);
            assert_eq!(conventional_path(path).unwrap(), path, "{input}");
        }
        let nul = PathBuf::from(OsString::from_wide(&[b'C' as u16, 0, b'x' as u16]));
        assert_eq!(
            conventional_path(&nul).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}
