//! Windows spelling for retained allocation claims, without filesystem access.
use std::path::{Component, Path};

pub(super) fn retained_directory_leaf(root: &Path) -> Option<String> {
    let text = root.to_str()?;
    if text.chars().any(char::is_control) {
        return None;
    }
    // Both Windows separators remain separators even in the extended namespace.
    // Inspect raw components before Rust can elide an original dot component.
    let text = text.replace('/', "\\");
    let normalized = Path::new(&text);
    if !normalized.is_absolute() {
        return None;
    }
    let mut components = normalized.components();
    let Component::Prefix(prefix) = components.next()? else {
        return None;
    };
    if components.next()? != Component::RootDir {
        return None;
    }
    let tail = text.get(prefix.as_os_str().len()..)?.strip_prefix('\\')?;
    if tail
        .split('\\')
        .any(|part| part.is_empty() || part == "." || part == "..")
        || normalized.parent()?.file_name()?.to_str()? != "projects"
    {
        return None;
    }
    normalized.file_name()?.to_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_and_extended_claims_accept_native_separators_without_opening_old_targets() {
        for root in [
            r"C:\disposable absent\projects\reserved-a",
            "D:/中文 space/projects/reserved-a",
            r"\\?\C:\disposable absent\projects\reserved-a",
            r"\\?\C:/中文 space/projects\reserved-a",
            r"\\server\share\projects\reserved-a",
            r"\\?\UNC\server\share\projects\reserved-a",
            r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\projects\reserved-a",
        ] {
            assert_eq!(
                retained_directory_leaf(Path::new(root)).as_deref(),
                Some("reserved-a"),
                "{root}"
            );
        }
    }

    #[test]
    fn ambiguous_or_traversing_claims_do_not_become_allocation_evidence() {
        for root in [
            r"C:projects\reserved-a",
            r"\projects\reserved-a",
            "/projects/reserved-a",
            r"C:\old\..\projects\reserved-a",
            r"C:\old\.\projects\reserved-a",
            r"\\?\C:\old/../projects\reserved-a",
            r"C:\old\\projects\reserved-a",
            r"C:\old\projects\reserved-a\",
            r"C:\old\other\reserved-a",
            "C:\\bad\0\\projects\\reserved-a",
        ] {
            assert_eq!(retained_directory_leaf(Path::new(root)), None, "{root}");
        }
    }
}
