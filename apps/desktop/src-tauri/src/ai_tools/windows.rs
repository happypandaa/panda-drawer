//! Windows tool detection (protocol registrations, conventional install
//! folders, PATH entries without running them) and native actions
//! (`ShellExecuteExW` for links, direct program start for folders, Explorer
//! as the fallback). Handoff rules live in plan.rs.
use super::{plan, plan::Capability, AiTool, ApiError, DetectedTool, OpenResult};
use platform_native::windows::api_paths::conventional_path;
use std::{
    ffi::{OsStr, OsString},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
use windows::{
    core::{w, GUID, PCWSTR, PWSTR},
    Win32::{
        Foundation::{CloseHandle, ERROR_SUCCESS},
        System::{
            Com::{
                CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
                COINIT_DISABLE_OLE1DDE,
            },
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
            Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ},
        },
        UI::{
            Shell::{
                AssocQueryStringW, FOLDERID_LocalAppData, FOLDERID_ProgramFiles,
                FOLDERID_ProgramFilesX86, FOLDERID_UserProgramFiles, SHGetKnownFolderPath,
                ShellExecuteExW, ASSOCF_IS_PROTOCOL, ASSOCF_NOTRUNCATE, ASSOCSTR, ASSOCSTR_APPID,
                ASSOCSTR_EXECUTABLE, ASSOCSTR_FRIENDLYAPPNAME, KF_FLAG_DEFAULT,
                SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
            },
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
};

struct ToolSpec {
    id: &'static str,
    name: &'static str,
    scheme: Option<&'static str>,
    /// Program file name and the install folders (under the per-user and
    /// machine program folders) where installers conventionally place it.
    exe: Option<&'static str>,
    dirs: &'static [&'static str],
    /// Command name checked on PATH. Only its resolved GUI program is launched.
    command: Option<&'static str>,
    capability: Capability,
    chat_scheme: Option<&'static str>,
    wait_for_start: bool,
    launch_url: Option<&'static str>,
}

const fn spec(id: &'static str, name: &'static str, capability: Capability) -> ToolSpec {
    ToolSpec {
        id,
        name,
        scheme: None,
        exe: None,
        dirs: &[],
        command: None,
        capability,
        chat_scheme: None,
        wait_for_start: false,
        launch_url: None,
    }
}

const fn folder_tool(
    id: &'static str,
    name: &'static str,
    scheme: &'static str,
    exe: &'static str,
    dirs: &'static [&'static str],
    command: Option<&'static str>,
) -> ToolSpec {
    ToolSpec {
        scheme: Some(scheme),
        exe: Some(exe),
        dirs,
        command,
        ..spec(id, name, Capability::Folder)
    }
}

const TOOLS: &[ToolSpec] = &[
    // ChatGPT for Windows is a Store/MSIX package; it keeps codex:// links.
    ToolSpec {
        scheme: Some("codex"),
        ..spec("codex", "ChatGPT（Codex）", Capability::Full)
    },
    ToolSpec {
        scheme: Some("claude"),
        ..spec("claude", "Claude 桌面版", Capability::Full)
    },
    ToolSpec {
        scheme: Some("workbuddy"),
        exe: Some("WorkBuddy.exe"),
        dirs: &["WorkBuddy"],
        wait_for_start: true,
        ..spec("workbuddy", "WorkBuddy", Capability::Full)
    },
    // Qoder is resolved by `locate_qoder`; this entry carries its chooser identity.
    spec("qoder", "Qoder", Capability::Folder),
    ToolSpec {
        scheme: Some("cursor"),
        exe: Some("Cursor.exe"),
        dirs: &["cursor", "Cursor"],
        command: Some("cursor"),
        chat_scheme: Some("cursor"),
        ..spec("cursor", "Cursor", Capability::FolderPrefill)
    },
    folder_tool(
        "vscode",
        "VS Code",
        "vscode",
        "Code.exe",
        &["Microsoft VS Code"],
        Some("code"),
    ),
    folder_tool("trae", "Trae", "trae", "Trae.exe", &["Trae"], Some("trae")),
    folder_tool(
        "trae-cn",
        "Trae CN",
        "trae-cn",
        "Trae CN.exe",
        &["Trae CN"],
        Some("trae-cn"),
    ),
    folder_tool(
        "windsurf",
        "Windsurf",
        "windsurf",
        "Windsurf.exe",
        &["Windsurf"],
        Some("windsurf"),
    ),
    folder_tool("kiro", "Kiro", "kiro", "Kiro.exe", &["Kiro"], Some("kiro")),
    folder_tool(
        "codebuddy",
        "CodeBuddy",
        "codebuddy",
        "CodeBuddy.exe",
        &["CodeBuddy"],
        Some("codebuddy"),
    ),
    folder_tool(
        "codebuddy-cn",
        "CodeBuddy CN",
        "codebuddycn",
        "CodeBuddy CN.exe",
        &["CodeBuddy CN"],
        Some("codebuddycn"),
    ),
    folder_tool(
        "lingma",
        "通义灵码",
        "lingma",
        "Lingma.exe",
        &["Lingma"],
        Some("lingma"),
    ),
];

/// Program folders checked for Qoder. Only a program named "...IDE.exe" is
/// treated as the IDE that opens folders; anything else is only started.
const QODER_PROGRAMS: &[(&str, &str)] = &[
    ("Qoder IDE", "Qoder IDE.exe"),
    ("Qoder CN IDE", "Qoder CN IDE.exe"),
    ("Qoder", "Qoder.exe"),
    ("Qoder CN", "Qoder CN.exe"),
];

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

fn from_wide(buffer: &[u16]) -> OsString {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    OsString::from_wide(&buffer[..end])
}

/// Shell association for a URL protocol, including packaged (MSIX) handlers.
fn assoc(scheme: &str, kind: ASSOCSTR) -> Option<OsString> {
    let scheme = wide(OsStr::new(scheme));
    let flags = ASSOCF_IS_PROTOCOL | ASSOCF_NOTRUNCATE;
    let mut length = 0u32;
    let _ = unsafe {
        AssocQueryStringW(
            flags,
            kind,
            PCWSTR(scheme.as_ptr()),
            PCWSTR::null(),
            None,
            &mut length,
        )
    };
    if length == 0 || length > 32_768 {
        return None;
    }
    let mut buffer = vec![0u16; length as usize];
    unsafe {
        AssocQueryStringW(
            flags,
            kind,
            PCWSTR(scheme.as_ptr()),
            PCWSTR::null(),
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut length,
        )
    }
    .ok()
    .ok()?;
    Some(from_wide(&buffer)).filter(|value| !value.is_empty())
}

fn registry_protocol(scheme: &str) -> bool {
    let key = wide(OsStr::new(&format!("Software\\Classes\\{scheme}")));
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
        .into_iter()
        .any(|root: HKEY| {
            let status = unsafe {
                RegGetValueW(
                    root,
                    PCWSTR(key.as_ptr()),
                    w!("URL Protocol"),
                    RRF_RT_REG_SZ,
                    None,
                    None,
                    None,
                )
            };
            status == ERROR_SUCCESS
        })
}

fn packaged(path: &Path) -> bool {
    path.components()
        .any(|part| part.as_os_str().eq_ignore_ascii_case("WindowsApps"))
}

/// A protocol whose handler program was removed is left over from an
/// uninstall and does not count. Packaged handlers cannot be checked by path.
fn protocol_left_over(scheme: &str) -> bool {
    assoc(scheme, ASSOCSTR_EXECUTABLE)
        .map(PathBuf::from)
        .is_some_and(|program| {
            let exe = program
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"));
            program.is_absolute() && exe && !packaged(&program) && !program.exists()
        })
}

fn protocol_registered(scheme: &str) -> bool {
    if protocol_left_over(scheme) {
        return false;
    }
    registry_protocol(scheme)
        || [
            ASSOCSTR_EXECUTABLE,
            ASSOCSTR_FRIENDLYAPPNAME,
            ASSOCSTR_APPID,
        ]
        .into_iter()
        .any(|kind| assoc(scheme, kind).is_some())
}

/// A startable program for a protocol. Packaged apps under WindowsApps cannot
/// be started by path and are reached through their links instead.
fn protocol_program(scheme: &str) -> Option<PathBuf> {
    let path = PathBuf::from(assoc(scheme, ASSOCSTR_EXECUTABLE)?);
    let program = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"));
    (path.is_absolute() && program && !packaged(&path) && path.is_file()).then_some(path)
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    let value = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.ok()?;
    let path = unsafe { value.to_string() }.ok().map(PathBuf::from);
    unsafe { CoTaskMemFree(Some(value.0 as *const _)) };
    path.filter(|path| path.is_absolute())
}

fn program_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = [
        &FOLDERID_UserProgramFiles,
        &FOLDERID_ProgramFiles,
        &FOLDERID_ProgramFilesX86,
        &FOLDERID_LocalAppData,
    ]
    .into_iter()
    .filter_map(known_folder)
    .collect();
    roots.dedup();
    roots
}

fn installed_program(roots: &[PathBuf], dirs: &[&str], exe: &str) -> Option<PathBuf> {
    roots
        .iter()
        .flat_map(|root| dirs.iter().map(move |dir| root.join(dir).join(exe)))
        .find(|path| path.is_file())
}

/// A string value with environment references expanded (REG_EXPAND_SZ is
/// expanded by RegGetValueW when RRF_NOEXPAND is not given).
fn registry_string(root: HKEY, key: &str, value: &str) -> Option<OsString> {
    let key = wide(OsStr::new(key));
    let value = wide(OsStr::new(value));
    // PATH is limited to 32767 characters.
    let mut buffer = vec![0u16; 32_768];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast::<core::ffi::c_void>()),
            Some(&mut size as *mut u32),
        )
    };
    (status == ERROR_SUCCESS)
        .then(|| from_wide(&buffer))
        .filter(|value| !value.is_empty())
}

/// Directories to search for commands. The saved user and machine PATH are
/// read fresh, so a tool installed after the platform started is found
/// without restarting it; the inherited PATH is kept as well.
fn search_path() -> Vec<PathBuf> {
    let saved = [
        registry_string(HKEY_CURRENT_USER, "Environment", "Path"),
        registry_string(
            HKEY_LOCAL_MACHINE,
            "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment",
            "Path",
        ),
        std::env::var_os("PATH"),
    ];
    let mut directories: Vec<PathBuf> = Vec::new();
    for value in saved.into_iter().flatten() {
        for directory in std::env::split_paths(&value).filter(|directory| directory.is_absolute()) {
            if !directories.iter().any(|known| {
                known
                    .as_os_str()
                    .eq_ignore_ascii_case(directory.as_os_str())
            }) {
                directories.push(directory);
            }
        }
    }
    directories
}

/// Distinguish a portable GUI from a same-named stand-alone console CLI.
/// PE32 and PE32+ store Subsystem at the same optional-header offset.
fn gui_executable(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};

    if packaged(path) || !path.is_file() {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut dos = [0u8; 64];
    if file.read_exact(&mut dos).is_err() || &dos[..2] != b"MZ" {
        return false;
    }
    let pe_offset = u32::from_le_bytes(dos[60..64].try_into().unwrap());
    let mut header = [0u8; 24 + 70]; // signature, COFF header and fields through Subsystem
    if file.seek(SeekFrom::Start(pe_offset.into())).is_err()
        || file.read_exact(&mut header).is_err()
    {
        return false;
    }
    let optional_size = u16::from_le_bytes([header[20], header[21]]);
    let characteristics = u16::from_le_bytes([header[22], header[23]]);
    let magic = u16::from_le_bytes([header[24], header[25]]);
    &header[..4] == b"PE\0\0"
        && optional_size >= 70
        && characteristics & 0x2002 == 0x0002 // executable image, excluding DLLs
        && matches!(magic, 0x10b | 0x20b)
        && u16::from_le_bytes([header[92], header[93]]) == 2 // IMAGE_SUBSYSTEM_WINDOWS_GUI
}

#[derive(Debug, PartialEq, Eq)]
enum PathCommand {
    CliOnly,
    Gui(PathBuf),
}

/// Preserve CLI-only evidence while searching later PATH entries for a GUI.
/// Installer shims in bin point back to the product's known executable.
fn path_command(directories: &[PathBuf], name: &str, exe: &str) -> Option<PathCommand> {
    let mut found = None;
    for directory in directories {
        let gui = directory.join(exe);
        if gui_executable(&gui) {
            return Some(PathCommand::Gui(gui));
        }
        for extension in ["exe", "cmd"] {
            let candidate = directory.join(format!("{name}.{extension}"));
            if !candidate.is_file() {
                continue;
            }
            if directory
                .file_name()
                .is_some_and(|folder| folder.eq_ignore_ascii_case("bin"))
            {
                let gui = directory.with_file_name(exe);
                if gui_executable(&gui) {
                    return Some(PathCommand::Gui(gui));
                }
            }
            found = Some(PathCommand::CliOnly);
            break;
        }
    }
    found
}

fn same_file_object(first: &Path, second: &Path) -> bool {
    use platform_native::windows::files::{following_metadata_file, identity};
    let (Ok(first), Ok(second)) = (
        following_metadata_file(first),
        following_metadata_file(second),
    ) else {
        return false;
    };
    match (identity(&first), identity(&second)) {
        (Ok(first), Ok(second)) => first.same_object(&second),
        _ => false,
    }
}

/// Match the EXE and its installation directory by filesystem identity. This
/// accepts case aliases on NTFS while distinguishing case-sensitive directories
/// and separate installs that hard-link the same binary but load different resources.
fn handler_matches(application: Option<&Path>, handler: Option<&Path>) -> bool {
    let (Some(selected), Some(owner)) = (application, handler) else {
        return false;
    };
    selected
        .parent()
        .zip(owner.parent())
        .is_some_and(|(left, right)| {
            same_file_object(left, right) && same_file_object(selected, owner)
        })
}

fn dispatch_to_selected_program(
    scheme: &str,
    application: Option<&Path>,
    handler_for: impl Fn(&str) -> Option<PathBuf>,
) -> bool {
    match scheme {
        // The default can change while the newly opened folder window settles.
        "qoder" | "qoder-cn" | "cursor" => {
            handler_matches(application, handler_for(scheme).as_deref())
        }
        _ => true, // Full links can also belong to packaged apps without an EXE path.
    }
}

fn locate_spec(
    spec: &ToolSpec,
    roots: &[PathBuf],
    path: &[PathBuf],
    registered: bool,
    associated: Option<PathBuf>,
) -> DetectedTool {
    let mut tool = DetectedTool::new(spec.id, spec.name, spec.capability);
    if let Some(scheme) = spec.scheme.filter(|_| registered) {
        tool.link_ready = true;
        tool.info.installed = true;
        tool.info.detected_by.push(format!("protocol:{scheme}"));
    }
    let installed = spec
        .exe
        .and_then(|exe| installed_program(roots, spec.dirs, exe))
        .inspect(|_| tool.info.detected_by.push("program".into()));
    let command = spec.command.zip(spec.exe).and_then(|(name, exe)| {
        path_command(path, name, exe)
            .inspect(|_| tool.info.detected_by.push(format!("command:{name}")))
    });
    if command.is_some() {
        tool.info.installed = true;
    }
    // For editors that send a separate chat link, an unrelated default
    // handler is not the editor's GUI and must not receive the folder either.
    let associated_app = associated.as_ref().filter(|program| {
        spec.chat_scheme.is_none()
            || program
                .file_name()
                .zip(spec.exe)
                .is_some_and(|(name, exe)| name.eq_ignore_ascii_case(exe))
    });
    tool.application = installed
        .or_else(|| associated_app.cloned())
        .or(match command {
            Some(PathCommand::Gui(program)) => Some(program),
            _ => None,
        });
    if tool.application.is_some() {
        tool.info.installed = true;
    }
    tool.process = tool
        .application
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned());
    if tool.info.installed
        && matches!(
            tool.capability,
            Capability::Folder | Capability::FolderPrefill
        )
    {
        tool.chat_scheme = spec
            .chat_scheme
            .filter(|_| handler_matches(tool.application.as_deref(), associated.as_deref()));
        tool.capability = match (&tool.application, tool.chat_scheme) {
            (None, _) => Capability::Manual,
            (_, None) => Capability::Folder,
            (_, Some(_)) => Capability::FolderPrefill,
        };
    }
    tool.launch_url = spec.launch_url.filter(|_| tool.link_ready);
    tool.wait_for_start = spec.wait_for_start;
    tool.finish()
}

fn qoder_kind(program: &Path) -> plan::QoderKind {
    let ide = program
        .file_stem()
        .and_then(OsStr::to_str)
        .is_some_and(|stem| stem.to_ascii_lowercase().ends_with(" ide"));
    if ide {
        plan::QoderKind::Ide
    } else {
        plan::QoderKind::Desktop
    }
}

fn locate_qoder(roots: &[PathBuf], handler_for: impl Fn(&str) -> Option<PathBuf>) -> DetectedTool {
    let mut candidates = Vec::new();
    for scheme in ["qoder", "qoder-cn"] {
        if let Some(program) = handler_for(scheme) {
            candidates.push((qoder_kind(&program), (program, Some(scheme))));
        }
    }
    for (dir, exe) in QODER_PROGRAMS {
        if let Some(program) = installed_program(roots, &[dir], exe) {
            // Protocol owners are already in candidates ahead of this entry.
            candidates.push((qoder_kind(&program), (program, None)));
        }
    }
    let Some((kind, (program, scheme))) = plan::select_qoder(candidates) else {
        return DetectedTool::new("qoder", "Qoder", Capability::Folder).finish();
    };
    let capability = match kind {
        plan::QoderKind::Ide if scheme.is_some() => Capability::FolderPrefill,
        plan::QoderKind::Ide => Capability::Folder,
        plan::QoderKind::Desktop => Capability::Launch,
    };
    let mut tool = DetectedTool::new("qoder", "Qoder", capability);
    tool.info.installed = true;
    tool.info.detected_by.push("program".into());
    // A desktop program found by its own protocol gets an unconfirmed attempt.
    tool.chat_scheme = scheme;
    tool.process = Some(program.to_string_lossy().into_owned());
    tool.application = Some(program);
    tool.finish()
}

fn locate_tool(spec: &ToolSpec, roots: &[PathBuf], path: &[PathBuf]) -> DetectedTool {
    if spec.id == "qoder" {
        locate_qoder(roots, protocol_program)
    } else {
        locate_spec(
            spec,
            roots,
            path,
            spec.scheme.is_some_and(protocol_registered),
            spec.scheme.and_then(protocol_program),
        )
    }
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tauri::async_runtime::spawn_blocking(work).await.ok()
}

pub(super) async fn locate(id: &str) -> Result<DetectedTool, ApiError> {
    if id == "other" {
        return Ok(DetectedTool::other());
    }
    let spec = TOOLS
        .iter()
        .position(|spec| spec.id == id)
        .ok_or_else(|| ApiError::new("AI_TOOL_UNKNOWN", "无法识别这个 AI 工具，请重新选择。"))?;
    blocking(move || locate_tool(&TOOLS[spec], &program_roots(), &search_path()))
        .await
        .ok_or_else(|| ApiError::new("AI_TOOL_DETECTION_FAILED", "暂时无法检测 AI 工具，请重试。"))
}

pub(super) async fn detect() -> Result<Vec<AiTool>, ApiError> {
    let mut tools = blocking(|| {
        let roots = program_roots();
        let path = search_path();
        TOOLS
            .iter()
            .map(|spec| locate_tool(spec, &roots, &path).info)
            .collect::<Vec<_>>()
    })
    .await
    .ok_or_else(|| ApiError::new("AI_TOOL_DETECTION_FAILED", "暂时无法检测 AI 工具，请重试。"))?;
    tools.sort_by_key(|tool| !tool.installed);
    Ok(tools)
}

/// One literal target for ShellExecuteExW: no parameters, verb "open", on a
/// dedicated single-threaded apartment like the existing browser opener.
fn shell_open(target: Vec<u16>) -> bool {
    std::thread::Builder::new()
        .name("ai-handoff-open".into())
        .spawn(move || {
            if unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) }
                .is_err()
            {
                return false;
            }
            struct Apartment;
            impl Drop for Apartment {
                fn drop(&mut self) {
                    unsafe { CoUninitialize() };
                }
            }
            let _apartment = Apartment;
            let mut execute = SHELLEXECUTEINFOW {
                cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
                lpVerb: w!("open"),
                lpFile: PCWSTR(target.as_ptr()),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            unsafe { ShellExecuteExW(&mut execute) }.is_ok()
        })
        .ok()
        .and_then(|thread| thread.join().ok())
        .unwrap_or(false)
}

fn url_target(url: &str) -> Option<Vec<u16>> {
    (!url.contains('\0') && url.chars().count() <= plan::MAX_URL_CHARS)
        .then(|| wide(OsStr::new(url)))
}

fn start_program(program: &Path, folder: Option<&Path>) -> bool {
    let mut command = Command::new(program);
    if let Some(folder) = folder {
        command.arg(folder);
    }
    if let Some(directory) = program.parent() {
        command.current_dir(directory);
    }
    // The tool keeps running on its own; it is never awaited or terminated.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

fn process_running(program: &Path) -> Option<bool> {
    let name = program.file_name()?.to_str()?.to_ascii_lowercase();
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.ok()?;
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = false;
    let mut next = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
    while next {
        if from_wide(&entry.szExeFile)
            .to_string_lossy()
            .to_ascii_lowercase()
            == name
        {
            found = true;
            break;
        }
        next = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
    }
    let _ = unsafe { CloseHandle(snapshot) };
    Some(found)
}

struct Win;

impl plan::Native for Win {
    async fn dispatch_url(&self, application: Option<&Path>, url: &str) -> bool {
        let Some(target) = url_target(url) else {
            return false;
        };
        let Some((scheme, _)) = url.split_once(':') else {
            return false;
        };
        let scheme = scheme.to_owned();
        let application = application.map(Path::to_path_buf);
        blocking(move || {
            dispatch_to_selected_program(&scheme, application.as_deref(), protocol_program)
                && shell_open(target)
        })
        .await
        .unwrap_or(false)
    }
    async fn launch(&self, application: &Path, folder: Option<&Path>) -> bool {
        let application = application.to_path_buf();
        let folder = folder.map(Path::to_path_buf);
        blocking(move || start_program(&application, folder.as_deref()))
            .await
            .unwrap_or(false)
    }
    async fn reveal(&self, folder: &Path) -> bool {
        crate::desktop_shell::reveal(folder.to_path_buf(), false).is_ok()
    }
    async fn running(&self, tool: &DetectedTool) -> Option<bool> {
        let program = PathBuf::from(tool.process.as_ref()?);
        blocking(move || process_running(&program)).await.flatten()
    }
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

pub(super) async fn open(tool: &DetectedTool, root: &Path, text: &str) -> OpenResult {
    // Links and program arguments use the ordinary drive or UNC spelling.
    let display = conventional_path(root).unwrap_or_else(|_| root.to_path_buf());
    plan::open(&Win, tool, &display, text, super::paste_note()).await
}

#[cfg(test)]
#[path = "windows_detection_tests.rs"]
mod detection_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chooser_tool_has_one_windows_entry() {
        let mut ids: Vec<_> = TOOLS.iter().map(|spec| spec.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), TOOLS.len());
        for id in ["codex", "claude", "workbuddy", "qoder", "trae", "trae-cn"] {
            assert!(ids.contains(&id), "{id}");
        }
    }

    #[test]
    fn only_programs_named_ide_receive_qoder_folders() {
        assert_eq!(
            qoder_kind(Path::new(r"C:\P\Qoder IDE\Qoder IDE.exe")),
            plan::QoderKind::Ide
        );
        assert_eq!(
            qoder_kind(Path::new(r"C:\P\Qoder CN IDE\Qoder CN IDE.EXE")),
            plan::QoderKind::Ide
        );
        assert_eq!(
            qoder_kind(Path::new(r"C:\P\Qoder\Qoder.exe")),
            plan::QoderKind::Desktop
        );
        assert_eq!(
            qoder_kind(Path::new(r"C:\P\Qoder CN\Qoder CN.exe")),
            plan::QoderKind::Desktop
        );
    }

    #[test]
    fn link_targets_are_one_terminated_utf16_string() {
        let url = "codex://new?path=C%3A%5C%E4%B8%AD%20x&prompt=a%2Bb";
        let target = url_target(url).unwrap();
        assert_eq!(target.last(), Some(&0));
        assert_eq!(
            String::from_utf16(&target[..target.len() - 1]).unwrap(),
            url
        );
        assert!(url_target("codex://new\0calc").is_none());
        assert!(url_target(&"a".repeat(plan::MAX_URL_CHARS + 1)).is_none());
    }

    #[test]
    fn search_path_keeps_every_inherited_directory_once() {
        let all = search_path();
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        for directory in
            std::env::split_paths(&inherited).filter(|directory| directory.is_absolute())
        {
            let matches = all
                .iter()
                .filter(|known| {
                    known
                        .as_os_str()
                        .eq_ignore_ascii_case(directory.as_os_str())
                })
                .count();
            assert_eq!(matches, 1, "{}", directory.display());
        }
    }

    #[test]
    fn unknown_protocols_are_not_reported_as_registered() {
        assert!(!protocol_registered(
            "panda-drawer-disposable-unregistered-scheme"
        ));
        assert!(protocol_program("panda-drawer-disposable-unregistered-scheme").is_none());
    }

    #[test]
    fn an_absent_program_is_not_running() {
        assert_eq!(
            process_running(Path::new(
                r"C:\disposable\panda-drawer-not-running-fixture.exe"
            )),
            Some(false)
        );
    }
}
