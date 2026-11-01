//! Author files for a new web project. No local instance identity is written.
use crate::{
    panda_manifest::{AppDecl, PandaManifest},
    ApiError,
};
use serde_json::json;
use std::{fs, io::Write, path::Path};

pub(crate) fn install(
    template_root: &Path,
    project_root: &Path,
    app: &AppDecl,
) -> Result<PandaManifest, ApiError> {
    copy_source(&template_root.join("app"), project_root)?;
    let manifest=PandaManifest::parse(&serde_json::to_vec(&json!({
        "manifestVersion":1,"runtime":"web","app":app,
        "web":{"root":"dist","entry":"index.html"},
        "platform":{"sdkApiVersion":1},
        "development":{"sourceDir":".","build":{"kind":"npm","script":"build"}},
        "documentation":{"readme":"README.md","development":"docs/development.md","database":"docs/database.md"}
    }))?).map_err(|error|ApiError::new("INVALID_TEMPLATE",error.to_string()))?;
    write_new(
        &project_root.join("panda.json"),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    write_new(&project_root.join(".gitignore"),b"node_modules/\ndist/\n# To include file data in Git, remove the data/ line below.\ndata/\n")?;
    if !project_root.join("README.md").try_exists()? {
        write_new(&project_root.join("README.md"),format!("# {}\n\n{}\n\nThis folder is the running application. Read panda.json and docs/development.md before changing it.\n",app.name,app.description.as_deref().unwrap_or("")).as_bytes())?;
    }
    crate::project_file_io::ensure_directory(&project_root.join("docs"))?;
    write_new(&project_root.join("docs/development.md"),b"# Development\n\nThe application source is in src/, and Vite builds dist/. Read panda.json for the actual build script. Close the application window before building; run npm ci then npm run build, or use the platform Build action. The next open reads this same folder. The platform does not retain versions or roll back a failed build. Use Git to save and revert code.\n\nLoad the platform SDK as a normal script from /_platform/sdk.js before application modules, then use window.panda. Types are in src/panda.d.ts. SQL/files use /_platform/api/v1/ and need platform.sdkApiVersion:1. Public typed configuration comes from /_platform/config.json or window.panda.config(), including when the data SDK is disabled. Do not parse the project ID from the URL or vendor the SDK.\n\nFile data belongs in data/ and is excluded from Git by default. Ask the user before deleting it. Browser data lives in the platform window storage and is always cleared when the application is removed. While the application is open, change SQLite only through the platform CLI. After it is fully closed, direct database work is allowed.\n")?;
    write_new(&project_root.join("docs/database.md"),b"# Database\n\nThe optional platform SDK creates data/main.sqlite and data/attachments/ on first use. No business tables are required. Document each table, column, type, unit, enum, relation and attachment lifecycle here when adding it. The platform does not back up or migrate file data.\n")?;
    Ok(manifest)
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), ApiError> {
    let mut file = platform_native::filesystem::create_new_private_file(path, false)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn copy_source(source: &Path, target: &Path) -> Result<(), ApiError> {
    crate::projects::check_no_symlinks(source, false)?;
    crate::project_file_io::ensure_directory(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if [
            "node_modules",
            "dist",
            ".git",
            ".hg",
            ".svn",
            ".cache",
            ".vite",
        ]
        .iter()
        .any(|ignored| name == *ignored)
        {
            continue;
        }
        let destination = target.join(name);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_source(&entry.path(), &destination)?;
        } else if kind.is_file() {
            write_new(&destination, &fs::read(entry.path())?)?;
        } else {
            return Err(ApiError::new(
                "INVALID_TEMPLATE",
                "模板不能包含链接或特殊文件。",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installs_author_files_at_the_root_without_instance_or_control_material() {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let template = base.join("template");
        let source = template.join("app");
        fs::create_dir_all(source.join("src")).unwrap();
        fs::write(source.join("index.html"), "<main>source</main>").unwrap();
        fs::write(source.join("src/main.js"), "console.log('source')").unwrap();
        fs::write(
            source.join("package.json"),
            r#"{"scripts":{"build":"vite build"}}"#,
        )
        .unwrap();
        fs::write(
            source.join("package-lock.json"),
            r#"{"lockfileVersion":3,"packages":{"":{}}}"#,
        )
        .unwrap();
        let project = base.join("project");
        fs::create_dir(&project).unwrap();
        let manifest = install(
            &template,
            &project,
            &AppDecl {
                name: "Disposable root template".into(),
                description: None,
                version: None,
                icon: None,
            },
        )
        .unwrap();
        assert!(project.join("index.html").is_file());
        assert!(project.join("src/main.js").is_file());
        assert!(!project.join("app").exists());
        assert!(!project.join("data").exists());
        assert!(!project.join(".platform").exists());
        assert!(!project.join(".agents").exists());
        assert!(!project.join("AGENTS.md").exists());
        assert_eq!(manifest.web.root, "dist");
        assert_eq!(manifest.web.entry, "index.html");
        assert_eq!(manifest.platform.as_ref().unwrap().sdk_api_version, 1);
        assert_eq!(manifest.inspect(&project).unwrap()["needsBuild"], true);
        let declaration: serde_json::Value =
            serde_json::from_slice(&fs::read(project.join("panda.json")).unwrap()).unwrap();
        assert!(declaration["app"].get("id").is_none());
        let ignore = fs::read_to_string(project.join(".gitignore")).unwrap();
        assert!(ignore.contains("node_modules/"));
        assert!(ignore.contains("data/"));
        assert!(ignore.contains("remove the data/ line"));
    }
    #[test]
    fn refuses_to_overwrite_an_existing_author_file() {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let template = base.join("template");
        fs::create_dir_all(template.join("app")).unwrap();
        fs::write(template.join("app/index.html"), "template").unwrap();
        let project = base.join("project");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("index.html"), "user source").unwrap();
        assert!(install(
            &template,
            &project,
            &AppDecl {
                name: "Disposable".into(),
                description: None,
                version: None,
                icon: None
            }
        )
        .is_err());
        assert_eq!(
            fs::read_to_string(project.join("index.html")).unwrap(),
            "user source"
        );
    }
}
