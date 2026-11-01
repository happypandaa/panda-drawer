//! Every delivery and destructive recovery here uses an isolated TempDir.
use super::*;
use crate::{
    builds::BuildManager, maintenance::MaintenanceManager,
    web_project_deletion::WebProjectDeletionManager, CreateProjectInput, ProjectStore,
    RuntimePaths, TaskStatus,
};
use serde_json::json;
use std::fs;
use std::{path::PathBuf, sync::Arc};

struct Fixture {
    _temp: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    lifecycle: Arc<WebProjectDeletionManager>,
    project: ProjectHandle,
    source: PathBuf,
    destination: PathBuf,
    other: PathBuf,
}
impl Fixture {
    async fn new() -> Self {
        let (temp, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        store.set_node_source_data_root(paths.data_root().into());
        let info = store
            .create(CreateProjectInput {
                name: "Disposable native delivery A".into(),
                description: None,
            })
            .await
            .unwrap();
        let project = store.project(&info.id).unwrap();
        let tasks = BuildManager::new(paths.clone(), store.clone()).unwrap();
        let maintenance = MaintenanceManager::new(store.clone(), tasks.clone());
        let lifecycle = WebProjectDeletionManager::new(
            paths.clone(),
            store.clone(),
            tasks.clone(),
            maintenance,
        )
        .unwrap();
        let external = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("external-deliveries");
        fs::create_dir(&external).unwrap();
        let source = external.join("independent-original.bin");
        fs::write(&source, b"disposable download bytes").unwrap();
        let destination = external.join("delivered.bin");
        let other = external.join("independent-B.bin");
        fs::write(&other, b"independent B original").unwrap();
        Self {
            _temp: temp,
            paths,
            store,
            tasks,
            lifecycle,
            project,
            source,
            destination,
            other,
        }
    }
    fn ownership(&self) -> WebProjectOwnership {
        WebProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        )
    }
    fn copy(&self, hook: impl FnMut(Point, &Path) -> Result<()>) -> Result<()> {
        publish(
            self.paths.data_root(),
            self.paths.workspace_root(),
            &self.project,
            File::open(&self.source).unwrap(),
            &self.destination,
            |_| Ok(()),
            hook,
        )
    }
    fn stages(&self) -> Vec<PathBuf> {
        self.ownership()
            .material_paths(&self.project.id)
            .unwrap()
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(".export-task_") && name.ends_with(".partial")
                    })
            })
            .collect()
    }
    async fn delete(&self) {
        let id = &self.project.id;
        let task = self
            .lifecycle
            .submit_project(
                id,
                "delete",
                json!({"expectedDocumentEpoch":self.project.document_epoch}),
            )
            .await
            .unwrap();
        let final_task = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let task = self
                    .lifecycle
                    .task_for_project(&task.task_id, id, "delete")
                    .unwrap();
                if task.status.is_final() {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(final_task.status, TaskStatus::Succeeded, "{final_task:?}");
        assert_eq!(fs::read(&self.other).unwrap(), b"independent B original");
        assert_eq!(
            fs::read(&self.source).unwrap(),
            b"disposable download bytes"
        );
    }
    async fn stop(self) {
        self.lifecycle.shutdown().await;
        self.tasks.shutdown().await;
        self.store.shutdown().await;
    }
}

#[test]
fn destination_identity_keeps_external_hardlinks_eligible() {
    let temporary = tempfile::tempdir().unwrap();
    let file = temporary.path().join("用户 原件.txt");
    let other = temporary.path().join("other.txt");
    std::fs::write(&file, b"external original").unwrap();
    std::fs::hard_link(&file, &other).unwrap();
    let expected = destination_identity(&file).unwrap().unwrap();
    assert!(expected.matches_captured(&destination_identity(&other).unwrap().unwrap()));
    assert_eq!(std::fs::read(other).unwrap(), b"external original");
}

#[tokio::test]
async fn actual_native_copy_and_approved_overwrite_survive_public_project_deletion() {
    let fixture = Fixture::new().await;
    fixture.copy(|_, _| Ok(())).unwrap();
    assert_eq!(
        fs::read(&fixture.destination).unwrap(),
        b"disposable download bytes"
    );
    fs::write(&fixture.destination, b"user selected previous output").unwrap();
    let alias = fixture
        .destination
        .with_file_name("old hardlink preserved.bin");
    fs::hard_link(&fixture.destination, &alias).unwrap();
    fixture.copy(|_, _| Ok(())).unwrap();
    assert_eq!(fs::read(&alias).unwrap(), b"user selected previous output");
    assert!(fixture.stages().is_empty());
    assert!(fixture
        .ownership()
        .task_ids(&fixture.project.id)
        .unwrap()
        .is_empty());
    fixture.delete().await;
    assert_eq!(
        fs::read(&fixture.destination).unwrap(),
        b"disposable download bytes"
    );
    assert_eq!(fs::read(alias).unwrap(), b"user selected previous output");
    fixture.stop().await;
}

#[tokio::test]
async fn prepublication_interrupt_reopens_and_deletes_only_the_registered_partial() {
    for point in [Point::Bound, Point::Copied, Point::BeforePublish] {
        let fixture = Fixture::new().await;
        fs::write(&fixture.destination, b"preserved selected original").unwrap();
        let mut stage = None;
        assert!(fixture
            .copy(|observed, path| {
                if observed == point {
                    stage = Some(path.to_path_buf());
                    Err(invalid("disposable interrupted copy"))
                } else {
                    Ok(())
                }
            })
            .is_err());
        let stage = stage.unwrap();
        assert!(fixture.stages().contains(&stage));
        assert_eq!(
            fs::read(&fixture.destination).unwrap(),
            b"preserved selected original"
        );
        fixture.delete().await;
        assert!(!stage.exists());
        assert_eq!(
            fs::read(&fixture.destination).unwrap(),
            b"preserved selected original"
        );
        fixture.stop().await;
    }
}

#[tokio::test]
async fn target_replacement_during_copy_is_preserved_without_publishing() {
    let fixture = Fixture::new().await;
    fs::write(&fixture.destination, b"selected original").unwrap();
    let original = fixture.destination.with_file_name("original kept.bin");
    assert!(fixture
        .copy(|point, _| {
            if point == Point::Copied {
                fs::rename(&fixture.destination, &original)?;
                fs::write(&fixture.destination, b"foreign later file")?;
            }
            Ok(())
        })
        .is_err());
    fixture.delete().await;
    assert_eq!(fs::read(&original).unwrap(), b"selected original");
    assert_eq!(
        fs::read(&fixture.destination).unwrap(),
        b"foreign later file"
    );
    fixture.stop().await;
}

#[tokio::test]
async fn rejected_destination_never_starts_or_adopts_external_material() {
    let fixture = Fixture::new().await;
    fs::write(&fixture.destination, b"external saved original").unwrap();
    let expected = PersistedIdentity::capture(&fixture.destination).unwrap();
    assert!(publish_file(
        fixture.paths.data_root(),
        fixture.paths.workspace_root(),
        &fixture.project,
        File::open(&fixture.source).unwrap(),
        &fixture.destination,
        |_| Err(invalid("protected selected path"))
    )
    .is_err());
    assert!(expected.matches_path(&fixture.destination).unwrap());
    assert_eq!(
        fs::read(&fixture.destination).unwrap(),
        b"external saved original"
    );
    assert!(fixture
        .ownership()
        .task_ids(&fixture.project.id)
        .unwrap()
        .is_empty());
    fixture.delete().await;
    assert_eq!(
        fs::read(&fixture.destination).unwrap(),
        b"external saved original"
    );
    fixture.stop().await;
}

#[tokio::test]
async fn real_chinese_space_and_long_destination_uses_exact_native_paths() {
    let fixture = Fixture::new().await;
    let mut parent = fixture.destination.parent().unwrap().to_path_buf();
    for _ in 0..5 {
        parent = parent.join("人工 中文 长文件夹 含空格 disposable");
        fs::create_dir(&parent).unwrap();
    }
    let destination = parent.join("人工 下载 完成.txt");
    assert!(destination.as_os_str().len() > 260);
    publish_file(
        fixture.paths.data_root(),
        fixture.paths.workspace_root(),
        &fixture.project,
        File::open(&fixture.source).unwrap(),
        &destination,
        |_| Ok(()),
    )
    .unwrap();
    fixture.delete().await;
    assert_eq!(fs::read(destination).unwrap(), b"disposable download bytes");
    fixture.stop().await;
}
