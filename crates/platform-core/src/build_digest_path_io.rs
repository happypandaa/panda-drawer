//! Keep the existing portable tree-digest labels independent of host separators.
use std::{borrow::Cow, path::Path};

pub(super) fn label(path: &Path) -> Cow<'_, str> {
    #[cfg(not(windows))]
    {
        // Preserve Unix filename bytes, including a literal backslash inside a
        // filename. collect_files has already required valid UTF-8 components.
        Cow::Borrowed(path.to_str().unwrap())
    }
    #[cfg(windows)]
    {
        Cow::Owned(
            path.iter()
                .map(|name| name.to_str().unwrap())
                .collect::<Vec<_>>()
                .join("/"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn ordinary_relative_components_use_the_packaging_digest_spelling() {
        let path = Path::new("assets").join("图😀.js");
        assert_eq!(label(&path), "assets/图😀.js");
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_backslash_filename_keeps_its_original_digest_bytes() {
        assert_eq!(
            label(Path::new(r"assets/name\literal.js")),
            r"assets/name\literal.js"
        );
    }

    #[test]
    fn actual_unicode_tree_matches_the_node_packaging_digest_vector() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-build-digest-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        fs::create_dir(root.join("assets")).unwrap();
        fs::write(root.join("assets/main.css"), b"body{}").unwrap();
        fs::write(root.join("assets/图😀.js"), b"console.log(1)").unwrap();
        fs::write(root.join("z.txt"), b"disposable").unwrap();
        // Independent Node crypto vector: depth-first UTF-8 names, '/', then
        // little-endian byte lengths and actual contents, as package-template.
        assert_eq!(
            super::super::tree_digest(&root, false).unwrap(),
            "e37ecb14393aa81b754c73b5ddb84b30efae27452b021724ab0dfe518cfae36c"
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Read-only opt-in against a newly prepared Windows template resource"]
    fn real_prepared_template_source_and_starter_match_the_packaged_records() {
        let root = std::path::PathBuf::from(
            std::env::var_os("PANDA_PREPARED_WINDOWS_TEMPLATE")
                .expect("explicit prepared template path"),
        );
        let record: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("starter.json")).unwrap()).unwrap();
        for (child, key) in [("app", "sourceDigest"), ("starter", "artifactDigest")] {
            assert_eq!(
                super::super::tree_digest(&root.join(child), false).unwrap(),
                record[key].as_str().unwrap(),
                "{child}"
            );
        }
    }
}
