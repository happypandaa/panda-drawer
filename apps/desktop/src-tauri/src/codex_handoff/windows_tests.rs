use super::*;
use windows::Win32::{
    Foundation::{LocalFree, HLOCAL},
    UI::Shell::CommandLineToArgvW,
};

fn decoded(root: &Path) -> Vec<String> {
    let args = arguments(root).unwrap();
    let mut command = "app.exe ".encode_utf16().collect::<Vec<_>>();
    command.extend(args);
    let mut count = 0;
    let raw = unsafe { CommandLineToArgvW(PCWSTR(command.as_ptr()), &mut count) };
    assert!(!raw.is_null());
    let values = unsafe { std::slice::from_raw_parts(raw, count as usize) }
        .iter()
        .map(|argument| unsafe { argument.to_string().unwrap() })
        .collect();
    let _ = unsafe { LocalFree(Some(HLOCAL(raw.cast()))) };
    values
}
#[test]
fn one_native_project_argument_roundtrips_chinese_spaces_quotes_shell_text_and_trailing_backslashes(
) {
    for value in [
        r"C:\人工 项目\#?&prompt=hello'$(echo test)",
        r"C:\project\",
        r"C:\project\\",
        r#"C:\project\quote"text"#,
    ] {
        assert_eq!(
            decoded(Path::new(value)),
            vec![
                "app.exe".to_owned(),
                "--open-project".to_owned(),
                value.to_owned()
            ]
        );
    }
    let long = format!("C:\\{}\\人工 项目", "folder\\".repeat(60));
    assert_eq!(decoded(Path::new(&long))[2], long);
    assert!(arguments(Path::new("relative")).is_err());
    assert!(arguments(Path::new("C:\\project\0truncated")).is_err());
}
#[test]
fn package_name_or_publisher_alone_never_authorizes_an_editor() {
    assert!(package_authorized(NAME, FAMILY, PUBLISHER, true, true));
    for (name, family, publisher, store, healthy) in [
        ("Codex", FAMILY, PUBLISHER, true, true),
        (NAME, "other-family", PUBLISHER, true, true),
        (NAME, FAMILY, "other-publisher", true, true),
        (NAME, FAMILY, PUBLISHER, false, true),
        (NAME, FAMILY, PUBLISHER, true, false),
    ] {
        assert!(!package_authorized(name, family, publisher, store, healthy));
    }
}
#[tokio::test]
async fn familiar_disposable_folder_cannot_launch_or_receive_editor_fallback_material() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let candidate = root.join("Codex");
    std::fs::create_dir(&candidate).unwrap();
    let sentinel = root.join("source.txt");
    std::fs::write(&sentinel, b"external author original").unwrap();
    let error = open(&candidate, &root).await.unwrap_err();
    assert!(matches!(
        error.code.as_str(),
        "CODEX_INSTALLATION_INVALID" | "CODEX_NOT_INSTALLED"
    ));
    assert_eq!(
        std::fs::read(sentinel).unwrap(),
        b"external author original"
    );
    assert_eq!(std::fs::read_dir(root).unwrap().count(), 2);
}
#[tokio::test]
#[ignore = "Read-only opt-in requires the official current-user installed Codex Store package; never launches it"]
async fn actual_current_user_store_package_is_integrity_verified_without_launch() {
    let path = installed_app().await.unwrap();
    assert!(path.is_absolute());
    assert!(path.is_dir());
    assert!(path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("OpenAI.Codex_"));
}
