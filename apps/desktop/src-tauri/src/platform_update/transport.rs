//! The updater reads release metadata and packages only from compiled-in channels.
//! Ready files are retained under the updater's own private application-data directory.

use base64::{engine::general_purpose::STANDARD, Engine};
use minisign_verify::{PublicKey, Signature};
use platform_native::{directory, filesystem as native_fs};
use reqwest::{redirect, Client, Response, StatusCode, Url};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, File},
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::Builder;
use unicode_normalization::UnicodeNormalization;

const MAX_DOCUMENT_BYTES: usize = 64 * 1024;
const MAX_NOTES_BYTES: usize = 16 * 1024;
const MAX_SIGNATURE_BYTES: usize = 8 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 1024 * 1024 * 1024;
const READY_FILE: &str = "ready.json";
const PACKAGE_PREFIX: &str = "package-";
const PACKAGE_SUFFIX: &str = ".stage";
const META_PREFIX: &str = "record-";
const META_SUFFIX: &str = ".stage";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    Network,
    Integrity,
    Space,
    Other,
}

#[derive(Debug)]
pub(crate) struct UpdateError {
    pub kind: ErrorKind,
    pub(super) reason: &'static str,
    pub(super) http_status: Option<u16>,
    message: &'static str,
}

impl UpdateError {
    fn new(kind: ErrorKind, reason: &'static str, message: &'static str) -> Self {
        Self {
            kind,
            reason,
            http_status: None,
            message,
        }
    }
}

impl fmt::Display for UpdateError {
    fn fmt(&self, format: &mut fmt::Formatter<'_>) -> fmt::Result {
        format.write_str(self.message)
    }
}

impl std::error::Error for UpdateError {}

fn integrity() -> UpdateError {
    UpdateError::new(
        ErrorKind::Integrity,
        "integrity",
        "更新文件校验不通过，已丢弃",
    )
}

fn unavailable(reason: &'static str) -> UpdateError {
    UpdateError::new(ErrorKind::Network, reason, "无法连接更新来源")
}

fn invalid_configuration() -> UpdateError {
    UpdateError::new(ErrorKind::Other, "configuration", "更新配置无效")
}

fn cache_error(error: io::Error) -> UpdateError {
    if matches!(
        error.kind(),
        io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded
    ) {
        UpdateError::new(ErrorKind::Space, "diskSpace", "磁盘空间不足，无法下载更新")
    } else {
        UpdateError::new(ErrorKind::Other, "cacheIo", "无法读写更新暂存目录")
    }
}

fn request_error(error: reqwest::Error) -> UpdateError {
    if error.is_redirect() || error.is_builder() {
        let mut failure = integrity();
        failure.reason = "requestValidation";
        return failure;
    }
    let reason = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connection"
    } else if error.is_body() || error.is_decode() {
        "responseBody"
    } else {
        "request"
    };
    unavailable(reason)
}

#[derive(Clone)]
pub(crate) struct Source {
    name: String,
    manifest: Url,
    artifact_base: Url,
    fixed_release: bool,
}

#[derive(Deserialize)]
struct Registry {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    updates: RegisteredUpdates,
    routes: BTreeMap<String, RegisteredRoute>,
}

#[derive(Deserialize)]
struct RegisteredUpdates {
    sources: BTreeMap<String, RegisteredSource>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisteredSource {
    manifest_url: String,
    artifact_base_url: String,
}

#[derive(Deserialize)]
struct RegisteredRoute {
    updates: String,
}

pub(crate) fn configured_sources(route: &str, fallback: bool) -> Result<Vec<Source>, UpdateError> {
    let registry: Registry = serde_json::from_str(include_str!(
        "../../../../../distribution/download-sources.json"
    ))
    .map_err(|_| invalid_configuration())?;
    if registry.schema_version != 1 {
        return Err(invalid_configuration());
    }
    let backup = match route {
        "cn" => "global",
        "global" => "cn",
        _ => return Err(invalid_configuration()),
    };
    let mut routes = vec![route];
    if fallback {
        routes.push(backup);
    }
    let mut sources = Vec::<Source>::new();
    for route in routes {
        let name = &registry
            .routes
            .get(route)
            .ok_or_else(invalid_configuration)?
            .updates;
        if sources.iter().any(|source| &source.name == name) {
            continue;
        }
        let value = registry
            .updates
            .sources
            .get(name)
            .ok_or_else(invalid_configuration)?;
        sources.push(Source {
            name: name.clone(),
            manifest: registered_url(&value.manifest_url)?,
            artifact_base: registered_url(&value.artifact_base_url)?,
            fixed_release: false,
        });
    }
    Ok(sources)
}

fn registered_url(value: &str) -> Result<Url, UpdateError> {
    let parsed = Url::parse(value).map_err(|_| invalid_configuration())?;
    if parsed.scheme() != "https"
        || !clean_url(&parsed)
        || parsed.query().is_some()
        || parsed.as_str() != value
        || parsed.path().ends_with('/')
    {
        return Err(invalid_configuration());
    }
    Ok(parsed)
}

fn clean_url(url: &Url) -> bool {
    url.username().is_empty() && url.password().is_none() && url.fragment().is_none()
}

impl Source {
    #[cfg(any(feature = "local-update-test", test))]
    pub(crate) fn loopback(port: u16) -> Self {
        let base = format!("http://127.0.0.1:{port}");
        Self {
            name: String::from("local-test"),
            manifest: Url::parse(&format!("{base}/latest.json")).expect("literal loopback URL"),
            artifact_base: Url::parse(&base).expect("literal loopback URL"),
            fixed_release: false,
        }
    }

    #[cfg(any(feature = "local-update-test", test))]
    pub(crate) fn github_test(tag: &str) -> Result<Self, UpdateError> {
        let (date, time) = tag
            .strip_prefix("auto-update-test-")
            .and_then(|rest| rest.split_once('-'))
            .ok_or_else(invalid_configuration)?;
        if date.len() != 8
            || time.len() != 6
            || !date
                .bytes()
                .chain(time.bytes())
                .all(|byte| byte.is_ascii_digit())
        {
            return Err(invalid_configuration());
        }
        let base = format!("https://github.com/happypandaa/panda-drawer/releases/download/{tag}");
        Ok(Self {
            name: tag.to_owned(),
            manifest: registered_url(&format!("{base}/latest.json"))?,
            artifact_base: registered_url(&base)?,
            fixed_release: true,
        })
    }

    fn artifact_url(&self, version: &str, filename: &str) -> Url {
        let mut url = self.artifact_base.clone();
        {
            let mut segments = url.path_segments_mut().expect("fixed base URL");
            segments.pop_if_empty();
            if !self.fixed_release {
                segments.push(&format!("v{version}"));
            }
            segments.push(filename);
        }
        url
    }

    fn allows_redirect(&self, previous: &[Url], url: &Url) -> bool {
        if url.scheme() != "https" || !clean_url(url) || previous.len() >= 4 {
            return false;
        }
        let Some(initial) = previous.first() else {
            return false;
        };
        // GitHub's latest alias redirects to a tagged release before its
        // signed HTTPS asset URL. A fixed test tag may go only to the asset host.
        if self.name == "github" || self.fixed_release {
            let tagged_prefix = format!("{}/", self.artifact_base.path());
            if url.origin() == self.manifest.origin() {
                return !self.fixed_release
                    && initial == &self.manifest
                    && url.query().is_none()
                    && url.path().strip_prefix(&tagged_prefix).is_some_and(|path| {
                        path.strip_suffix("/latest.json")
                            .and_then(|tag| tag.strip_prefix('v'))
                            .is_some_and(|version| stable_version(version).is_ok())
                    });
            }
            return url.port().is_none()
                && matches!(
                    url.host_str(),
                    Some("release-assets.githubusercontent.com" | "objects.githubusercontent.com")
                )
                && url.path().starts_with("/github-production-release-asset");
        }
        self.name != "local-test"
            && url.origin() == initial.origin()
            && url.path() == initial.path()
            && url.query().is_none()
    }

    fn client(&self, timeout: Duration) -> Result<Client, UpdateError> {
        let source = self.clone();
        let mut builder = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(timeout)
            .https_only(self.name != "local-test")
            .redirect(redirect::Policy::custom(move |attempt| {
                if source.allows_redirect(attempt.previous(), attempt.url()) {
                    attempt.follow()
                } else {
                    attempt.error("unregistered update redirect")
                }
            }));
        if self.name == "local-test" {
            builder = builder.no_proxy();
        }
        builder.build().map_err(|_| invalid_configuration())
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Release {
    pub version: String,
    pub notes: String,
    pub source: usize,
    pub filename: String,
    pub target: String,
    pub(super) source_name: String,
    url: String,
    signature: String,
}

#[derive(Clone)]
pub(crate) struct Ready {
    pub release: Release,
    pub path: PathBuf,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CacheRecord {
    release: Release,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: String,
    notes: String,
    pub_date: String,
    platforms: Platforms,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Platforms {
    #[serde(rename = "darwin-aarch64")]
    macos: Option<ManifestAsset>,
    #[serde(rename = "windows-x86_64")]
    windows: Option<ManifestAsset>,
}

impl Platforms {
    fn entries(&self) -> impl Iterator<Item = (&str, &ManifestAsset)> {
        [
            ("darwin-aarch64", self.macos.as_ref()),
            ("windows-x86_64", self.windows.as_ref()),
        ]
        .into_iter()
        .filter_map(|(target, entry)| entry.map(|entry| (target, entry)))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestAsset {
    url: String,
    signature: String,
}

#[derive(Clone)]
pub(crate) struct Transport {
    key: PublicKey,
    sources: Vec<Source>,
    target: String,
}

impl Transport {
    pub(crate) fn new(
        public_key: &str,
        sources: Vec<Source>,
        target: &str,
    ) -> Result<Self, UpdateError> {
        if sources.is_empty() || target_suffix(target).is_none() {
            return Err(invalid_configuration());
        }
        let key = decode_box(public_key.trim(), 2).map_err(|_| invalid_configuration())?;
        if !valid_untrusted_comment(&key[0]) || !canonical_base64(&key[1], 42) {
            return Err(invalid_configuration());
        }
        let key = PublicKey::from_base64(&key[1]).map_err(|_| invalid_configuration())?;
        Ok(Self {
            key,
            sources,
            target: target.to_owned(),
        })
    }

    pub(crate) fn source_count(&self) -> usize {
        self.sources.len()
    }

    pub(crate) async fn check_source(
        &self,
        index: usize,
        current_version: &str,
    ) -> Result<Option<Release>, UpdateError> {
        let source = self.sources.get(index).ok_or_else(invalid_configuration)?;
        let client = source.client(Duration::from_secs(20))?;
        let mut response = client
            .get(source.manifest.clone())
            .send()
            .await
            .map_err(request_error)?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        check_response(&response)?;
        let mut document = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(request_error)? {
            if document.len() + chunk.len() > MAX_DOCUMENT_BYTES {
                return Err(integrity());
            }
            document.extend_from_slice(&chunk);
        }
        self.parse_manifest(index, current_version, &document)
    }

    fn parse_manifest(
        &self,
        index: usize,
        current_version: &str,
        document: &[u8],
    ) -> Result<Option<Release>, UpdateError> {
        if document.len() > MAX_DOCUMENT_BYTES {
            return Err(integrity());
        }
        let manifest: Manifest = serde_json::from_slice(document).map_err(|_| integrity())?;
        let version = parse_version(
            manifest
                .version
                .strip_prefix('v')
                .unwrap_or(&manifest.version),
        )?;
        if manifest.notes.len() > MAX_NOTES_BYTES
            || manifest.pub_date.len() > 64
            || time::OffsetDateTime::parse(
                &manifest.pub_date,
                &time::format_description::well_known::Rfc3339,
            )
            .is_err()
        {
            return Err(integrity());
        }
        let source = self.sources.get(index).ok_or_else(invalid_configuration)?;
        let current = parse_version(current_version).map_err(|_| invalid_configuration())?;
        let mut release = None;
        let mut has_target = false;
        for (target, asset) in manifest.platforms.entries() {
            has_target = true;
            let (signature, filename) = signature(&asset.signature, &version.to_string(), target)?;
            self.key
                .verify_stream(&signature)
                .map_err(|_| integrity())?;
            let expected = source.artifact_url(&version.to_string(), &filename);
            if expected.as_str() != asset.url {
                return Err(integrity());
            }
            if target == self.target {
                release = Some(Release {
                    version: version.to_string(),
                    notes: manifest.notes.clone(),
                    source: index,
                    source_name: source.name.clone(),
                    filename,
                    target: target.to_owned(),
                    url: asset.url.clone(),
                    signature: asset.signature.clone(),
                });
            }
        }
        if !has_target {
            return Err(integrity());
        }
        if !version.pre.is_empty()
            || !version.build.is_empty()
            || version.cmp_precedence(&current).is_le()
        {
            return Ok(None);
        }
        Ok(release)
    }

    pub(crate) async fn download(
        &self,
        release: &Release,
        cache_root: &Path,
        mut progress: impl FnMut(u64, Option<u64>),
    ) -> Result<Ready, UpdateError> {
        let source = self.sources.get(release.source).ok_or_else(integrity)?;
        let signature = self.validate_release(release)?;
        let url = source.artifact_url(&release.version, &release.filename);
        if source.name != release.source_name || url.as_str() != release.url {
            return Err(integrity());
        }
        ensure_cache(cache_root, true)?;
        let client = source.client(Duration::from_secs(15 * 60))?;
        let mut response = client.get(url).send().await.map_err(request_error)?;
        check_response(&response)?;
        let length = response.content_length();
        if length.is_some_and(|value| value == 0 || value > MAX_ARTIFACT_BYTES) {
            return Err(integrity());
        }
        let mut file = temporary(cache_root, PACKAGE_PREFIX, PACKAGE_SUFFIX)?;
        let mut verifier = self
            .key
            .verify_stream(&signature)
            .map_err(|_| integrity())?;
        let mut received = 0;
        progress(received, length);
        while let Some(chunk) = response.chunk().await.map_err(request_error)? {
            received += chunk.len() as u64;
            if received > MAX_ARTIFACT_BYTES || length.is_some_and(|length| received > length) {
                return Err(integrity());
            }
            file.write_all(&chunk).map_err(cache_error)?;
            verifier.update(&chunk);
            progress(received, length);
        }
        if received == 0 || length.is_some_and(|length| received != length) {
            return Err(unavailable("incompleteBody"));
        }
        verifier.finalize().map_err(|_| integrity())?;
        file.as_file().sync_all().map_err(cache_error)?;

        let package = file.path().to_owned();
        let record = CacheRecord {
            release: release.clone(),
            name: package
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(integrity)?
                .to_owned(),
        };
        let mut metadata = temporary(cache_root, META_PREFIX, META_SUFFIX)?;
        serde_json::to_writer(&mut metadata, &record).map_err(|error| {
            error
                .io_error_kind()
                .map_or_else(integrity, |kind| cache_error(kind.into()))
        })?;
        metadata.as_file().sync_all().map_err(cache_error)?;
        // The package stays owned by its cleanup guard until the new record
        // commits. A failed download or record replacement keeps the old pair.
        let (held, package_path) = file.keep().map_err(|error| cache_error(error.error))?;
        drop(held);
        if let Err(error) = metadata.persist(cache_root.join(READY_FILE)) {
            let _ = fs::remove_file(package_path);
            return Err(cache_error(error.error));
        }
        native_fs::sync_directory(cache_root).map_err(cache_error)?;
        clean_cache(cache_root, Some(&package));
        Ok(Ready {
            release: release.clone(),
            path: package,
        })
    }

    pub(crate) fn recover(
        &self,
        cache_root: &Path,
        current_version: &str,
    ) -> Result<Option<Ready>, UpdateError> {
        if !ensure_cache(cache_root, false)? {
            return Ok(None);
        }
        let record_path = cache_root.join(READY_FILE);
        let mut record_file = match native_fs::open_read(
            &record_path,
            native_fs::ReadOptions::NoLinksNonblockingCloseOnExec,
        ) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                clean_cache(cache_root, None);
                return Ok(None);
            }
            Err(error) => return Err(cache_error(error)),
        };
        let len = ordinary_file_size(&record_file, MAX_DOCUMENT_BYTES as u64)?;
        let mut document = Vec::new();
        (&mut record_file)
            .take(MAX_DOCUMENT_BYTES as u64 + 1)
            .read_to_end(&mut document)
            .map_err(cache_error)?;
        let record: CacheRecord = serde_json::from_slice(&document).map_err(|_| integrity())?;
        if document.len() as u64 != len
            || !temporary_name(&record.name, PACKAGE_PREFIX, PACKAGE_SUFFIX)
        {
            return Err(integrity());
        }
        let version = stable_version(&record.release.version)?;
        let current = parse_version(current_version).map_err(|_| invalid_configuration())?;
        if version.cmp_precedence(&current).is_le() {
            drop(record_file);
            fs::remove_file(&record_path).map_err(cache_error)?;
            clean_cache(cache_root, None);
            return Ok(None);
        }
        let ready = Ready {
            release: record.release,
            path: cache_root.join(record.name),
        };
        if let Err(error) = self.verify_ready(&ready) {
            if error.kind == ErrorKind::Integrity {
                drop(record_file);
                fs::remove_file(&record_path).map_err(cache_error)?;
                clean_cache(cache_root, None);
            }
            return Err(error);
        }
        clean_cache(cache_root, Some(&ready.path));
        Ok(Some(ready))
    }

    pub(crate) fn verify_ready(&self, ready: &Ready) -> Result<File, UpdateError> {
        let parent = ready.path.parent().ok_or_else(integrity)?;
        if !ensure_cache(parent, false)?
            || !ready
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| temporary_name(name, PACKAGE_PREFIX, PACKAGE_SUFFIX))
        {
            return Err(integrity());
        }

        let signature = self.validate_release(&ready.release)?;
        let mut file = open_package(&ready.path)?;
        let size = ordinary_file_size(&file, MAX_ARTIFACT_BYTES)?;
        let before = directory::stamp(&file).map_err(cache_error)?;
        let mut verifier = self
            .key
            .verify_stream(&signature)
            .map_err(|_| integrity())?;
        let mut input = (&mut file).take(MAX_ARTIFACT_BYTES + 1);
        let mut bytes = [0; 64 * 1024];
        let mut verified = 0_u64;
        loop {
            let count = input.read(&mut bytes).map_err(cache_error)?;
            if count == 0 {
                break;
            }
            verified += count as u64;
            verifier.update(&bytes[..count]);
        }
        verifier.finalize().map_err(|_| integrity())?;
        if verified != size
            || directory::stamp(&file).map_err(cache_error)? != before
            || directory::stamp(&open_package(&ready.path)?).map_err(cache_error)? != before
        {
            return Err(integrity());
        }
        file.rewind().map_err(cache_error)?;
        Ok(file)
    }

    fn validate_release(&self, release: &Release) -> Result<Signature, UpdateError> {
        if release.notes.len() > MAX_NOTES_BYTES || release.target != self.target {
            return Err(integrity());
        }
        stable_version(&release.version)?;
        let (signature, filename) =
            signature(&release.signature, &release.version, &release.target)?;
        if filename != release.filename {
            return Err(integrity());
        }
        if self.sources.iter().any(|source| {
            source.fixed_release
                && (source.name != release.source_name
                    || source.artifact_url(&release.version, &filename).as_str() != release.url)
        }) {
            return Err(integrity());
        }
        Ok(signature)
    }
}

fn check_response(response: &Response) -> Result<(), UpdateError> {
    let status = response.status();
    if status == StatusCode::OK {
        return Ok(());
    }
    let mut failure = if status.is_server_error()
        || matches!(
            status,
            StatusCode::NOT_FOUND
                | StatusCode::GONE
                | StatusCode::REQUEST_TIMEOUT
                | StatusCode::TOO_MANY_REQUESTS
        ) {
        unavailable("httpStatus")
    } else {
        let mut failure = integrity();
        failure.reason = "httpStatus";
        failure
    };
    failure.http_status = Some(status.as_u16());
    Err(failure)
}

fn target_suffix(target: &str) -> Option<&'static str> {
    match target {
        "darwin-aarch64" => Some(".app.tar.gz"),
        "windows-x86_64" => Some(".exe"),
        _ => None,
    }
}

fn parse_version(text: &str) -> Result<Version, UpdateError> {
    if text.len() > 128 {
        return Err(integrity());
    }
    Version::parse(text).map_err(|_| integrity())
}

fn stable_version(text: &str) -> Result<Version, UpdateError> {
    let version = parse_version(text)?;
    if !version.pre.is_empty() || !version.build.is_empty() {
        return Err(integrity());
    }
    Ok(version)
}

fn decode_box(value: &str, expected_lines: usize) -> Result<Vec<String>, UpdateError> {
    if value.len() > MAX_SIGNATURE_BYTES {
        return Err(integrity());
    }
    let decoded = STANDARD.decode(value).map_err(|_| integrity())?;
    if STANDARD.encode(&decoded) != value {
        return Err(integrity());
    }
    let decoded = String::from_utf8(decoded).map_err(|_| integrity())?;
    let lines: Vec<_> = decoded
        .strip_suffix('\n')
        .unwrap_or(&decoded)
        .split('\n')
        .map(str::to_owned)
        .collect();
    if lines.len() != expected_lines {
        return Err(integrity());
    }
    Ok(lines)
}

fn canonical_base64(value: &str, expected_length: usize) -> bool {
    STANDARD
        .decode(value)
        .is_ok_and(|bytes| bytes.len() == expected_length && STANDARD.encode(&bytes) == value)
}

fn valid_untrusted_comment(line: &str) -> bool {
    line.strip_prefix("untrusted comment: ")
        .is_some_and(|comment| {
            !comment.is_empty()
                && comment.chars().count() <= 200
                && !comment
                    .chars()
                    .any(|character| character.is_ascii_control())
        })
}

fn signature(
    encoded: &str,
    version: &str,
    target: &str,
) -> Result<(Signature, String), UpdateError> {
    let lines = decode_box(encoded, 4)?;
    if !valid_untrusted_comment(&lines[0])
        || !canonical_base64(&lines[1], 74)
        || !canonical_base64(&lines[3], 64)
    {
        return Err(integrity());
    }
    let packet = STANDARD.decode(&lines[1]).map_err(|_| integrity())?;
    if !packet.starts_with(b"ED") {
        return Err(integrity());
    }
    let comment = lines[2]
        .strip_prefix("trusted comment: timestamp:")
        .ok_or_else(integrity)?;
    let (timestamp, rest) = comment.split_once("\tfile:").ok_or_else(integrity)?;
    let (filename, signed_version) = rest.split_once("\tversion:").ok_or_else(integrity)?;
    if timestamp.parse::<u64>().map_or(true, |number| {
        number > 253402300799 || number.to_string() != timestamp
    }) || version != signed_version
        || !valid_filename(filename, target)
    {
        return Err(integrity());
    }
    let signature = Signature::decode(&lines.join("\n")).map_err(|_| integrity())?;
    Ok((signature, filename.to_owned()))
}

fn valid_filename(filename: &str, target: &str) -> bool {
    if filename.len() > 255 || filename.contains("..") || !filename.nfc().eq(filename.chars()) {
        return false;
    }
    let Some(stem) = target_suffix(target).and_then(|suffix| filename.strip_suffix(suffix)) else {
        return false;
    };
    stem.chars().next().is_some_and(char::is_alphanumeric)
        && !stem.ends_with(' ')
        && stem.chars().all(|character| {
            character.is_alphanumeric() || matches!(character, ' ' | '.' | '_' | '-')
        })
}

fn ensure_cache(path: &Path, create: bool) -> Result<bool, UpdateError> {
    if !native_fs::is_explicit_non_root_absolute_path(path)
        || native_fs::find_reparse_component(path, true)
            .map_err(cache_error)?
            .is_some()
    {
        return Err(integrity());
    }
    let info = match fs::symlink_metadata(path) {
        Ok(info) => info,
        Err(error) if error.kind() == io::ErrorKind::NotFound && !create => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match native_fs::create_private_directory(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(cache_error(error)),
            }
            fs::symlink_metadata(path).map_err(cache_error)?
        }
        Err(error) => return Err(cache_error(error)),
    };
    if !info.is_dir()
        || native_fs::is_reparse(&info)
        || !native_fs::directory_is_private(path).map_err(cache_error)?
    {
        return Err(integrity());
    }
    Ok(true)
}

fn temporary(
    root: &Path,
    prefix: &str,
    suffix: &str,
) -> Result<tempfile::NamedTempFile, UpdateError> {
    Builder::new()
        .prefix(prefix)
        .suffix(suffix)
        .rand_bytes(16)
        .make_in(root, |path| native_fs::create_new_private_file(path, true))
        .map_err(cache_error)
}

fn temporary_name(name: &str, prefix: &str, suffix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|name| name.strip_suffix(suffix))
        .is_some_and(|random| {
            random.len() == 16 && random.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}

fn clean_cache(root: &Path, keep: Option<&Path>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if keep == Some(entry.path().as_path()) {
            continue;
        }
        let name = entry.file_name();
        if name.to_str().is_some_and(|name| {
            temporary_name(name, PACKAGE_PREFIX, PACKAGE_SUFFIX)
                || temporary_name(name, META_PREFIX, META_SUFFIX)
        }) && entry.file_type().is_ok_and(|kind| kind.is_file())
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn ordinary_file_size(file: &File, max: u64) -> Result<u64, UpdateError> {
    let info = file.metadata().map_err(cache_error)?;
    if !info.is_file() || native_fs::is_reparse(&info) || info.len() == 0 || info.len() > max {
        return Err(integrity());
    }
    #[cfg(unix)]
    let links = {
        use std::os::unix::fs::MetadataExt;
        info.nlink()
    };
    #[cfg(windows)]
    let links = platform_native::windows::files::identity(file)
        .map_err(cache_error)?
        .links;
    if links != 1 {
        return Err(integrity());
    }
    Ok(info.len())
}

fn open_package(path: &Path) -> Result<File, UpdateError> {
    native_fs::open_read(path, native_fs::ReadOptions::NoLinksNonblockingCloseOnExec)
        .map_err(|_| integrity())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::{
        io::{BufRead, BufReader},
        net::{Ipv4Addr, TcpListener},
        thread,
        time::Instant,
    };

    const BODY: &[u8] = b"Panda transport bytes\n";
    const FILENAME: &str = "熊猫小抽屉 0.1.1.app.tar.gz";
    const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IGRpc3Bvc2FibGUgcHVibGljIGtleQpSV1J3WVc1a1lURXlNOUJLc2pKMEs3U3JPaE5vdlVZVjVPYlFJa3EzR2dGcnI0VWdvekxKZDRjMwo=";
    const SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IGRpc3Bvc2FibGUgc2lnbmF0dXJlClJVUndZVzVrWVRFeU15SVA2cGdqOXJXSmhLd2NqUTcxUm1ocDhoQjVtS1AxTGhpZE1UbFZvUDdOcC9FZ2dkaTJHZlhkSGprT0t3VklzWEdRTEJhVnA5OTBJNkF0VDBZYmZnST0KdHJ1c3RlZCBjb21tZW50OiB0aW1lc3RhbXA6MTc4OTAwMDAwMAlmaWxlOueGiueMq+Wwj+aKveWxiSAwLjEuMS5hcHAudGFyLmd6CXZlcnNpb246MC4xLjEKZXpKV2c4QXRnZWgzeE9ucmxNcEp3WHBzM2FId1VZKzR5UDFzOXFaWFgzTU9lNEM4cjBNdzcyN2hQUDd1NWpzK0xQSmZlTWdvaEYydWRDUVpOaGk4QlE9PQo=";

    fn manifest(source: &Source) -> Value {
        json!({
            "version": "0.1.1",
            "notes": "本地测试",
            "pub_date": "2026-11-19T00:00:00+08:00",
            "platforms": {
                "darwin-aarch64": {
                    "url": source.artifact_url("0.1.1", FILENAME).as_str(),
                    "signature": SIGNATURE
                }
            }
        })
    }

    #[test]
    fn check_does_not_fetch_updates_and_confirmed_download_can_be_verified_offline() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let source = Source::loopback(listener.local_addr().unwrap().port());
        let document = serde_json::to_vec(&manifest(&source)).unwrap();
        let artifact = source.artifact_url("0.1.1", FILENAME);
        let (sent, seen) = std::sync::mpsc::channel();
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            for (route, status, bytes) in [
                ("/latest.json", "200 OK", document.as_slice()),
                (artifact.path(), "503 Service Unavailable", &[]),
                (artifact.path(), "200 OK", BODY),
            ] {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline);
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("cannot accept fixture connection: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(&stream);
                let mut start = String::new();
                reader.read_line(&mut start).unwrap();
                assert_eq!(start.split_ascii_whitespace().nth(1), Some(route));
                sent.send(route.to_owned()).unwrap();
                loop {
                    let mut header = String::new();
                    assert_ne!(reader.read_line(&mut header).unwrap(), 0);
                    if header == "\r\n" {
                        break;
                    }
                }
                writeln!(
                    stream,
                    "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n\r",
                    bytes.len()
                )
                .unwrap();
                stream.write_all(bytes).unwrap();
            }
        });

        let transport = Transport::new(PUBLIC_KEY, vec![source], "darwin-aarch64").unwrap();
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().canonicalize().unwrap().join("updates");
        tauri::async_runtime::block_on(async {
            let release = transport.check_source(0, "0.1.0").await.unwrap().unwrap();
            assert_eq!(seen.try_recv().as_deref(), Ok("/latest.json"));
            assert!(seen.try_recv().is_err());
            assert!(!cache.exists());

            let failure = transport
                .download(&release, &cache, |_, _| {
                    panic!("failed response has no progress")
                })
                .await
                .err()
                .unwrap();
            assert_eq!(failure.kind, ErrorKind::Network);
            assert_eq!(failure.reason, "httpStatus");
            assert_eq!(failure.http_status, Some(503));
            assert!(fs::read_dir(&cache).unwrap().next().is_none());

            let mut latest_progress = None;
            let downloaded = transport
                .download(&release, &cache, |current, total| {
                    latest_progress = Some((current, total));
                })
                .await
                .unwrap();
            assert_eq!(
                latest_progress,
                Some((BODY.len() as u64, Some(BODY.len() as u64)))
            );
            let resumed = transport.recover(&cache, "0.1.0").unwrap().unwrap();
            assert_eq!(resumed.path, downloaded.path);
            assert_eq!(resumed.release.filename, FILENAME);
            let mut opened = transport.verify_ready(&resumed).unwrap();
            let mut read = Vec::new();
            opened.read_to_end(&mut read).unwrap();
            assert_eq!(read, BODY);
            drop(opened);

            fs::write(&downloaded.path, b"damaged file").unwrap();
            assert_eq!(
                transport.verify_ready(&resumed).unwrap_err().kind,
                ErrorKind::Integrity
            );
            assert_eq!(
                transport.recover(&cache, "0.1.0").err().unwrap().kind,
                ErrorKind::Integrity
            );
            assert!(!downloaded.path.exists());
        });
        server.join().unwrap();
    }

    #[test]
    fn manifest_binding_rejects_a_relabelled_or_off_origin_package() {
        let source = Source::loopback(18427);
        let original = manifest(&source);
        let transport = Transport::new(PUBLIC_KEY, vec![source], "darwin-aarch64").unwrap();
        let parse = |value: &Value| {
            transport.parse_manifest(0, "0.1.0", &serde_json::to_vec(value).unwrap())
        };
        assert!(parse(&original).unwrap().is_some());
        assert!(transport
            .parse_manifest(0, "0.1.1", &serde_json::to_vec(&original).unwrap())
            .unwrap()
            .is_none());

        let mut newer = original.clone();
        newer["version"] = json!("0.1.2");
        assert_eq!(parse(&newer).err().unwrap().kind, ErrorKind::Integrity);
        let mut other_source = original;
        other_source["platforms"]["darwin-aarch64"]["url"] =
            json!("http://127.0.0.1:18428/v0.1.1/example.app.tar.gz");
        assert_eq!(
            parse(&other_source).err().unwrap().kind,
            ErrorKind::Integrity
        );
    }

    #[test]
    fn registry_keeps_route_order_and_rejects_user_provided_origins() {
        let sources = configured_sources("global", true).unwrap();
        assert_eq!(sources[0].name, "github");
        assert_eq!(configured_sources("global", false).unwrap().len(), 1);
        assert!(configured_sources("https://example.invalid", true).is_err());
        assert!(!sources[0].allows_redirect(&[sources[0].manifest.clone()], &Url::parse("https://release-assets.githubusercontent.com.invalid/github-production-release-asset").unwrap()));
        assert!(!Source::loopback(18427).allows_redirect(
            &[Url::parse("http://127.0.0.1:18427/latest.json").unwrap()],
            &Url::parse("http://127.0.0.1:18428/latest.json").unwrap()
        ));
    }

    #[test]
    fn github_test_uses_only_its_compiled_release_and_official_asset_redirect() {
        let tag = "auto-update-test-20261119-120000";
        let source = Source::github_test(tag).unwrap();
        let base = format!("https://github.com/happypandaa/panda-drawer/releases/download/{tag}");
        assert_eq!(source.manifest.as_str(), format!("{base}/latest.json"));
        let artifact = source.artifact_url("0.1.1", "test.app.tar.gz");
        assert_eq!(artifact.as_str(), format!("{base}/test.app.tar.gz"));
        assert!(source.allows_redirect(
            std::slice::from_ref(&artifact),
            &Url::parse("https://release-assets.githubusercontent.com/github-production-release-asset-2e65be/test?sig=abc").unwrap()
        ));
        assert!(!source.allows_redirect(
            std::slice::from_ref(&artifact),
            &Url::parse("https://github.com/happypandaa/panda-drawer/releases/download/auto-update-test-20261119-120001/test.app.tar.gz").unwrap()
        ));
        assert!(Source::github_test("auto-update-test-20261119-120000/other").is_err());
        assert!(Source::github_test("v0.1.1").is_err());

        let document = serde_json::to_vec(&manifest(&source)).unwrap();
        let transport = Transport::new(PUBLIC_KEY, vec![source], "darwin-aarch64").unwrap();
        let release = transport
            .parse_manifest(0, "0.1.0", &document)
            .unwrap()
            .unwrap();
        assert!(transport.validate_release(&release).is_ok());

        let other = Source::github_test("auto-update-test-20261119-120001").unwrap();
        let other_transport = Transport::new(PUBLIC_KEY, vec![other], "darwin-aarch64").unwrap();
        assert!(other_transport
            .parse_manifest(0, "0.1.0", &document)
            .is_err());
        assert!(other_transport.validate_release(&release).is_err());
    }
}
