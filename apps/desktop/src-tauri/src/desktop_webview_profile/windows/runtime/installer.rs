//! One bounded, current-user attempt using Microsoft's signed WebView2 setup.
//! No application data or WebView is initialized by this module.
use crate::ui_locale::text;
use platform_native::{filesystem as native_fs, windows::files};
use std::{
    ffi::OsStr,
    fs::{File, OpenOptions},
    io::{self, Write},
    mem::size_of,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::Duration,
};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{HANDLE, HWND},
        Security::{
            Cryptography::{
                szOID_ORGANIZATION_NAME, CertGetNameStringW, CERT_CONTEXT, CERT_NAME_ATTR_TYPE,
                CERT_NAME_SIMPLE_DISPLAY_TYPE,
            },
            WinTrust::{
                WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
                WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
                WINTRUST_FILE_INFO, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT,
                WTD_REVOKE_WHOLECHAIN, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
            },
        },
        Storage::FileSystem::{
            FileDispositionInfo, SetFileInformationByHandle, DELETE, FILE_DISPOSITION_INFO,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        },
        System::Com::CoCreateGuid,
    },
};

const BOOTSTRAPPER_URL: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";
const INSTALLER_NAME: &str = "MicrosoftEdgeWebview2Setup.exe";
const MAX_DOWNLOAD_BYTES: usize = 8 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub(super) async fn install() -> Result<(), String> {
    let override_folder = std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER");
    require_system_runtime(override_folder.as_deref())?;
    let mut directory = InstallerDirectory::create()?;
    let result = install_in(&mut directory).await;
    match (result, directory.close()) {
        (result, Ok(())) => result,
        (Ok(()), Err(cleanup)) => Err(cleanup),
        (Err(error), Err(cleanup)) => Err(format!("{error}; {cleanup}")),
    }
}

fn require_system_runtime(override_folder: Option<&OsStr>) -> Result<(), String> {
    if override_folder.is_some_and(|folder| !folder.is_empty()) {
        return Err(text(
            "已设置 WEBVIEW2_BROWSER_EXECUTABLE_FOLDER，自动安装不会替换明确指定的 WebView2 配置。",
        )
        .into());
    }
    Ok(())
}

async fn install_in(directory: &mut InstallerDirectory) -> Result<(), String> {
    let (path, verified) = prepare_installer(directory).await?;
    let mut command = tokio::process::Command::new(&path);
    // /install displays Microsoft's own progress. Do not request runas or use
    // a shell: the child inherits the current user and ordinary token.
    command
        .arg("/install")
        .current_dir(&directory.path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000); // CREATE_NO_WINDOW suppresses console only.
    let result =
        installation_outcome(crate::native_process::status(command, INSTALL_TIMEOUT).await);
    drop(verified);
    result
}

async fn prepare_installer(directory: &mut InstallerDirectory) -> Result<(PathBuf, File), String> {
    let bytes = download().await?;
    let path = directory.path.join(INSTALLER_NAME);
    let mut file = native_fs::create_new_private_file(&path, true)
        .map_err(|error| format!("{}: {error}", text("无法创建 WebView2 安装程序")))?;
    let identity = files::identity(&file)
        .map_err(|error| format!("{}: {error}", text("无法核实 WebView2 安装程序")))?;
    directory.payload_identity = Some(identity.clone());
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("{}: {error}", text("无法保存 WebView2 安装程序")))?;
    drop(file);
    let verification_path = path.clone();
    // Keep the verified file locked against replacement until its child has
    // exited. The native verifier and subsequent executable use this same file.
    let verified = tauri::async_runtime::spawn_blocking(move || {
        locked_microsoft_installer(&verification_path, &identity)
    })
    .await
    .map_err(|error| format!("{}: {error}", text("WebView2 签名验证未完成")))??;
    Ok((path, verified))
}

fn allowed_download_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && matches!(
            url.host_str(),
            Some("go.microsoft.com" | "msedge.sf.dl.delivery.mp.microsoft.com")
        )
}

async fn download() -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(15))
        .timeout(DOWNLOAD_TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS || !allowed_download_url(attempt.url()) {
                attempt.error(text("WebView2 安装程序重定向超出固定的微软下载地址范围"))
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|error| format!("{}: {error}", text("无法准备 WebView2 下载")))?;
    let mut response = client
        .get(BOOTSTRAPPER_URL)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|error| format!("{}: {error}", text("无法下载 WebView2 安装程序")))?;
    if response.status() != reqwest::StatusCode::OK || !allowed_download_url(response.url()) {
        return Err(format!(
            "{} HTTP {}",
            text("WebView2 安装程序下载返回"),
            response.status()
        ));
    }
    if response
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .is_some_and(|value| value.as_bytes() != b"identity")
    {
        return Err(text("WebView2 安装程序下载使用了不支持的内容编码").into());
    }
    require_download_length(response.content_length())?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("{}: {error}", text("WebView2 安装程序下载中断")))?
    {
        append_download(&mut bytes, &chunk)?;
    }
    if !bytes.starts_with(b"MZ") {
        return Err(text("下载的 WebView2 文件不是 Windows 可执行文件").into());
    }
    Ok(bytes)
}

fn require_download_length(length: Option<u64>) -> Result<(), String> {
    if length.is_some_and(|length| length == 0 || length > MAX_DOWNLOAD_BYTES as u64) {
        return Err(text("WebView2 安装程序下载长度超出允许范围").into());
    }
    Ok(())
}

fn append_download(bytes: &mut Vec<u8>, chunk: &[u8]) -> Result<(), String> {
    if chunk.len() > MAX_DOWNLOAD_BYTES.saturating_sub(bytes.len()) {
        return Err(text("WebView2 安装程序下载超过大小限制").into());
    }
    bytes.extend_from_slice(chunk);
    Ok(())
}

fn installation_outcome(result: io::Result<ExitStatus>) -> Result<(), String> {
    let status = result.map_err(|error| {
        if error.kind() == io::ErrorKind::TimedOut {
            format!(
                "{}: {}s; {error}",
                text("WebView2 安装超时"),
                INSTALL_TIMEOUT.as_secs()
            )
        } else {
            format!("{}: {error}", text("无法运行 WebView2 安装程序"))
        }
    })?;
    if status.success() {
        return Ok(());
    }
    match status.code() {
        Some(code) => Err(format!(
            "{} {code} (0x{:08X})",
            text("WebView2 安装程序返回退出码"),
            code as u32
        )),
        None => Err(text("WebView2 安装程序未返回退出码").into()),
    }
}

fn locked_microsoft_installer(path: &Path, expected: &files::FileIdentity) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .map_err(|error| format!("{}: {error}", text("无法锁定 WebView2 安装程序")))?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    let actual = files::identity(&file).map_err(|error| error.to_string())?;
    if native_fs::is_reparse(&metadata)
        || actual.directory
        || actual.links != 1
        || !expected.same_object(&actual)
    {
        return Err(text("WebView2 安装程序在签名验证前发生变化").into());
    }
    verify_microsoft_signature(path, &file)?;
    Ok(file)
}

fn verify_microsoft_signature(path: &Path, file: &File) -> Result<(), String> {
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut subject = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(name.as_ptr()),
        hFile: HANDLE(file.as_raw_handle()),
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let mut trust = WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 {
            pFile: &mut subject,
        },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT,
        ..Default::default()
    };
    // WinVerifyTrust returns a LONG, not HRESULT; only exact zero is trusted.
    let status = unsafe {
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut trust as *mut WINTRUST_DATA).cast(),
        )
    };
    let result = if status == 0 {
        // These pointers belong to the verification state and remain valid
        // until the mandatory WTD_STATEACTION_CLOSE below.
        unsafe { verified_publisher(&trust) }
    } else {
        Err(format!(
            "{} (0x{:08X})",
            text("WebView2 安装程序的 Authenticode 签名验证失败"),
            status as u32
        ))
    };
    trust.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe {
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut trust as *mut WINTRUST_DATA).cast(),
        )
    };
    result
}

unsafe fn verified_publisher(trust: &WINTRUST_DATA) -> Result<(), String> {
    let provider = unsafe { WTHelperProvDataFromStateData(trust.hWVTStateData) };
    if provider.is_null() {
        return Err(text("WebView2 安装程序签名缺少可信验证提供程序").into());
    }
    let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, false, 0) };
    if signer.is_null() || unsafe { (*signer).csCertChain == 0 || (*signer).pasCertChain.is_null() }
    {
        return Err(text("WebView2 安装程序签名缺少可信签名者").into());
    }
    let certificate = unsafe { (*(*signer).pasCertChain).pCert };
    if certificate.is_null() {
        return Err(text("WebView2 安装程序签名缺少签名证书").into());
    }
    let display = unsafe { certificate_name(certificate, CERT_NAME_SIMPLE_DISPLAY_TYPE, None) }?;
    let organization = unsafe {
        certificate_name(
            certificate,
            CERT_NAME_ATTR_TYPE,
            Some(szOID_ORGANIZATION_NAME.as_ptr().cast()),
        )
    }?;
    if display != "Microsoft Corporation" || organization != "Microsoft Corporation" {
        return Err(text("WebView2 安装程序未由 Microsoft Corporation 签名").into());
    }
    Ok(())
}

unsafe fn certificate_name(
    certificate: *const CERT_CONTEXT,
    kind: u32,
    attribute: Option<*const std::ffi::c_void>,
) -> Result<String, String> {
    let length = unsafe { CertGetNameStringW(certificate, kind, 0, attribute, None) };
    if !(2..=1024).contains(&length) {
        return Err(text("无法读取 WebView2 安装程序的发布者证书").into());
    }
    let mut name = vec![0; length as usize];
    if unsafe { CertGetNameStringW(certificate, kind, 0, attribute, Some(&mut name)) } != length {
        return Err(text("无法读取 WebView2 安装程序的发布者证书").into());
    }
    String::from_utf16(&name[..name.len() - 1])
        .map_err(|_| text("WebView2 安装程序的发布者证书无效").into())
}

struct InstallerDirectory {
    path: PathBuf,
    pin: Option<files::PinnedDirectory>,
    identity: files::FileIdentity,
    payload_identity: Option<files::FileIdentity>,
    closed: bool,
}

impl InstallerDirectory {
    fn create() -> Result<Self, String> {
        let parent = std::env::temp_dir()
            .canonicalize()
            .map_err(|error| format!("{}: {error}", text("无法找到 Windows 临时目录")))?;
        let id = unsafe { CoCreateGuid() }
            .map_err(|error| format!("{}: {error}", text("无法生成 WebView2 安装临时目录名称")))?;
        let path = parent.join(format!("panda-webview2-{id:?}"));
        native_fs::create_private_directory(&path).map_err(|error| {
            format!(
                "{}: {error}",
                text("无法创建 WebView2 安装程序的私有临时目录")
            )
        })?;
        let pin = files::pin_directory(&path)
            .map_err(|error| format!("{}: {error}", text("无法固定 WebView2 安装临时目录")))?;
        let identity = files::identity(&pin.file).map_err(|error| error.to_string())?;
        Ok(Self {
            path,
            pin: Some(pin),
            identity,
            payload_identity: None,
            closed: false,
        })
    }

    fn close(&mut self) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        if let Some(identity) = self.payload_identity.as_ref() {
            files::remove_file(&self.path.join(INSTALLER_NAME), identity)
                .map_err(|error| format!("{}: {error}", text("无法清理 WebView2 临时安装程序")))?;
            self.payload_identity = None;
        }
        // Root and ancestor pins protect file cleanup. Release the root pin
        // only before reopening this exact directory with deletion access.
        drop(self.pin.take());
        let file = OpenOptions::new()
            .access_mode((DELETE | FILE_READ_ATTRIBUTES).0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
            .open(&self.path)
            .map_err(|error| format!("{}: {error}", text("无法打开 WebView2 临时目录进行清理")))?;
        let actual = files::identity(&file).map_err(|error| error.to_string())?;
        if native_fs::is_reparse(&file.metadata().map_err(|error| error.to_string())?)
            || !actual.directory
            || !self.identity.same_object(&actual)
        {
            return Err(text("WebView2 临时目录在清理前发生变化").into());
        }
        // Handle-bound, empty-directory deletion only; never recursively walk
        // the system temporary directory or adopt unrecognized child files.
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        unsafe {
            SetFileInformationByHandle(
                HANDLE(file.as_raw_handle()),
                FileDispositionInfo,
                (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        }
        .map_err(|error| {
            format!(
                "{}: {error}",
                text("无法清理 WebView2 安装程序的空临时目录")
            )
        })?;
        self.closed = true;
        Ok(())
    }
}

impl Drop for InstallerDirectory {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::process::ExitStatusExt;

    #[test]
    fn download_route_rejects_insecure_or_unrelated_redirects() {
        for value in [
            BOOTSTRAPPER_URL,
            "https://msedge.sf.dl.delivery.mp.microsoft.com/installer.exe",
        ] {
            assert!(allowed_download_url(&reqwest::Url::parse(value).unwrap()));
        }
        for value in [
            "http://go.microsoft.com/fwlink/",
            "https://go.microsoft.com:444/",
            "https://go.microsoft.com.evil.test/",
            "https://user@go.microsoft.com/",
            "https://example.com/MicrosoftEdgeWebview2Setup.exe",
        ] {
            assert!(
                !allowed_download_url(&reqwest::Url::parse(value).unwrap()),
                "{value}"
            );
        }
    }

    #[test]
    fn download_limits_apply_to_both_headers_and_streamed_bytes() {
        assert!(require_download_length(None).is_ok());
        assert!(require_download_length(Some(MAX_DOWNLOAD_BYTES as u64)).is_ok());
        assert!(require_download_length(Some(0)).is_err());
        assert!(require_download_length(Some(MAX_DOWNLOAD_BYTES as u64 + 1)).is_err());
        let mut bytes = vec![0; MAX_DOWNLOAD_BYTES - 1];
        append_download(&mut bytes, &[1]).unwrap();
        assert!(append_download(&mut bytes, &[2]).is_err());
        assert_eq!(bytes.len(), MAX_DOWNLOAD_BYTES);
        assert_eq!(*bytes.last().unwrap(), 1);
    }

    #[test]
    fn installer_diagnostics_preserve_exit_codes_and_timeout() {
        assert!(installation_outcome(Ok(ExitStatus::from_raw(0))).is_ok());
        let error = installation_outcome(Ok(ExitStatus::from_raw(0x8007_04c7))).unwrap_err();
        assert!(error.contains("0x800704C7"));
        assert!(installation_outcome(Ok(ExitStatus::from_raw(3010)))
            .unwrap_err()
            .contains("3010"));
        let error = installation_outcome(Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "native child reaped",
        )))
        .unwrap_err();
        assert!(error.contains("600") && error.contains("native child reaped"));
    }

    #[test]
    fn explicit_webview_folder_is_not_repaired_by_installing_an_unrelated_runtime() {
        assert!(require_system_runtime(None).is_ok());
        assert!(require_system_runtime(Some(OsStr::new(""))).is_ok());
        assert!(
            require_system_runtime(Some(OsStr::new("C:\\explicit-webview")))
                .unwrap_err()
                .contains("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER")
        );
    }

    #[test]
    fn unsigned_download_is_rejected_and_private_artifacts_are_removed() {
        let mut directory = InstallerDirectory::create().unwrap();
        let directory_path = directory.path.clone();
        let path = directory.path.join(INSTALLER_NAME);
        let mut file = native_fs::create_new_private_file(&path, true).unwrap();
        file.write_all(b"MZ artificial unsigned installer fixture")
            .unwrap();
        let identity = files::identity(&file).unwrap();
        directory.payload_identity = Some(identity.clone());
        drop(file);
        assert!(native_fs::directory_is_private(&directory.path).unwrap());
        assert!(locked_microsoft_installer(&path, &identity)
            .unwrap_err()
            .contains("Authenticode"));
        directory.close().unwrap();
        assert!(!directory_path.exists());
    }

    #[tokio::test]
    #[ignore = "Requires Microsoft's download service and Windows certificate verification; never executes the installer"]
    async fn official_bootstrapper_download_has_a_trusted_microsoft_signature_without_execution() {
        let mut directory = InstallerDirectory::create().unwrap();
        let directory_path = directory.path.clone();
        // This is the exact production download/save/WinTrust preparation.
        // Only install_in builds or starts a command; this test never calls it.
        let (path, verified) = prepare_installer(&mut directory).await.unwrap();
        assert_eq!(path.file_name().unwrap(), INSTALLER_NAME);
        assert!((2..=MAX_DOWNLOAD_BYTES as u64).contains(&verified.metadata().unwrap().len()));
        drop(verified);
        directory.close().unwrap();
        assert!(!directory_path.exists());
    }
}
