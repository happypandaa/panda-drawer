use super::*;
use std::fs;

// Only the PE headers are read; these fixtures never run.
fn pe_fixture(directory: &Path, name: &str, subsystem: u16) -> PathBuf {
    fs::create_dir_all(directory).unwrap();
    let path = directory.join(name);
    let pe = 128usize;
    let mut image = vec![0u8; pe + 24 + 240];
    image[..2].copy_from_slice(b"MZ");
    image[60..64].copy_from_slice(&(pe as u32).to_le_bytes());
    image[pe..pe + 4].copy_from_slice(b"PE\0\0");
    image[pe + 20..pe + 22].copy_from_slice(&240u16.to_le_bytes());
    image[pe + 22..pe + 24].copy_from_slice(&2u16.to_le_bytes());
    image[pe + 24..pe + 26].copy_from_slice(&0x20bu16.to_le_bytes());
    image[pe + 92..pe + 94].copy_from_slice(&subsystem.to_le_bytes());
    fs::write(&path, image).unwrap();
    path
}

#[test]
fn path_only_gui_opens_folders_even_when_a_cli_shadows_it() {
    let temp = tempfile::tempdir().unwrap();
    let shims = temp.path().join("shims");
    pe_fixture(&shims, "code.exe", 3); // standalone console CLI
    fs::write(shims.join("code.cmd"), b"not launched").unwrap();
    let portable = temp.path().join("My portable editor");
    let gui = pe_fixture(&portable, "Code.exe", 2);
    let vscode = TOOLS.iter().find(|spec| spec.id == "vscode").unwrap();
    let cli = locate_spec(vscode, &[], std::slice::from_ref(&shims), false, None);
    assert!(cli.info.installed);
    assert_eq!(cli.application, None);
    assert_eq!(cli.info.capability, "manual");

    let tool = locate_spec(vscode, &[], &[shims, portable.clone()], false, None);
    assert_eq!(tool.application, Some(gui.clone()));
    assert_eq!(tool.info.capability, "folder");

    let bin = portable.join("bin");
    fs::create_dir(&bin).unwrap();
    fs::write(bin.join("code.cmd"), b"not launched").unwrap();
    assert_eq!(
        path_command(&[bin], "code", "Code.exe"),
        Some(PathCommand::Gui(gui))
    );
}

#[test]
fn directory_only_qoder_ide_uses_clipboard_until_it_owns_a_chat_protocol() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let ide = pe_fixture(&root.join("Qoder IDE"), "Qoder IDE.exe", 2);
    let desktop = pe_fixture(&root.join("Qoder"), "Qoder.exe", 2);
    let roots = std::slice::from_ref(&root);
    for other_owner in [None, Some(desktop)] {
        let tool = locate_qoder(roots, |scheme| {
            (scheme == "qoder").then(|| other_owner.clone()).flatten()
        });
        assert_eq!(tool.application.as_ref(), Some(&ide));
        assert_eq!(tool.info.capability, "folder");
        assert_eq!(tool.chat_scheme, None);
    }
    let tool = locate_qoder(roots, |scheme| (scheme == "qoder").then(|| ide.clone()));
    assert_eq!(tool.info.capability, "folder-prefill");
    assert_eq!(tool.chat_scheme, Some("qoder"));
}

#[test]
fn cursor_chat_stays_with_the_selected_installation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let installed = pe_fixture(&root.join("Cursor"), "Cursor.exe", 2);
    let other = pe_fixture(&root.join("other"), "Cursor.exe", 2);
    let cursor = TOOLS.iter().find(|spec| spec.id == "cursor").unwrap();
    for owner in [None, Some(other.clone()), Some(installed.clone())] {
        let owns_link = owner.as_ref() == Some(&installed);
        let tool = locate_spec(
            cursor,
            std::slice::from_ref(&root),
            &[],
            owner.is_some(),
            owner,
        );
        assert!(same_file_object(
            tool.application.as_deref().unwrap(),
            &installed
        ));
        assert_eq!(tool.chat_scheme, owns_link.then_some("cursor"));
        assert_eq!(
            tool.info.capability,
            if owns_link {
                "folder-prefill"
            } else {
                "folder"
            }
        );
    }
    // A previously matched protocol can switch while the folder window opens.
    for scheme in ["cursor", "qoder", "qoder-cn"] {
        assert!(!dispatch_to_selected_program(
            scheme,
            Some(&installed),
            |_| Some(other.clone())
        ));
        assert!(dispatch_to_selected_program(
            scheme,
            Some(&installed),
            |_| Some(installed.clone())
        ));
    }
    assert!(dispatch_to_selected_program("codex", None, |_| panic!(
        "MSIX full handoff does not require a program path"
    )));

    // Some installations share binary bytes via hard links but have their
    // own runtime resources. A link owner in that directory is still distinct.
    let linked_dir = root.join("linked-install");
    fs::create_dir(&linked_dir).unwrap();
    let linked = linked_dir.join("Cursor.exe");
    fs::hard_link(&installed, &linked).unwrap();
    assert!(!handler_matches(Some(&installed), Some(&linked)));
}
