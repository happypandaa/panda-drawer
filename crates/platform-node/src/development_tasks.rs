//! Append-only task evidence. Interrupted work is never replayed on startup.
use crate::ApiError;
use platform_host::{WindowsObjectIdentity, WorkspaceDirectory};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{watch, Notify};
use uuid::Uuid;
type Result<T> = std::result::Result<T, ApiError>;
fn error(message: impl Into<String>) -> ApiError {
    ApiError::new("DEVELOPMENT_TASK_INVALID", message)
}
fn fs(e: platform_host::WorkspaceError) -> ApiError {
    ApiError::new(e.code, e.message)
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Proof {
    schema_version: u32,
    task_id: String,
    directory_identity: WindowsObjectIdentity,
}
struct State {
    record: Value,
    directory: WorkspaceDirectory,
    event: u32,
    log: u32,
    cancel: watch::Sender<bool>,
}
pub struct DevelopmentTasks {
    data: WorkspaceDirectory,
    states: Mutex<BTreeMap<String, State>>,
    stopping: AtomicBool,
    drained: Notify,
}
pub struct TaskContext {
    id: String,
    tasks: Arc<DevelopmentTasks>,
    directory: WorkspaceDirectory,
}
fn final_record(record: &Value) -> bool {
    matches!(
        record["status"].as_str(),
        Some("succeeded" | "failed" | "interrupted")
    )
}
fn valid_id(id: &str) -> bool {
    id.len() == 37 && id.starts_with("task_") && id[5..].bytes().all(|b| b.is_ascii_hexdigit())
}
fn source_id(id: &str) -> bool {
    id.len() == 36 && id.starts_with("src_") && id[4..].bytes().all(|b| b.is_ascii_hexdigit())
}

impl DevelopmentTasks {
    pub fn new(data: WorkspaceDirectory) -> Result<Arc<Self>> {
        let tasks = Arc::new(Self {
            data,
            states: Mutex::new(BTreeMap::new()),
            stopping: AtomicBool::new(false),
            drained: Notify::new(),
        });
        if let Some(root) =
            platform_host::windows_development::area(&tasks.data, "tasks", false).map_err(fs)?
        {
            for item in root.list().map_err(fs)? {
                if item.name == ".windows-development.json" {
                    continue;
                }
                if !item.is_directory || !valid_id(&item.name) {
                    return Err(error("任务根含未知记录。"));
                }
                let directory = root.open_directory(&item.name).map_err(fs)?;
                let proof: Proof = serde_json::from_slice(
                    &directory
                        .read_bounded(".windows-task.json", 65536)
                        .map_err(fs)?
                        .bytes,
                )?;
                if proof.schema_version != 1
                    || proof.task_id != item.name
                    || proof.directory_identity != *directory.identity()
                {
                    return Err(error("任务目录身份变化，未接管。"));
                }
                let entries = directory.list().map_err(fs)?;
                let mut events: Vec<_> = entries
                    .iter()
                    .filter(|e| e.name.starts_with("event-"))
                    .map(|e| e.name.clone())
                    .collect();
                events.sort();
                for (index, name) in events.iter().enumerate() {
                    if *name != format!("event-{:08}.json", index + 1) {
                        return Err(error("任务事件缺失或格式未知。"));
                    }
                }
                let last = events
                    .last()
                    .ok_or_else(|| error("任务创建未完成，现场已保留。"))?;
                let mut record: Value = serde_json::from_slice(
                    &directory.read_bounded(last, 8_000_000).map_err(fs)?.bytes,
                )?;
                if record["taskId"] != item.name
                    || !matches!(
                        record["status"].as_str(),
                        Some("queued" | "running" | "succeeded" | "failed" | "interrupted")
                    )
                {
                    return Err(error("任务记录身份或状态无效。"));
                }
                let log = entries
                    .iter()
                    .filter(|e| e.name.starts_with("log-"))
                    .count() as u32;
                let (cancel, _) = watch::channel(false);
                let mut state = State {
                    record: record.clone(),
                    directory,
                    event: events.len() as u32,
                    log,
                    cancel,
                };
                if !final_record(&record) {
                    record["status"] = json!("interrupted");
                    record["final"] = json!(true);
                    record["error"] = json!(ApiError::new(
                        "TASK_INTERRUPTED",
                        "平台退出前任务未结束，现场已保留；没有自动重试。"
                    )
                    .outcome("unknown"));
                    Self::save(&mut state, record)?;
                }
                tasks.states.lock().unwrap().insert(item.name, state);
            }
        }
        Ok(tasks)
    }
    fn save(state: &mut State, record: Value) -> Result<()> {
        let next = state.event + 1;
        state
            .directory
            .write_new(
                &format!("event-{next:08}.json"),
                &serde_json::to_vec(&record)?,
            )
            .map_err(|e| fs(e).outcome("unknown"))?;
        state.event = next;
        state.record = record;
        Ok(())
    }
    pub fn start(self: &Arc<Self>, kind: &str, source_id: Option<&str>) -> Result<TaskContext> {
        let mut states = self.states.lock().unwrap();
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "平台正在退出，未接受新任务。",
            ));
        }
        if !matches!(kind, "sourceCapture" | "sourceAcquire" | "sourceBuild") {
            return Err(error("不支持的开发任务。"));
        }
        if source_id.is_some_and(|id| !self::source_id(id)) {
            return Err(error("来源标识无效。"));
        }
        let root = platform_host::windows_development::area(&self.data, "tasks", true)
            .map_err(fs)?
            .unwrap();
        if root.list().map_err(fs)?.len() >= 240 {
            return Err(error("当前内部版本的任务记录数量已达上限，旧记录已保留。"));
        }
        let id = format!("task_{}", Uuid::new_v4().simple());
        let directory = root.create_directory(&id).map_err(fs)?;
        directory
            .write_new(
                ".windows-task.json",
                &serde_json::to_vec(&Proof {
                    schema_version: 1,
                    task_id: id.clone(),
                    directory_identity: directory.identity().clone(),
                })?,
            )
            .map_err(|e| fs(e).outcome("unknown"))?;
        let (cancel, _) = watch::channel(false);
        let record = json!({"taskId":id,"projectId":null,"sourceId":source_id,"type":kind,"status":"queued","final":false,"phase":"queued","result":null,"error":null});
        let mut state = State {
            record: Value::Null,
            directory: directory.clone(),
            event: 0,
            log: 0,
            cancel,
        };
        Self::save(&mut state, record)?;
        states.insert(id.clone(), state);
        Ok(TaskContext {
            id,
            tasks: self.clone(),
            directory,
        })
    }
    pub fn get(&self, id: &str) -> Result<Value> {
        self.states
            .lock()
            .unwrap()
            .get(id)
            .map(|s| s.record.clone())
            .ok_or_else(|| error("未找到该开发任务。"))
    }
    pub fn list(&self) -> Value {
        json!(self
            .states
            .lock()
            .unwrap()
            .values()
            .map(|s| s.record.clone())
            .collect::<Vec<_>>())
    }
    pub fn cancel(&self, id: &str) -> Result<Value> {
        let mut states = self.states.lock().unwrap();
        let state = states.get_mut(id).ok_or_else(|| error("未找到该任务。"))?;
        if !final_record(&state.record) && state.record["cancelRequested"] != true {
            state.cancel.send_replace(true);
            let mut record = state.record.clone();
            record["cancelRequested"] = json!(true);
            Self::save(state, record)?;
        }
        Ok(state.record.clone())
    }
    pub fn logs(&self, id: &str, cursor: u64) -> Result<Value> {
        let states = self.states.lock().unwrap();
        let state = states.get(id).ok_or_else(|| error("未找到该任务。"))?;
        let mut bytes = Vec::new();
        for index in 1..=state.log {
            let chunk = state
                .directory
                .read_bounded(&format!("log-{index:08}.txt"), 200_000)
                .map_err(fs)?
                .bytes;
            if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                return Err(error("任务日志超出读取预算。"));
            }
            bytes.extend(chunk);
        }
        let start = (cursor as usize).min(bytes.len());
        let end = (start + 64 * 1024).min(bytes.len());
        Ok(
            json!({"taskId":id,"text":String::from_utf8_lossy(&bytes[start..end]),"nextCursor":end,"eof":end==bytes.len(),"truncated":false,"gaps":[]}),
        )
    }
    pub async fn shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        {
            let states = self.states.lock().unwrap();
            for state in states.values() {
                if !final_record(&state.record) {
                    state.cancel.send_replace(true);
                }
            }
        }
        loop {
            let notified = self.drained.notified();
            if self
                .states
                .lock()
                .unwrap()
                .values()
                .all(|s| final_record(&s.record))
            {
                break;
            }
            notified.await;
        }
    }
}
impl TaskContext {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn directory(&self) -> &WorkspaceDirectory {
        &self.directory
    }
    pub fn cancellation(&self) -> watch::Receiver<bool> {
        self.tasks.states.lock().unwrap()[&self.id]
            .cancel
            .subscribe()
    }
    pub fn check(&self) -> Result<()> {
        if *self.tasks.states.lock().unwrap()[&self.id].cancel.borrow() {
            Err(ApiError::new("TASK_CANCELLED", "任务已取消，现场已保留。")
                .outcome("not_committed"))
        } else {
            self.directory.verify_binding().map_err(fs)
        }
    }
    pub fn phase(&self, phase: &str) -> Result<()> {
        self.check()?;
        let mut states = self.tasks.states.lock().unwrap();
        let state = states.get_mut(&self.id).unwrap();
        if final_record(&state.record) {
            return Err(error("任务已结束。"));
        }
        let mut record = state.record.clone();
        record["phase"] = json!(phase);
        record["status"] = json!("running");
        DevelopmentTasks::save(state, record)
    }
    pub fn log(&self, stream: &str, bytes: &[u8], truncated: bool) -> Result<()> {
        let mut states = self.tasks.states.lock().unwrap();
        let state = states.get_mut(&self.id).unwrap();
        let text = format!(
            "[{stream}]\n{}{}\n",
            String::from_utf8_lossy(&bytes[..bytes.len().min(120_000)]),
            if truncated || bytes.len() > 120_000 {
                "\n[日志片段已截断]"
            } else {
                ""
            }
        );
        let next = state.log + 1;
        state
            .directory
            .write_new(&format!("log-{next:08}.txt"), text.as_bytes())
            .map_err(fs)?;
        state.log = next;
        Ok(())
    }
    pub fn finish(&self, result: Result<Value>) -> Result<Value> {
        let mut states = self.tasks.states.lock().unwrap();
        let state = states.get_mut(&self.id).unwrap();
        if final_record(&state.record) {
            return Ok(state.record.clone());
        }
        let mut record = state.record.clone();
        record["final"] = json!(true);
        match result {
            Ok(value) => {
                record["status"] = json!("succeeded");
                record["phase"] = json!("completed");
                record["result"] = value;
            }
            Err(issue) => {
                record["status"] = json!("failed");
                record["error"] = json!(issue);
            }
        }
        let saved = DevelopmentTasks::save(state, record.clone());
        if let Err(issue) = saved {
            record["status"] = json!("interrupted");
            record["error"] = json!(ApiError::new(
                "TASK_PERSISTENCE_UNKNOWN",
                "任务结果持久化未确认，现场已保留，请重新读取。"
            )
            .detail("persistence", json!(issue))
            .outcome("unknown"));
            state.record = record.clone();
        }
        self.tasks.drained.notify_waiters();
        Ok(record)
    }
}
impl Drop for TaskContext {
    fn drop(&mut self) {
        let _ = self.finish(Err(ApiError::new(
            "TASK_INTERRUPTED",
            "任务执行被中断，保留现场，没有自动重试。",
        )
        .outcome("unknown")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> WorkspaceDirectory {
        let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(".test-artifacts");
        let path = parent.join(format!(
            "windows-node-disposable-development-tasks-{}",
            Uuid::new_v4()
        ));
        let workspace = platform_host::disposable_node_workspace(&path).unwrap();
        let root = workspace.root();
        root.create_directory("workspace")
            .unwrap()
            .create_directory(".platform")
            .unwrap();
        root
    }
    #[test]
    fn observing_tasks_does_not_create_the_task_area() {
        let root = root();
        let tasks = DevelopmentTasks::new(root.clone()).unwrap();
        assert_eq!(tasks.list(), json!([]));
        assert!(
            platform_host::windows_development::area(&root, "tasks", false)
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn cancellation_waits_for_runner_and_does_not_replay_or_duplicate_writes() {
        let root = root();
        let tasks = DevelopmentTasks::new(root.clone()).unwrap();
        let ctx = tasks.start("sourceCapture", None).unwrap();
        let id = ctx.id().to_owned();
        ctx.phase("source-copy").unwrap();
        ctx.log("stdout", "人工日志中文".as_bytes(), false).unwrap();
        assert_eq!(tasks.get(&id).unwrap()["final"], false);
        let cancel = tasks.cancel(&id).unwrap();
        assert_eq!(cancel["final"], false);
        let entries = ctx.directory().list().unwrap().len();
        tasks.cancel(&id).unwrap();
        assert_eq!(entries, ctx.directory().list().unwrap().len());
        assert!(ctx.check().is_err());
        let finished = ctx
            .finish(Err(ApiError::new("TASK_CANCELLED", "已取消")))
            .unwrap();
        assert_eq!(finished["final"], true);
        assert_eq!(finished["status"], "failed");
        assert!(tasks.logs(&id, 0).unwrap()["text"]
            .as_str()
            .unwrap()
            .contains("人工日志中文"));
        drop(ctx);
        let reopened = DevelopmentTasks::new(root).unwrap();
        assert_eq!(reopened.get(&id).unwrap()["status"], "failed");
        reopened.shutdown().await;
        assert!(reopened.start("sourceCapture", None).is_err());
    }
    #[test]
    fn dropped_runner_is_recorded_as_interrupted_and_never_succeeded() {
        let tasks = DevelopmentTasks::new(root()).unwrap();
        let ctx = tasks.start("sourceAcquire", None).unwrap();
        let id = ctx.id().to_owned();
        drop(ctx);
        let record = tasks.get(&id).unwrap();
        assert_eq!(record["status"], "failed");
        assert_eq!(record["error"]["code"], "TASK_INTERRUPTED");
        assert_eq!(record["final"], true);
    }
    #[test]
    fn startup_classifies_a_persisted_pending_record_without_executing_it() {
        let data = root();
        let task_root = platform_host::windows_development::area(&data, "tasks", true)
            .unwrap()
            .unwrap();
        let id = format!("task_{}", Uuid::new_v4().simple());
        let directory = task_root.create_directory(&id).unwrap();
        directory
            .write_new(
                ".windows-task.json",
                &serde_json::to_vec(&Proof {
                    schema_version: 1,
                    task_id: id.clone(),
                    directory_identity: directory.identity().clone(),
                })
                .unwrap(),
            )
            .unwrap();
        directory.write_new("event-00000001.json",&serde_json::to_vec(&json!({"taskId":id,"projectId":null,"type":"sourceAcquire","status":"running","phase":"git-fetch","final":false,"result":null,"error":null})).unwrap()).unwrap();
        directory
            .write_new("external-evidence.txt", b"retained interruption evidence")
            .unwrap();
        let tasks = DevelopmentTasks::new(data).unwrap();
        assert_eq!(tasks.get(&id).unwrap()["status"], "interrupted");
        assert_eq!(
            directory
                .read_bounded("external-evidence.txt", 1024)
                .unwrap()
                .bytes,
            b"retained interruption evidence"
        );
        assert_eq!(
            directory
                .list()
                .unwrap()
                .iter()
                .filter(|e| e.name.starts_with("event-"))
                .count(),
            2
        );
    }
}
