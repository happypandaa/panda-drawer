use clap::{Args, Parser, Subcommand};
use platform_core::{
    platform_support::RuntimeSupportError, Discovery, RuntimePaths, API_VERSION, APP_IDENTIFIER,
    DISPLAY_NAME, ENVIRONMENT,
};
use reqwest::blocking::{Client, RequestBuilder};
use serde::Serialize;
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const MAX_RESPONSE_BYTES: u64 = 64 * 1024;
const MAX_INPUT_BYTES: u64 = 2_000_000;
const MAX_BUSINESS_RESPONSE_BYTES: u64 = 8_200_000;

#[derive(Parser)]
#[command(
    name = "local-projects-dev",
    version,
    about = "Manage projects in the isolated development platform",
    long_about = "Manage projects and SQLite in the isolated development platform. This CLI never starts the desktop app or connects to a production environment. Build projects using service-selected Node/npm, manage complete backups and maintenance, export/import/copy projects, restore or purge trash, and migrate the complete workspace. Inspect persistent tasks. Migration task reads reconnect after instance changes. Writes are never automatically retried."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Acquire public source without executing it, or inspect local declarations.
    Source {
        #[command(subcommand)]
        command: SourceCommand,
    },
    /// Inspect the development service without starting it.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Create and inspect local development projects.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Query or modify a project’s SQLite database.
    Db {
        #[command(subcommand)]
        command: DatabaseCommand,
    },
    /// Inspect persistent asynchronous project tasks.
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    /// Create, inspect, and restore complete project snapshots.
    Backup {
        #[command(subcommand)]
        command: BackupCommand,
    },
    /// Explicitly prepare and end persistent project maintenance.
    Maintenance {
        #[command(subcommand)]
        command: MaintenanceCommand,
    },
    /// Inspect and explicitly restore or permanently purge deleted projects.
    Trash {
        #[command(subcommand)]
        command: TrashCommand,
    },
    /// Inspect project files, measured storage, and explicit safe cleanup.
    Storage {
        #[command(subcommand)]
        command: StorageCommand,
    },
    /// Inspect or migrate the complete workspace.
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    /// Diagnose the service and its Node/npm and optional Git development tools.
    Env {
        #[command(subcommand)]
        command: EnvironmentCommand,
    },
}

#[derive(Subcommand)]
enum SourceCommand {
    List(OutputOptions),
    Get {
        #[arg(long)]
        source: String,
        #[command(flatten)]
        output: OutputOptions,
    },
    Inspect {
        #[arg(long)]
        path: String,
        #[command(flatten)]
        output: OutputOptions,
    },
    Acquire {
        #[arg(long)]
        url: String,
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        output: OutputOptions,
    },
    /// Request cancellation; inspect the returned task until it is final.
    Cancel(TaskOptions),
}

#[derive(Subcommand)]
enum ServiceCommand {
    /// Check discovery, service identity and authenticated runtime information.
    Status(OutputOptions),
}

#[derive(Subcommand)]
enum EnvironmentCommand {
    /// Report service identity and the service-selected Node/npm environment.
    Doctor(OutputOptions),
    /// Choose a Node executable for this development platform only.
    Node {
        #[command(subcommand)]
        command: NodeCommand,
    },
    /// Diagnose/select Git, or explicitly check public HTTPS repository access.
    Git {
        #[command(subcommand)]
        command: GitCommand,
    },
}

#[derive(Subcommand)]
enum GitCommand {
    /// Report Git and system SSH tools. Does not check repository authentication.
    Doctor(OutputOptions),
    /// Persist an explicit absolute Git executable path for this platform only.
    Set {
        #[arg(long)]
        path: String,
        #[command(flatten)]
        output: OutputOptions,
    },
    /// Clear the explicit path and select the bundled Git toolchain.
    Clear(OutputOptions),
    /// Read a public GitHub/Gitee repository HEAD over HTTPS, without credentials, clone or push.
    Check {
        #[arg(long)]
        url: String,
        #[command(flatten)]
        output: OutputOptions,
    },
}

#[derive(Subcommand)]
enum NodeCommand {
    /// Persist an explicit absolute Node executable path. Does not install Node.
    Set {
        #[arg(long)]
        path: String,
        #[command(flatten)]
        output: OutputOptions,
    },
    /// Clear the explicit path and restore the service's default toolchain.
    Clear(OutputOptions),
}

#[derive(Subcommand)]
enum TaskCommand {
    Get(TaskOptions),
    /// Wait up to 30 seconds. A timeout does not cancel or fail the task.
    Wait {
        #[command(flatten)]
        task: TaskOptions,
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(0..=30))]
        timeout: u64,
    },
    /// Read a bounded log chunk; pass nextCursor to read subsequent output.
    Logs {
        #[command(flatten)]
        task: TaskOptions,
        #[arg(long, default_value_t = 0)]
        cursor: u64,
    },
}

#[derive(Args)]
struct TaskOptions {
    #[arg(long)]
    task: String,
    #[command(flatten)]
    output: OutputOptions,
}

#[derive(Args)]
struct OutputOptions {
    /// Print a machine-readable result envelope.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum ProjectCommand {
    List(OutputOptions),
    Metadata {
        #[command(subcommand)]
        command: MetadataCommand,
    },
    Create(InputOptions),
    Info(ProjectOptions),
    /// Get a single-use browser launch URL, valid for 60 seconds.
    Url(ProjectOptions),
    /// Tell open pages to query their data again.
    Refresh(ProjectMutationOptions),
    /// Submit a build and return immediately with its persistent task ID.
    Build(ProjectMutationOptions),
    /// Read project-specific instructions to give an external AI editor.
    Handoff(ProjectOptions),
    /// Export a validated complete ZIP to destinationPath in JSON.
    Export(ProjectInputOptions),
    /// Import archivePath with mode original or copy; never overwrites a project.
    Import(InputOptions),
    /// Create an independent project copy, optionally choosing a name in JSON.
    Copy(ProjectMutationOptions),
    /// Move the complete project to trash. This is reversible.
    Delete(ProjectMutationOptions),
}

#[derive(Subcommand)]
enum TrashCommand {
    List(OutputOptions),
    Info(TrashOptions),
    Restore(TrashOptions),
    /// Permanently remove this trash entry's project files. Does not remove historical backups or exported ZIPs.
    Purge(TrashOptions),
}

#[derive(Args)]
struct TrashOptions {
    #[arg(long)]
    trash: String,
    #[command(flatten)]
    output: OutputOptions,
}

#[derive(Subcommand)]
enum WorkspaceCommand {
    Info(OutputOptions),
    Storage(OutputOptions),
    /// Preview an offline legacy identity repair; does not initialize or start the app.
    IdentityRepairPlan(OutputOptions),
    /// Confirm the exact offline preview. Preserves original migration records and project data.
    IdentityRepair(InputOptions),
    /// Inspect registered historical cleanup candidates without deleting files.
    CleanupPlan(OutputOptions),
    /// Permanently clean explicitly selected IDs from an unexpired preview plan.
    Cleanup(InputOptions),
    /// Migrate to destinationPath in JSON. Old root is retained; never resubmit on a timeout.
    Migrate(InputOptions),
}

#[derive(Subcommand)]
enum MetadataCommand {
    Get(ProjectOptions),
    Set(ProjectInputOptions),
}
#[derive(Subcommand)]
enum StorageCommand {
    Info(ProjectOptions),
    Scan(ProjectOptions),
    Files(ProjectInputOptions),
    Cleanup(ProjectInputOptions),
}

#[derive(Subcommand)]
enum BackupCommand {
    List(ProjectOptions),
    Info {
        #[command(flatten)]
        project: ProjectOptions,
        #[arg(long)]
        backup: String,
    },
    /// Submit a complete snapshot task. Wait for its final result before editing files.
    Create(ProjectMutationOptions),
    /// Restore the explicit backupId in JSON. The project remains in maintenance.
    Restore(ProjectInputOptions),
}

#[derive(Subcommand)]
enum MaintenanceCommand {
    Status(ProjectOptions),
    /// Enter maintenance and prepare a complete baseline before allowing changes.
    Begin(ProjectInputOptions),
    /// Check the project, end this maintenance, and require old pages to reload.
    End(ProjectInputOptions),
}

#[derive(Subcommand)]
enum DatabaseCommand {
    Schema(ProjectOptions),
    Execute(ProjectInputOptions),
    Transaction(ProjectInputOptions),
}

#[derive(Args)]
struct ProjectOptions {
    #[arg(long)]
    project: String,
    #[command(flatten)]
    output: OutputOptions,
}

#[derive(Args)]
struct InputOptions {
    /// Read a JSON object from this file, or - for standard input.
    #[arg(long)]
    input: String,
    #[command(flatten)]
    output: OutputOptions,
}

#[derive(Args)]
struct ProjectInputOptions {
    #[arg(long)]
    project: String,
    /// Associate this request with the current maintenance session.
    #[arg(long)]
    maintenance: Option<String>,
    #[command(flatten)]
    input: InputOptions,
}

#[derive(Args)]
struct ProjectMutationOptions {
    #[arg(long)]
    project: String,
    /// Read optional JSON fields from a file, or - for standard input.
    #[arg(long)]
    input: Option<String>,
    /// Must match maintenanceId when both this option and JSON supply it.
    #[arg(long)]
    maintenance: Option<String>,
    #[command(flatten)]
    output: OutputOptions,
}

#[derive(Debug, Serialize)]
struct CliError {
    code: &'static str,
    message: String,
    details: Value,
    retryable: bool,
}

impl CliError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: json!({}),
            retryable: false,
        }
    }
    fn unknown(message: impl Into<String>) -> Self {
        Self {
            details: json!({"operationOutcome": "unknown"}),
            ..Self::new("REQUEST_OUTCOME_UNKNOWN", message)
        }
    }
}

type Result<T> = std::result::Result<T, CliError>;

fn success(data: Value) -> Value {
    json!({"apiVersion": API_VERSION, "ok": true, "requestId": null, "data": data})
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let (json_output, result) = match cli.command {
        Command::Service {
            command: ServiceCommand::Status(options),
        } => (options.json, service_status().map(success)),
        Command::Env {
            command: EnvironmentCommand::Doctor(options),
        } => (options.json, environment_doctor().map(success)),
        Command::Env {
            command: EnvironmentCommand::Node { command },
        } => node_command(command),
        Command::Env {
            command: EnvironmentCommand::Git { command },
        } => git_command(command),
        Command::Task { command } => task_command(command),
        Command::Project { command } => project_command(command),
        Command::Db { command } => database_command(command),
        Command::Backup { command } => backup_command(command),
        Command::Maintenance { command } => maintenance_command(command),
        Command::Trash { command } => trash_command(command),
        Command::Workspace { command } => workspace_command(command),
        Command::Storage { command } => storage_command(command),
        Command::Source { command } => source_command(command),
    };
    let (output, code) = match result {
        Ok(value) => {
            let code = if value["ok"] == true { 0 } else { 1 };
            (value, code)
        }
        Err(error) => {
            let code = if error.code == "INPUT_INVALID" { 2 } else { 1 };
            (
                json!({"apiVersion": API_VERSION, "ok": false, "requestId": null, "error": error}),
                code,
            )
        }
    };
    if json_output {
        println!("{output}");
    } else if code == 0 {
        println!(
            "{}",
            serde_json::to_string_pretty(&output["data"]).expect("JSON serializes")
        );
    } else {
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&output).expect("JSON serializes")
        );
    }
    ExitCode::from(code)
}

fn project_command(command: ProjectCommand) -> (bool, Result<Value>) {
    match command {
        ProjectCommand::Metadata { command } => match command {
            MetadataCommand::Get(options) => (
                options.output.json,
                project_request(&options.project, "GET", "/metadata", None),
            ),
            MetadataCommand::Set(options) => (
                options.input.output.json,
                read_input(&options.input.input)
                    .and_then(|body| merge_maintenance(body, options.maintenance.as_deref()))
                    .and_then(|body| {
                        project_request(&options.project, "PATCH", "/metadata", Some(body))
                    }),
            ),
        },
        ProjectCommand::List(options) => (
            options.json,
            business_request("GET", "/api/v1/projects", None, None),
        ),
        ProjectCommand::Create(options) => (
            options.output.json,
            read_input(&options.input)
                .and_then(|input| business_request("POST", "/api/v1/projects", None, Some(input))),
        ),
        ProjectCommand::Export(options) => (
            options.input.output.json,
            input_request(options, "/exports"),
        ),
        ProjectCommand::Import(options) => (
            options.output.json,
            read_input(&options.input).and_then(|input| {
                validate_path_field(&input, "archivePath")?;
                if !matches!(input["mode"].as_str(), Some("original" | "copy")) {
                    return Err(CliError::new(
                        "INPUT_INVALID",
                        "Import mode must explicitly be original or copy.",
                    ));
                }
                business_request("POST", "/api/v1/projects/imports", None, Some(input))
            }),
        ),
        ProjectCommand::Copy(options) => {
            (options.output.json, mutation_request(options, "/copies"))
        }
        ProjectCommand::Delete(options) => {
            (options.output.json, mutation_request(options, "/trash"))
        }
        ProjectCommand::Info(options) => (
            options.output.json,
            project_request(&options.project, "GET", "", None),
        ),
        ProjectCommand::Url(options) => (
            options.output.json,
            project_request(&options.project, "POST", "/launch", Some(json!({}))),
        ),
        ProjectCommand::Build(options) => {
            (options.output.json, mutation_request(options, "/builds"))
        }
        ProjectCommand::Handoff(options) => (
            options.output.json,
            project_request(&options.project, "GET", "/handoff", None),
        ),
        ProjectCommand::Refresh(options) => {
            (options.output.json, mutation_request(options, "/refresh"))
        }
    }
}

fn trash_command(command: TrashCommand) -> (bool, Result<Value>) {
    let (options, method, suffix) = match command {
        TrashCommand::List(options) => {
            return (
                options.json,
                business_request("GET", "/api/v1/trash", None, None),
            )
        }
        TrashCommand::Info(options) => (options, "GET", ""),
        TrashCommand::Restore(options) => (options, "POST", "/restore"),
        TrashCommand::Purge(options) => (options, "POST", "/purge"),
    };
    (
        options.output.json,
        validate_reference_id(&options.trash, "trash").and_then(|()| {
            business_request(
                method,
                &format!("/api/v1/trash/{}{suffix}", options.trash),
                None,
                (method == "POST").then(|| json!({})),
            )
        }),
    )
}

fn workspace_command(command: WorkspaceCommand) -> (bool, Result<Value>) {
    match command {
        WorkspaceCommand::IdentityRepairPlan(options) => (
            options.json,
            platform_core::platform_support::require_runtime()
                .map_err(runtime_gate_error)
                .and_then(|_| {
                    RuntimePaths::identity_repair_preview()
                        .map(success)
                        .map_err(|e| {
                            CliError::new("WORKSPACE_IDENTITY_REPAIR_REJECTED", format!("{e:#}"))
                        })
                }),
        ),
        WorkspaceCommand::IdentityRepair(options) => (
            options.output.json,
            platform_core::platform_support::require_runtime()
                .map_err(runtime_gate_error)
                .and_then(|_| read_input(&options.input))
                .and_then(|input| {
                    RuntimePaths::identity_repair(input)
                        .map(success)
                        .map_err(|e| {
                            CliError::new("WORKSPACE_IDENTITY_REPAIR_REJECTED", format!("{e:#}"))
                        })
                }),
        ),
        WorkspaceCommand::CleanupPlan(options) => (
            options.json,
            business_request(
                "POST",
                "/api/v1/workspace/cleanup-plans",
                None,
                Some(json!({})),
            ),
        ),
        WorkspaceCommand::Cleanup(options) => (
            options.output.json,
            read_input(&options.input).and_then(|input| {
                business_request("POST", "/api/v1/workspace/cleanups", None, Some(input))
            }),
        ),
        WorkspaceCommand::Storage(options) => (
            options.json,
            business_request("GET", "/api/v1/workspace/storage", None, None),
        ),
        WorkspaceCommand::Info(options) => (
            options.json,
            business_request("GET", "/api/v1/workspace", None, None),
        ),
        WorkspaceCommand::Migrate(options) => (
            options.output.json,
            read_input(&options.input).and_then(|input| {
                validate_path_field(&input, "destinationPath")?;
                business_request("POST", "/api/v1/workspace/migrations", None, Some(input))
            }),
        ),
    }
}

fn storage_command(command: StorageCommand) -> (bool, Result<Value>) {
    match command {
        StorageCommand::Info(options) => (
            options.output.json,
            project_request(&options.project, "GET", "/storage", None),
        ),
        StorageCommand::Scan(options) => (
            options.output.json,
            project_request(&options.project, "POST", "/storage/scans", Some(json!({}))),
        ),
        StorageCommand::Cleanup(options) => (
            options.input.output.json,
            input_request(options, "/storage/cleanups"),
        ),
        StorageCommand::Files(options) => (
            options.input.output.json,
            read_input(&options.input.input).and_then(|input| {
                let mut query = reqwest::Url::parse("http://127.0.0.1/").expect("fixed URL");
                for (key, value) in input.as_object().expect("validated object") {
                    if ![
                        "type", "prefix", "query", "sort", "order", "limit", "cursor",
                    ]
                    .contains(&key.as_str())
                    {
                        return Err(CliError::new("INPUT_INVALID", "Unknown attachment filter"));
                    }
                    let text = match value {
                        Value::String(v) => v.clone(),
                        Value::Number(n) if key == "limit" => n.to_string(),
                        _ => {
                            return Err(CliError::new(
                                "INPUT_INVALID",
                                "Attachment filters must be strings; limit must be an integer",
                            ))
                        }
                    };
                    query.query_pairs_mut().append_pair(key, &text);
                }
                project_request(
                    &options.project,
                    "GET",
                    &format!("/storage/files?{}", query.query().unwrap_or("")),
                    None,
                )
            }),
        ),
    }
}

fn validate_path_field(input: &Value, field: &str) -> Result<()> {
    if !input[field]
        .as_str()
        .is_some_and(|path| !path.is_empty() && Path::new(path).is_absolute())
    {
        return Err(CliError::new(
            "INPUT_INVALID",
            format!("{field} must be an explicit absolute path."),
        ));
    }
    Ok(())
}

fn database_command(command: DatabaseCommand) -> (bool, Result<Value>) {
    match command {
        DatabaseCommand::Schema(options) => (
            options.output.json,
            project_request(&options.project, "GET", "/db/schema", None),
        ),
        DatabaseCommand::Execute(options) => (
            options.input.output.json,
            input_request(options, "/db/execute"),
        ),
        DatabaseCommand::Transaction(options) => (
            options.input.output.json,
            input_request(options, "/db/transaction"),
        ),
    }
}

fn backup_command(command: BackupCommand) -> (bool, Result<Value>) {
    match command {
        BackupCommand::List(options) => (
            options.output.json,
            project_request(&options.project, "GET", "/backups", None),
        ),
        BackupCommand::Info { project, backup } => (
            project.output.json,
            validate_reference_id(&backup, "backup").and_then(|()| {
                project_request(&project.project, "GET", &format!("/backups/{backup}"), None)
            }),
        ),
        BackupCommand::Create(options) => {
            (options.output.json, mutation_request(options, "/backups"))
        }
        BackupCommand::Restore(options) => (
            options.input.output.json,
            input_request(options, "/restore"),
        ),
    }
}

fn maintenance_command(command: MaintenanceCommand) -> (bool, Result<Value>) {
    match command {
        MaintenanceCommand::Status(options) => (
            options.output.json,
            project_request(&options.project, "GET", "/maintenance", None),
        ),
        MaintenanceCommand::Begin(options) => (
            options.input.output.json,
            input_request(options, "/maintenance"),
        ),
        MaintenanceCommand::End(options) => (
            options.input.output.json,
            input_request(options, "/maintenance/end"),
        ),
    }
}

fn validate_reference_id(id: &str, name: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(CliError::new(
            "INPUT_INVALID",
            format!("{name} must be an explicit nonempty ID without URL or path characters."),
        ));
    }
    Ok(())
}

fn merge_maintenance(mut input: Value, maintenance: Option<&str>) -> Result<Value> {
    let object = input
        .as_object_mut()
        .ok_or_else(|| CliError::new("INPUT_INVALID", "JSON input must be an object."))?;
    let from_json = match object.get("maintenanceId") {
        Some(value) => {
            let id = value.as_str().ok_or_else(|| {
                CliError::new("INPUT_INVALID", "maintenanceId must be a nonempty string.")
            })?;
            validate_reference_id(id, "maintenanceId")?;
            Some(id)
        }
        None => None,
    };
    if let Some(id) = maintenance {
        validate_reference_id(id, "--maintenance")?;
        if from_json.is_some_and(|from_json| from_json != id) {
            return Err(CliError::new(
                "INPUT_INVALID",
                "--maintenance and input maintenanceId must match. No request was sent.",
            ));
        }
        object.insert("maintenanceId".to_owned(), json!(id));
    }
    Ok(input)
}

fn mutation_request(options: ProjectMutationOptions, route: &str) -> Result<Value> {
    let input = options
        .input
        .as_deref()
        .map(read_input)
        .transpose()?
        .unwrap_or_else(|| json!({}));
    let input = merge_maintenance(input, options.maintenance.as_deref())?;
    project_request(&options.project, "POST", route, Some(input))
}

fn input_request(options: ProjectInputOptions, route: &str) -> Result<Value> {
    let input = merge_maintenance(
        read_input(&options.input.input)?,
        options.maintenance.as_deref(),
    )?;
    if route == "/exports" {
        validate_path_field(&input, "destinationPath")?;
    }
    if route == "/restore" {
        let backup = input["backupId"].as_str().ok_or_else(|| {
            CliError::new(
                "INPUT_INVALID",
                "Restore requires an explicit backupId in JSON input.",
            )
        })?;
        validate_reference_id(backup, "backupId")?;
    }
    if route == "/maintenance/end" && input.get("maintenanceId").is_none() {
        return Err(CliError::new(
            "INPUT_INVALID",
            "Ending maintenance requires the current maintenanceId.",
        ));
    }
    project_request(&options.project, "POST", route, Some(input))
}

fn source_command(command: SourceCommand) -> (bool, Result<Value>) {
    match command {
        SourceCommand::List(output) => (
            output.json,
            business_request("GET", "/api/v1/sources", None, None),
        ),
        SourceCommand::Get { source, output } => (
            output.json,
            validate_reference_id(&source, "source").and_then(|()| {
                business_request("GET", &format!("/api/v1/sources/{source}"), None, None)
            }),
        ),
        SourceCommand::Inspect { path, output } => (
            output.json,
            if Path::new(&path).is_absolute() {
                business_request(
                    "POST",
                    "/api/v1/sources/inspect",
                    None,
                    Some(json!({"sourcePath": path})),
                )
            } else {
                Err(CliError::new(
                    "INPUT_INVALID",
                    "--path must be an absolute source directory.",
                ))
            },
        ),
        SourceCommand::Acquire { url, name, output } => (
            output.json,
            business_request(
                "POST",
                "/api/v1/sources/acquisitions",
                None,
                Some(json!({"url":url, "name":name})),
            ),
        ),
        SourceCommand::Cancel(options) => (
            options.output.json,
            validate_reference_id(&options.task, "task").and_then(|()| {
                business_request(
                    "POST",
                    &format!("/api/v1/sources/tasks/{}/cancel", options.task),
                    None,
                    Some(json!({})),
                )
            }),
        ),
    }
}

fn git_command(command: GitCommand) -> (bool, Result<Value>) {
    match command {
        GitCommand::Check { url, output } => (
            output.json,
            business_request(
                "POST",
                "/api/v1/environment/git/remote-check",
                None,
                Some(json!({"url": url})),
            ),
        ),
        GitCommand::Doctor(output) => (
            output.json,
            business_request("GET", "/api/v1/environment/git", None, None),
        ),
        GitCommand::Set { path, output } => (
            output.json,
            if Path::new(&path).is_absolute() {
                business_request(
                    "PUT",
                    "/api/v1/environment/git",
                    None,
                    Some(json!({"gitPath": path})),
                )
            } else {
                Err(CliError::new(
                    "INPUT_INVALID",
                    "--path must be an absolute Git executable path.",
                ))
            },
        ),
        GitCommand::Clear(output) => (
            output.json,
            business_request(
                "PUT",
                "/api/v1/environment/git",
                None,
                Some(json!({"gitPath": null})),
            ),
        ),
    }
}

fn node_command(command: NodeCommand) -> (bool, Result<Value>) {
    match command {
        NodeCommand::Set { path, output } => (
            output.json,
            if Path::new(&path).is_absolute() {
                business_request(
                    "PUT",
                    "/api/v1/environment/node",
                    None,
                    Some(json!({"nodePath": path})),
                )
            } else {
                Err(CliError::new(
                    "INPUT_INVALID",
                    "--path must be an absolute Node executable path.",
                ))
            },
        ),
        NodeCommand::Clear(output) => (
            output.json,
            business_request(
                "PUT",
                "/api/v1/environment/node",
                None,
                Some(json!({"nodePath": null})),
            ),
        ),
    }
}

fn task_command(command: TaskCommand) -> (bool, Result<Value>) {
    match command {
        TaskCommand::Get(options) => (options.output.json, task_request(&options.task, None)),
        TaskCommand::Logs { task, cursor } => {
            (task.output.json, task_request(&task.task, Some(cursor)))
        }
        TaskCommand::Wait { task, timeout } => (
            task.output.json,
            wait_for_task(&task.task, Duration::from_secs(timeout)),
        ),
    }
}

fn validate_task_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(CliError::new(
            "INPUT_INVALID",
            "--task must be an explicit task ID without URL or path characters.",
        ));
    }
    Ok(())
}

fn validate_task_response(value: &Value, id: &str, logs: bool) -> Result<()> {
    if value["ok"] != true {
        return Ok(());
    }
    if value["data"]["taskId"].as_str() != Some(id) {
        return Err(CliError::new(
            "SERVICE_PROTOCOL_ERROR",
            "The returned task does not match the requested task ID.",
        ));
    }
    if !logs {
        let terminal = matches!(
            value["data"]["status"].as_str(),
            Some("succeeded" | "failed" | "interrupted")
        );
        let active = matches!(value["data"]["status"].as_str(), Some("queued" | "running"));
        if (!terminal && !active) || value["data"]["final"].as_bool() != Some(terminal) {
            return Err(CliError::new(
                "SERVICE_PROTOCOL_ERROR",
                "The task status and final marker are inconsistent.",
            ));
        }
    }
    Ok(())
}

fn task_request(id: &str, cursor: Option<u64>) -> Result<Value> {
    validate_task_id(id)?;
    let suffix = cursor
        .map(|value| format!("/logs?offset={value}"))
        .unwrap_or_default();
    let value = business_request("GET", &format!("/api/v1/tasks/{id}{suffix}"), None, None)?;
    validate_task_response(&value, id, cursor.is_some())?;
    Ok(value)
}

fn wait_for_task(id: &str, timeout: Duration) -> Result<Value> {
    validate_task_id(id)?;
    let connection = BusinessConnection::connect()?;
    wait_for_connected_task(&connection, id, timeout)
}

fn wait_for_connected_task(
    connection: &BusinessConnection,
    id: &str,
    timeout: Duration,
) -> Result<Value> {
    wait_for_connected_task_with_discovery(connection, id, timeout, BusinessConnection::connect)
}

fn wait_for_connected_task_with_discovery(
    connection: &BusinessConnection,
    id: &str,
    timeout: Duration,
    mut rediscover: impl FnMut() -> Result<BusinessConnection>,
) -> Result<Value> {
    let route = format!("/api/v1/tasks/{id}");
    let deadline = Instant::now() + timeout;
    let initial_timeout = if timeout.is_zero() {
        Duration::from_secs(3)
    } else {
        timeout.min(Duration::from_secs(3))
    };
    let mut refreshed_connection = None;
    let mut current = connection.request("GET", &route, None, None, initial_timeout)?;
    validate_task_response(&current, id, false)?;
    while current["ok"] == true && current["data"]["final"] == false {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        std::thread::sleep(remaining.min(Duration::from_millis(750)));
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        // Repeated reads are safe; task submission and writes are never replayed.
        let active_connection = refreshed_connection.as_ref().unwrap_or(connection);
        let response = active_connection.request(
            "GET",
            &route,
            None,
            None,
            remaining.min(Duration::from_secs(3)),
        );
        let migration = current["data"]["type"] == "workspaceMigration";
        let reconnect = migration
            && match &response {
                Err(_) => true,
                Ok(value) => {
                    value["ok"] == false
                        && matches!(
                            value["error"]["code"].as_str(),
                            Some(
                                "UNAUTHENTICATED"
                                    | "UNAUTHORIZED"
                                    | "INVALID_TOKEN"
                                    | "SERVICE_INSTANCE_CHANGED"
                                    | "WORKSPACE_MIGRATING"
                            )
                        )
                }
            };
        if reconnect {
            // Discovery and task status are read-only. Never repeat the migration request.
            if let Ok(fresh) = rediscover() {
                refreshed_connection = Some(fresh);
            }
            continue;
        }
        match response {
            Ok(next) => {
                validate_task_response(&next, id, false)?;
                current = next;
            }
            Err(_) if Instant::now() >= deadline => break,
            Err(error) => return Err(error),
        }
    }
    if current["ok"] == true {
        current["data"]["waitTimedOut"] = json!(current["data"]["final"] == false);
    }
    Ok(current)
}

fn read_input(path: &str) -> Result<Value> {
    let reader: Box<dyn Read> =
        if path == "-" {
            Box::new(std::io::stdin())
        } else {
            Box::new(std::fs::File::open(path).map_err(|e| {
                CliError::new("INPUT_INVALID", format!("Cannot read JSON input: {e}"))
            })?)
        };
    parse_input(reader)
}

fn parse_input(reader: impl Read) -> Result<Value> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| CliError::new("INPUT_INVALID", format!("Cannot read JSON input: {e}")))?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(CliError::new(
            "INPUT_INVALID",
            "JSON input exceeds 2,000,000 bytes.",
        ));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|e| CliError::new("INPUT_INVALID", format!("Invalid JSON input: {e}")))?;
    if !value.is_object() {
        return Err(CliError::new(
            "INPUT_INVALID",
            "JSON input must be an object.",
        ));
    }
    Ok(value)
}

fn project_request(id: &str, method: &str, suffix: &str, body: Option<Value>) -> Result<Value> {
    if id.is_empty()
        || id.len() > 64
        || !id.as_bytes()[0].is_ascii_lowercase()
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(CliError::new(
            "INPUT_INVALID",
            "--project must be a valid explicit project ID.",
        ));
    }
    business_request(
        method,
        &format!("/api/v1/projects/{id}{suffix}"),
        Some(id),
        body,
    )
}

fn business_request(
    method: &str,
    route: &str,
    project_id: Option<&str>,
    body: Option<Value>,
) -> Result<Value> {
    BusinessConnection::connect()?.request(
        method,
        route,
        project_id,
        body,
        Duration::from_secs(130),
    )
}

struct BusinessConnection {
    client: Client,
    base_url: String,
    token: String,
}

impl BusinessConnection {
    fn connect() -> Result<Self> {
        let paths = discover_paths()?;
        let discovery = discover_service(&paths)?;
        // Verify the public instance and private runtime before using credentials.
        query_runtime(&paths, &discovery)?;
        let token = paths.read_control_token().map_err(|_| {
            CliError::new(
                "CREDENTIAL_INVALID",
                "Development credential is unavailable.",
            )
        })?;
        Ok(Self {
            client: client_with_timeout(Duration::from_secs(130))?,
            base_url: discovery.base_url,
            token,
        })
    }

    fn request(
        &self,
        method: &str,
        route: &str,
        project_id: Option<&str>,
        body: Option<Value>,
        timeout: Duration,
    ) -> Result<Value> {
        let mut request = self
            .client
            .request(
                method.parse().expect("fixed method"),
                format!("{}{route}", self.base_url),
            )
            .bearer_auth(&self.token)
            .timeout(timeout);
        if let Some(body) = body {
            request = request.json(&body);
        }

        let response = request.send().map_err(|_| CliError::unknown("The request did not return a result. A write may have committed. Query the data before deciding to retry."))?;
        if response.status().is_redirection() {
            return Err(CliError::new(
                "SERVICE_PROTOCOL_ERROR",
                "The service returned a redirect. It was not followed.",
            ));
        }
        let status = response.status();
        let mut bytes = Vec::new();
        response.take(MAX_BUSINESS_RESPONSE_BYTES + 1).read_to_end(&mut bytes)
        .map_err(|_| CliError::unknown("The response was interrupted. A write may have committed; do not automatically retry."))?;
        if bytes.len() as u64 > MAX_BUSINESS_RESPONSE_BYTES {
            return Err(CliError::unknown("The service response exceeded the response limit. Query the data before retrying a write."));
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
            CliError::unknown(
                "The service did not return a valid result. A write may have committed.",
            )
        })?;
        validate_business_response(&value, project_id, status.is_success())?;
        Ok(value)
    }
}

fn validate_business_response(
    value: &Value,
    project_id: Option<&str>,
    http_success: bool,
) -> Result<()> {
    let valid = value["apiVersion"].as_u64() == Some(u64::from(API_VERSION))
        && value["requestId"].as_str().is_some_and(|s| !s.is_empty())
        && value["ok"].as_bool() == Some(http_success)
        && (if http_success {
            value.get("data").is_some()
        } else {
            value["error"]["code"].is_string() && value["error"]["message"].is_string()
        })
        && project_id.is_none_or(|id| value["projectId"].as_str() == Some(id));
    if !valid {
        return Err(CliError::unknown("The response envelope or project identity is invalid. A write may have committed; no retry was made."));
    }
    Ok(())
}

fn runtime_gate_error(gate: RuntimeSupportError) -> CliError {
    let api_error = platform_core::ApiError::from(gate);
    CliError {
        code: gate.code,
        message: api_error.message,
        details: api_error.details,
        retryable: api_error.retryable,
    }
}

fn discover_paths() -> Result<RuntimePaths> {
    RuntimePaths::discover().map_err(|error| {
        if let Some(gate) = error.downcast_ref::<RuntimeSupportError>() {
            return runtime_gate_error(*gate);
        }
        let missing = error.chain().any(|cause| {
            cause.downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
        });
        if missing {
            CliError::new("SERVICE_NOT_RUNNING", "The development platform is not initialized or running. Open 熊猫应用小抽屉（开发版） first.")
        } else {
            CliError::new("ENVIRONMENT_INVALID", format!("The isolated development environment could not be verified: {error}"))
        }
    })
}

fn discover_service(paths: &RuntimePaths) -> Result<Discovery> {
    paths.read_discovery().map_err(|error| {
        let missing = error.chain().any(|cause| {
            cause.downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
        });
        if missing {
            CliError::new("SERVICE_NOT_RUNNING", "The development service has no active discovery record. Open 熊猫应用小抽屉（开发版） first.")
        } else {
            CliError::new("DISCOVERY_INVALID", format!("Development service discovery could not be verified: {error}"))
        }
    })
}

fn service_status() -> Result<Value> {
    let paths = discover_paths()?;
    let discovery = discover_service(&paths)?;
    Ok(json!({ "running": true, "runtime": query_runtime(&paths, &discovery)? }))
}

fn environment_doctor() -> Result<Value> {
    let mut result = json!({
        "scope": "m2-platform-runtime",
        "displayName": DISPLAY_NAME,
        "identifier": APP_IDENTIFIER,
        "environment": ENVIRONMENT,
        "apiVersion": API_VERSION,
        "cliVersion": env!("CARGO_PKG_VERSION"),
        "service": { "status": "offline" },
        "candidatePaths": null,
        "runtime": null,
        "buildEnvironment": {
            "status": "unavailable",
            "message": "Open the desktop application to inspect the actual build environment. The CLI does not infer it from its own PATH."
        }
    });
    let paths = match discover_paths() {
        Ok(paths) => paths,
        Err(error) => return environment_service_failure(result, error),
    };
    result["candidatePaths"] = json!({
        "dataRoot": paths.data_root(),
        "workspaceRoot": paths.workspace_root(),
        "connectionPath": paths.connection_path()
    });
    let runtime = discover_service(&paths).and_then(|discovery| query_runtime(&paths, &discovery));
    match runtime {
        Ok(runtime) => {
            result["service"] = json!({ "status": "online" });
            result["runtime"] = runtime;
            let environment = business_request("GET", "/api/v1/environment", None, None)?;
            if environment["ok"] == true {
                result["buildEnvironment"] = environment["data"].clone();
            } else {
                result["buildEnvironment"] =
                    json!({"status":"unavailable", "error": environment["error"]});
            }
            Ok(result)
        }
        Err(error) => environment_service_failure(result, error),
    }
}

fn environment_service_failure(mut result: Value, error: CliError) -> Result<Value> {
    let (status, message) = match error.code {
        "SERVICE_NOT_RUNNING" => (
            "offline",
            "No active development service record is available. Open the desktop application to inspect its build environment.",
        ),
        "SERVICE_UNREACHABLE" => (
            "unreachable",
            "The recorded service could not be reached. Its running state and build environment have not been verified.",
        ),
        "LOCAL_ACCESS_DENIED" => (
            "blocked",
            "Local access was denied. The service's running state and build environment have not been verified.",
        ),
        _ => return Err(error),
    };
    result["service"] = json!({"status": status, "diagnostic": error});
    result["buildEnvironment"] = json!({"status": "unavailable", "message": message});
    Ok(result)
}

fn local_client() -> Result<Client> {
    client_with_timeout(Duration::from_secs(3))
}

fn client_with_timeout(timeout: Duration) -> Result<Client> {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(1))
        .timeout(timeout)
        .build()
        .map_err(|_| {
            CliError::new(
                "CLIENT_INITIALIZATION_FAILED",
                "Could not initialize the local HTTP client.",
            )
        })
}

fn query_runtime(paths: &RuntimePaths, discovery: &Discovery) -> Result<Value> {
    // Verify before even making an unauthenticated request. Never infer a port,
    // follow a redirect or consult a proxy when handling local credentials.
    let port = discovery.port().map_err(|_| {
        CliError::new(
            "DISCOVERY_INVALID",
            "The development service URL is not a literal IPv4 loopback origin.",
        )
    })?;
    if discovery.api_version != API_VERSION
        || discovery.identifier != APP_IDENTIFIER
        || discovery.environment != ENVIRONMENT
        || discovery.workspace_root != paths.workspace_root()
        || discovery.instance_id.is_empty()
        || discovery.pid == 0
    {
        return Err(identity_error("discovery"));
    }
    let client = local_client()?;
    let host = format!("127.0.0.1:{port}");
    let health = fetch_json(
        client
            .get(format!("{}/healthz", discovery.base_url))
            .header(reqwest::header::HOST, &host),
    )?;
    validate_identity(&health, discovery, "health response")?;

    // Read the token only after the public response matches this exact instance.
    let token = paths.read_control_token().map_err(|_| {
        CliError::new("CREDENTIAL_INVALID", "The development service credential is unavailable or invalid. Reopen 熊猫应用小抽屉（开发版）.")
    })?;
    let runtime = fetch_json(
        client
            .get(format!("{}/api/v1/runtime", discovery.base_url))
            .header(reqwest::header::HOST, host)
            .bearer_auth(token),
    )?;
    validate_runtime(&runtime, discovery, paths.data_root())?;
    Ok(runtime)
}

fn local_connection_error(error: &(dyn std::error::Error + 'static)) -> CliError {
    let mut cause = Some(error);
    while let Some(error) = cause {
        if error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
        {
            return CliError::new(
                "LOCAL_ACCESS_DENIED",
                "Local access to the recorded development service was denied. Allow this tool to connect to the local platform through its permission controls, or manually run the managed CLI command in a permitted terminal. The service's running state is unverified; no retry was made.",
            );
        }
        cause = error.source();
    }
    // Never include the request URL, headers, token or raw transport error.
    CliError::new(
        "SERVICE_UNREACHABLE",
        "A development service record exists, but this process could not reach that service. It may be unavailable or local access may be restricted; its running state is unverified. Check the application and local connection permissions. No retry was made.",
    )
}

fn fetch_json(request: RequestBuilder) -> Result<Value> {
    let response = request
        .send()
        .map_err(|error| local_connection_error(&error))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(CliError::new("SERVICE_UNAUTHORIZED", "The development service rejected this instance's credential. Retry after confirming the application is running."));
    }
    if !response.status().is_success() {
        return Err(CliError::new(
            "SERVICE_PROTOCOL_ERROR",
            format!(
                "The local service returned HTTP {}. Redirects are not followed.",
                response.status().as_u16()
            ),
        ));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            CliError::new(
                "SERVICE_PROTOCOL_ERROR",
                "The local service response could not be read.",
            )
        })?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(CliError::new(
            "SERVICE_PROTOCOL_ERROR",
            "The local service response exceeded the M0 diagnostic size limit.",
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        CliError::new(
            "SERVICE_PROTOCOL_ERROR",
            "The local service did not return a valid JSON response.",
        )
    })
}

fn identity_error(source: &str) -> CliError {
    CliError::new(
        "SERVICE_IDENTITY_MISMATCH",
        format!(
            "The {source} does not match the isolated development instance. Refusing to connect."
        ),
    )
}

fn validate_identity(value: &Value, discovery: &Discovery, source: &str) -> Result<()> {
    if value.get("apiVersion").and_then(Value::as_u64) != Some(u64::from(API_VERSION))
        || value.get("identifier").and_then(Value::as_str) != Some(APP_IDENTIFIER)
        || value.get("environment").and_then(Value::as_str) != Some(ENVIRONMENT)
        || value.get("instanceId").and_then(Value::as_str) != Some(discovery.instance_id.as_str())
    {
        return Err(identity_error(source));
    }
    Ok(())
}

fn validate_runtime(value: &Value, discovery: &Discovery, data_root: &Path) -> Result<()> {
    validate_identity(value, discovery, "runtime response")?;
    if value.get("baseUrl").and_then(Value::as_str) != Some(discovery.base_url.as_str())
        || value
            .get("workspaceRoot")
            .and_then(Value::as_str)
            .map(Path::new)
            != Some(discovery.workspace_root.as_path())
        || value.get("dataRoot").and_then(Value::as_str).map(Path::new) != Some(data_root)
    {
        return Err(identity_error("runtime paths"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;

    fn discovery() -> Discovery {
        Discovery {
            api_version: API_VERSION,
            instance_id: "a-development-instance".into(),
            pid: 123,
            base_url: "http://127.0.0.1:12345".into(),
            workspace_root: "/development/workspace".into(),
            environment: ENVIRONMENT.into(),
            identifier: APP_IDENTIFIER.into(),
        }
    }

    fn runtime(discovery: &Discovery) -> Value {
        json!({
            "apiVersion": API_VERSION,
            "instanceId": discovery.instance_id,
            "identifier": APP_IDENTIFIER,
            "environment": ENVIRONMENT,
            "baseUrl": discovery.base_url,
            "workspaceRoot": discovery.workspace_root,
            "dataRoot": "/development"
        })
    }

    #[test]
    fn maintenance_alias_preserves_input_and_rejects_conflicts_before_requests() {
        let input = json!({"sql":"SELECT ?", "params":[42], "maintenanceId":"maintenance_a"});
        assert_eq!(
            merge_maintenance(input.clone(), Some("maintenance_a")).unwrap(),
            input
        );
        assert_eq!(
            merge_maintenance(json!({"sql":"SELECT 1"}), Some("maintenance_a")).unwrap()
                ["maintenanceId"],
            "maintenance_a"
        );
        assert_eq!(
            merge_maintenance(input, Some("maintenance_b"))
                .unwrap_err()
                .code,
            "INPUT_INVALID"
        );
        for value in [
            json!(null),
            json!(42),
            json!(""),
            json!("a/b"),
            json!("a?b"),
        ] {
            assert_eq!(
                merge_maintenance(json!({"maintenanceId":value}), None)
                    .unwrap_err()
                    .code,
                "INPUT_INVALID"
            );
        }
        assert_eq!(
            merge_maintenance(json!({}), Some("")).unwrap_err().code,
            "INPUT_INVALID"
        );
    }

    #[test]
    fn maintenance_and_backup_commands_parse_without_changing_existing_forms() {
        for args in [
            vec!["cli", "backup", "list", "--project", "p_test", "--json"],
            vec![
                "cli",
                "backup",
                "info",
                "--project",
                "p_test",
                "--backup",
                "backup_a",
            ],
            vec!["cli", "backup", "create", "--project", "p_test"],
            vec![
                "cli",
                "backup",
                "restore",
                "--project",
                "p_test",
                "--input",
                "-",
                "--maintenance",
                "maintenance_a",
            ],
            vec![
                "cli",
                "maintenance",
                "begin",
                "--project",
                "p_test",
                "--input",
                "-",
            ],
            vec![
                "cli",
                "maintenance",
                "end",
                "--project",
                "p_test",
                "--input",
                "-",
                "--maintenance",
                "maintenance_a",
            ],
            vec!["cli", "project", "build", "--project", "p_test"],
            vec![
                "cli",
                "project",
                "refresh",
                "--project",
                "p_test",
                "--maintenance",
                "maintenance_a",
            ],
            vec![
                "cli",
                "db",
                "transaction",
                "--project",
                "p_test",
                "--input",
                "-",
                "--maintenance",
                "maintenance_a",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
        assert!(Cli::try_parse_from(["cli", "backup", "restore", "--project", "p_test"]).is_err());
    }

    #[test]
    fn lifecycle_commands_require_explicit_targets_and_inputs() {
        for args in [
            vec![
                "cli",
                "project",
                "export",
                "--project",
                "p_test",
                "--input",
                "-",
                "--maintenance",
                "maintenance_a",
            ],
            vec!["cli", "project", "import", "--input", "-"],
            vec!["cli", "project", "copy", "--project", "p_test"],
            vec![
                "cli",
                "project",
                "delete",
                "--project",
                "p_test",
                "--maintenance",
                "maintenance_a",
            ],
            vec!["cli", "trash", "list", "--json"],
            vec!["cli", "trash", "info", "--trash", "trash_a"],
            vec!["cli", "trash", "restore", "--trash", "trash_a"],
            vec!["cli", "trash", "purge", "--trash", "trash_a"],
            vec!["cli", "workspace", "info", "--json"],
            vec!["cli", "workspace", "identity-repair-plan", "--json"],
            vec![
                "cli",
                "workspace",
                "identity-repair",
                "--input",
                "-",
                "--json",
            ],
            vec!["cli", "workspace", "migrate", "--input", "-"],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
        for args in [
            vec!["cli", "project", "export", "--project", "p_test"],
            vec!["cli", "project", "import"],
            vec!["cli", "trash", "purge"],
            vec!["cli", "workspace", "migrate"],
            vec!["cli", "workspace", "identity-repair"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
        for value in [
            json!({}),
            json!({"destinationPath": "relative/archive.zip"}),
            json!({"destinationPath": null}),
        ] {
            assert_eq!(
                validate_path_field(&value, "destinationPath")
                    .unwrap_err()
                    .code,
                "INPUT_INVALID"
            );
        }
        assert!(validate_path_field(
            &json!({"archivePath": "/Users/test/export.zip"}),
            "archivePath"
        )
        .is_ok());
        let (_, result) = trash_command(TrashCommand::Purge(TrashOptions {
            trash: "../project".into(),
            output: OutputOptions { json: true },
        }));
        assert_eq!(result.unwrap_err().code, "INPUT_INVALID");
    }

    #[test]
    fn migration_wait_rediscovers_rotated_credentials_without_replaying_writes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            for index in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = [0; 4096];
                let size = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..size]);
                assert!(request.starts_with("GET /api/v1/tasks/task_migration "));
                assert!(request.to_lowercase().contains(if index == 2 {
                    "authorization: bearer new-token"
                } else {
                    "authorization: bearer old-token"
                }));
                let (status, body) = if index == 1 {
                    (
                        "401 Unauthorized",
                        json!({"apiVersion":1,"ok":false,"requestId":"req_rotated","error":{"code":"UNAUTHENTICATED","message":"Credential rotated"}}),
                    )
                } else {
                    (
                        "200 OK",
                        json!({"apiVersion":1,"ok":true,"requestId":"req_task","data":{"taskId":"task_migration","projectId":null,"type":"workspaceMigration","status":if index == 2 {"succeeded"} else {"running"},"final":index == 2}}),
                    )
                };
                let body = body.to_string();
                write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let connection = BusinessConnection {
            client: local_client().unwrap(),
            base_url: base_url.clone(),
            token: "old-token".into(),
        };
        let mut discoveries = 0;
        let result = wait_for_connected_task_with_discovery(
            &connection,
            "task_migration",
            Duration::from_secs(5),
            || {
                discoveries += 1;
                Ok(BusinessConnection {
                    client: local_client().unwrap(),
                    base_url: base_url.clone(),
                    token: "new-token".into(),
                })
            },
        )
        .unwrap();
        assert_eq!(discoveries, 1);
        assert_eq!(result["data"]["status"], "succeeded");
        assert_eq!(result["data"]["waitTimedOut"], false);
        server.join().unwrap();
    }

    #[test]
    fn rejects_another_instance_environment_or_workspace() {
        let discovery = discovery();
        let original = runtime(&discovery);
        assert!(validate_runtime(&original, &discovery, Path::new("/development")).is_ok());
        for (field, replacement) in [
            ("instanceId", json!("another-instance")),
            ("environment", json!("production")),
            ("identifier", json!("com.example.production")),
            ("apiVersion", json!(2)),
            ("workspaceRoot", json!("/production/workspace")),
            ("dataRoot", json!("/production")),
            ("baseUrl", json!("http://127.0.0.1:54321")),
        ] {
            let mut changed = original.clone();
            changed[field] = replacement;
            assert_eq!(
                validate_runtime(&changed, &discovery, Path::new("/development"))
                    .unwrap_err()
                    .code,
                "SERVICE_IDENTITY_MISMATCH",
                "{field}"
            );
        }
    }

    #[test]
    fn rejects_missing_or_wrongly_typed_identity_fields() {
        let discovery = discovery();
        let mut value = runtime(&discovery);
        value["apiVersion"] = json!("1");
        assert!(validate_identity(&value, &discovery, "health").is_err());
        value = runtime(&discovery);
        value.as_object_mut().unwrap().remove("instanceId");
        assert!(validate_identity(&value, &discovery, "health").is_err());
    }

    #[test]
    fn connection_failures_classify_permission_sources_without_exposing_details() {
        #[derive(Debug)]
        struct TransportError(Box<dyn std::error::Error + Send + Sync>);
        impl std::fmt::Display for TransportError {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "http://127.0.0.1:12345/?token=private-sentinel")
            }
        }
        impl std::error::Error for TransportError {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(self.0.as_ref())
            }
        }
        for (kind, expected) in [
            (std::io::ErrorKind::PermissionDenied, "LOCAL_ACCESS_DENIED"),
            (std::io::ErrorKind::ConnectionRefused, "SERVICE_UNREACHABLE"),
            (std::io::ErrorKind::TimedOut, "SERVICE_UNREACHABLE"),
            (std::io::ErrorKind::NotFound, "SERVICE_UNREACHABLE"),
        ] {
            let error = TransportError(Box::new(TransportError(Box::new(std::io::Error::new(
                kind,
                "Authorization: Bearer private-sentinel",
            )))));
            let diagnostic = local_connection_error(&error);
            assert_eq!(diagnostic.code, expected);
            assert!(!diagnostic.retryable);
            let serialized = serde_json::to_string(&diagnostic).unwrap();
            assert!(!serialized.contains("private-sentinel"));
            assert!(!serialized.contains("127.0.0.1"));
            assert_eq!(diagnostic.details, json!({}));
        }
    }

    #[test]
    fn doctor_distinguishes_missing_record_unreachable_and_blocked_service() {
        for (code, status) in [
            ("SERVICE_NOT_RUNNING", "offline"),
            ("SERVICE_UNREACHABLE", "unreachable"),
            ("LOCAL_ACCESS_DENIED", "blocked"),
        ] {
            let result = environment_service_failure(
                json!({"runtime":null, "candidatePaths":{"dataRoot":"/disposable"}}),
                CliError::new(code, "fixed diagnostic"),
            )
            .unwrap();
            assert_eq!(result["service"]["status"], status);
            assert_eq!(result["service"]["diagnostic"]["code"], code);
            assert_eq!(result["service"]["diagnostic"]["retryable"], false);
            assert_eq!(result["buildEnvironment"]["status"], "unavailable");
            assert!(result["runtime"].is_null());
            assert_eq!(result["candidatePaths"]["dataRoot"], "/disposable");
        }
        assert_eq!(
            environment_service_failure(json!({}), identity_error("discovery"))
                .unwrap_err()
                .code,
            "SERVICE_IDENTITY_MISMATCH"
        );
    }

    #[test]
    fn refused_local_connection_is_unreachable_not_missing_discovery() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let error = fetch_json(
            local_client()
                .unwrap()
                .get(format!("{origin}/healthz"))
                .bearer_auth("private-refused-sentinel"),
        )
        .unwrap_err();
        assert_eq!(error.code, "SERVICE_UNREACHABLE");
        assert!(!error.retryable);
        let serialized = serde_json::to_string(&error).unwrap();
        assert!(!serialized.contains("private-refused-sentinel"));
        assert!(!serialized.contains(&origin));
        let doctor = environment_service_failure(json!({}), error).unwrap();
        assert_eq!(doctor["service"]["status"], "unreachable");
    }

    #[test]
    fn failed_local_handshake_is_not_retried() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            drop(stream);
            listener.set_nonblocking(true).unwrap();
            let until = Instant::now() + Duration::from_millis(200);
            while Instant::now() < until {
                match listener.accept() {
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
                    result => panic!("unexpected second connection: {result:?}"),
                }
                thread::sleep(Duration::from_millis(10));
            }
        });
        let error =
            fetch_json(local_client().unwrap().get(format!("{origin}/healthz"))).unwrap_err();
        assert_eq!(error.code, "SERVICE_UNREACHABLE");
        server.join().unwrap();
    }

    #[test]
    fn local_http_client_does_not_follow_redirects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let redirect_origin = origin.clone();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0; 4096];
            let size = stream.read(&mut request).unwrap();
            assert!(String::from_utf8_lossy(&request[..size]).starts_with("GET /healthz "));
            write!(stream, "HTTP/1.1 302 Found\r\nLocation: {redirect_origin}/other\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let result = fetch_json(local_client().unwrap().get(format!("{origin}/healthz")));
        assert_eq!(result.unwrap_err().code, "SERVICE_PROTOCOL_ERROR");
        server.join().unwrap();
    }
    #[test]
    fn inputs_are_bounded_objects_and_keep_lossless_values() {
        let input = br#"{"sql":"SELECT ?", "params":[{"$int64":"9223372036854775807"}]}"#;
        assert_eq!(
            parse_input(&input[..]).unwrap()["params"][0]["$int64"],
            "9223372036854775807"
        );
        assert_eq!(parse_input(&b"[]"[..]).unwrap_err().code, "INPUT_INVALID");
        assert_eq!(
            parse_input(std::io::repeat(b' ').take(MAX_INPUT_BYTES + 1))
                .unwrap_err()
                .code,
            "INPUT_INVALID"
        );
        assert_eq!(
            parse_input(&b"{ invalid"[..]).unwrap_err().code,
            "INPUT_INVALID"
        );
    }

    #[test]
    fn task_identity_and_terminal_status_are_checked_before_waiting() {
        for id in [
            "",
            "../task",
            "task_a?query=1",
            "task_a/logs",
            "task_a%2fother",
        ] {
            assert_eq!(validate_task_id(id).unwrap_err().code, "INPUT_INVALID");
        }
        let value = json!({"ok":true,"data":{"taskId":"task_a","status":"failed","final":true}});
        assert!(validate_task_response(&value, "task_a", false).is_ok());
        assert!(validate_task_response(&value, "task_b", false).is_err());
        let mut inconsistent = value.clone();
        inconsistent["data"]["final"] = json!(false);
        assert!(validate_task_response(&inconsistent, "task_a", false).is_err());
        inconsistent["data"]["status"] = json!("cancelled");
        assert!(validate_task_response(&inconsistent, "task_a", false).is_err());
    }

    #[test]
    fn wait_timeout_returns_current_task_without_cancelling_or_replaying_submission() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0; 4096];
            let size = stream.read(&mut request).unwrap();
            assert!(
                String::from_utf8_lossy(&request[..size]).starts_with("GET /api/v1/tasks/task_a ")
            );
            let body = json!({"apiVersion":1,"ok":true,"requestId":"req_a","data":{"taskId":"task_a","status":"running","final":false}}).to_string();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        });
        let connection = BusinessConnection {
            client: local_client().unwrap(),
            base_url,
            token: "test-only".into(),
        };
        let result = wait_for_connected_task(&connection, "task_a", Duration::ZERO).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["data"]["status"], "running");
        assert_eq!(result["data"]["final"], false);
        assert_eq!(result["data"]["waitTimedOut"], true);
        server.join().unwrap();
    }

    #[test]
    fn task_wait_is_bounded_and_node_selection_requires_explicit_absolute_path() {
        assert!(Cli::try_parse_from([
            "platform",
            "task",
            "wait",
            "--task",
            "task_a",
            "--timeout",
            "31"
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "platform",
            "task",
            "wait",
            "--task",
            "task_a",
            "--timeout",
            "0",
            "--json"
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "platform", "task", "logs", "--task", "task_a", "--cursor", "512", "--json"
        ])
        .is_ok());
        let (_, result) = node_command(NodeCommand::Set {
            path: "node".into(),
            output: OutputOptions { json: true },
        });
        assert_eq!(result.unwrap_err().code, "INPUT_INVALID");
    }

    #[test]
    fn git_commands_keep_diagnosis_selection_and_clear_separate() {
        for arguments in [
            vec!["platform", "env", "git", "doctor", "--json"],
            vec!["platform", "env", "git", "clear", "--json"],
            vec![
                "platform",
                "env",
                "git",
                "check",
                "--url",
                "https://github.com/git/git.git",
                "--json",
            ],
            vec![
                "platform",
                "env",
                "git",
                "set",
                "--path",
                "/disposable/git",
                "--json",
            ],
        ] {
            assert!(Cli::try_parse_from(arguments).is_ok());
        }
        assert!(Cli::try_parse_from(["platform", "env", "git", "set"]).is_err());
        assert!(Cli::try_parse_from(["platform", "env", "git", "check"]).is_err());
        assert!(Cli::try_parse_from([
            "platform",
            "env",
            "git",
            "check",
            "--url",
            "https://gitee.com/mirrors/git.git",
            "--clone",
        ])
        .is_err());
        let (_, result) = git_command(GitCommand::Set {
            path: "git".into(),
            output: OutputOptions { json: true },
        });
        assert_eq!(result.unwrap_err().code, "INPUT_INVALID");
    }

    #[test]
    fn explicit_project_and_response_identity_cannot_escape_or_change_target() {
        for id in ["", "../prod", "prj_x/launch", "prj_x?other=1", "A"] {
            assert_eq!(
                project_request(id, "GET", "", None).unwrap_err().code,
                "INPUT_INVALID"
            );
        }
        let response =
            json!({"apiVersion":1,"ok":true,"requestId":"req_a","projectId":"prj_a","data":{}});
        assert!(validate_business_response(&response, Some("prj_a"), true).is_ok());
        assert_eq!(
            validate_business_response(&response, Some("prj_b"), true)
                .unwrap_err()
                .code,
            "REQUEST_OUTCOME_UNKNOWN"
        );
        assert!(validate_business_response(&response, Some("prj_a"), false).is_err());
    }
}
