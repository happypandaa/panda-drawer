//! Bounded failure excerpts, never a process output log. Successful commands
//! discard their buffers; failed commands attach a scrubbed summary to ApiError.
use crate::{environment::Toolchain, ApiError};
use regex::Regex;
use serde_json::json;
use std::{
    collections::VecDeque,
    process::ExitStatus,
    sync::{Arc, Mutex, OnceLock},
};
use tokio::io::{AsyncRead, AsyncReadExt};

const RAW_BYTES: usize = 16 * 1024;
const EXCERPT_BYTES: usize = 4096;

#[derive(Clone, Default)]
struct Tail {
    bytes: VecDeque<u8>,
    starts_mid_line: bool,
    truncated: bool,
    line: Vec<u8>,
    long_line: bool,
    private_key: bool,
}
impl Tail {
    fn append(&mut self, bytes: &[u8]) {
        for byte in bytes {
            if self.line.len() < RAW_BYTES {
                self.line.push(*byte);
            } else {
                self.long_line = true;
                self.truncated = true;
            }
            if *byte == b'\n' {
                self.finish_line();
            }
        }
    }

    fn finish_line(&mut self) {
        let line = std::mem::take(&mut self.line);
        let text = String::from_utf8_lossy(&line);
        let begins = text.contains("-----BEGIN ") && text.contains("PRIVATE KEY-----");
        let ends = text.contains("-----END ") && text.contains("PRIVATE KEY-----");
        if self.private_key {
            if ends {
                self.private_key = false;
            }
        } else if begins {
            // This state survives truncation of the tail and incomplete blocks.
            self.private_key = !ends;
            self.append_raw("[已隐藏私钥]\n".as_bytes());
        } else if self.long_line {
            self.append_raw("[过长的单行输出已省略]\n".as_bytes());
        } else {
            self.append_raw(&line);
        }
        self.long_line = false;
    }

    fn append_raw(&mut self, bytes: &[u8]) {
        for byte in bytes {
            if self.bytes.len() == RAW_BYTES {
                self.starts_mid_line = self.bytes.pop_front() != Some(b'\n');
                self.truncated = true;
            }
            self.bytes.push_back(*byte);
        }
    }

    fn excerpt(&self) -> (String, bool) {
        let mut complete = self.clone();
        if !complete.line.is_empty() || complete.long_line {
            complete.finish_line();
        }
        complete.complete_excerpt()
    }

    fn complete_excerpt(&self) -> (String, bool) {
        let bytes: Vec<_> = self.bytes.iter().copied().collect();
        // Never publish a tail whose credential label may have been discarded.
        let bytes = if self.starts_mid_line {
            let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') else {
                return ("[过长的单行输出已省略]".into(), true);
            };
            &bytes[newline + 1..]
        } else {
            &bytes[..]
        };
        let text = scrub(&String::from_utf8_lossy(bytes));
        let (text, clipped) = bounded_tail(&text, EXCERPT_BYTES);
        (text, self.truncated || clipped)
    }
}

#[derive(Default)]
struct Streams {
    stdout: Tail,
    stderr: Tail,
}

#[derive(Clone, Default)]
pub(crate) struct ProcessDiagnostics(Arc<Mutex<Streams>>);

impl ProcessDiagnostics {
    pub(crate) async fn read(
        &self,
        mut reader: impl AsyncRead + Unpin,
        stream: &str,
        output_limit: Option<usize>,
    ) -> Result<(), ApiError> {
        let mut total = 0usize;
        let mut bytes = [0u8; 8192];
        loop {
            let count = reader.read(&mut bytes).await?;
            if count == 0 {
                return Ok(());
            }
            {
                let mut streams = self.0.lock().unwrap();
                let tail = if stream == "stderr" {
                    &mut streams.stderr
                } else {
                    &mut streams.stdout
                };
                tail.append(&bytes[..count]);
            }
            total = total.saturating_add(count);
            if output_limit.is_some_and(|limit| total > limit) {
                return Err(ApiError::new("PROCESS_OUTPUT_LIMIT", "进程输出超过限制。"));
            }
        }
    }

    pub(crate) fn attach(
        &self,
        error: ApiError,
        command: &str,
        tools: &Toolchain,
        status: Option<&ExitStatus>,
    ) -> ApiError {
        let streams = self.0.lock().unwrap();
        let (stdout, stdout_truncated) = streams.stdout.excerpt();
        let (stderr, stderr_truncated) = streams.stderr.excerpt();
        error.detail(
            "processDiagnostics",
            json!({
                "command": bounded_prefix(&scrub(command), 256),
                "exitCode": status.and_then(ExitStatus::code),
                "signal": status.and_then(platform_native::process_status::signal),
                "nodeVersion": bounded_prefix(&scrub(&tools.node.version), 128),
                "npmVersion": bounded_prefix(&scrub(&tools.npm.version), 128),
                "stdout": stdout,
                "stderr": stderr,
                "truncated": stdout_truncated || stderr_truncated,
            }),
        )
    }
}

fn bounded_prefix(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn bounded_tail(text: &str, limit: usize) -> (String, bool) {
    if text.len() <= limit {
        return (text.into(), false);
    }
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    if start > 0 && text.as_bytes()[start - 1] != b'\n' {
        let Some(newline) = text[start..].find('\n') else {
            return ("[过长的单行输出已省略]".into(), true);
        };
        start += newline + 1;
    }
    (text[start..].into(), true)
}

// Known credentials and terminal controls are scrubbed before the excerpt is
// persisted or displayed. This cannot identify arbitrary unlabeled secrets.
fn scrub(text: &str) -> String {
    static RULES: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        [
            (r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\))", ""),
            (r"(?is)-----BEGIN [^-]*PRIVATE KEY-----.*?(?:-----END [^-]*PRIVATE KEY-----|$)", "[已隐藏私钥]"),
            (r"(?i)(https?://)[^\s/@]+(?::[^\s/@]*)?@", "$1[已隐藏]@"),
            (r#"(?i)\bBearer\s+[^\s"'<>]+"#, "Bearer [已隐藏]"),
            (r"(?i)((?:authorization|set-cookie|cookie)\s*:\s*)[^\r\n]+", "$1[已隐藏]"),
            (r#"(?i)(["']?[a-z0-9_.-]*(?:api[_-]?key|auth[_-]?token|access[_-]?token|refresh[_-]?token|token|secret|password|passwd|authorization|cookie)[a-z0-9_.-]*["']?\s*[=:]\s*)[^\r\n,;}&]+"#, "$1[已隐藏]"),
            (r"\b(?:npm_[A-Za-z0-9]{16,}|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{12,})\b", "[已隐藏]"),
        ]
        .into_iter()
        .map(|(pattern, replacement)| (Regex::new(pattern).unwrap(), replacement))
        .collect()
    });
    let mut result = text.to_owned();
    for (pattern, replacement) in rules {
        result = pattern.replace_all(&result, *replacement).into_owned();
    }
    result.retain(|character| !character.is_control() || matches!(character, '\n' | '\t'));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::ExecutableInfo;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;

    fn tools() -> Toolchain {
        Toolchain {
            node: ExecutableInfo {
                path: "/unused/node".into(),
                version: "24.22.0".into(),
            },
            npm: ExecutableInfo {
                path: "/unused/npm".into(),
                version: "11.19.1".into(),
            },
            path: "/unused".into(),
        }
    }

    #[tokio::test]
    async fn failed_process_retains_both_streams_and_real_exit_status() {
        let capture = ProcessDiagnostics::default();
        capture
            .read(&b"stdout context\n"[..], "stdout", None)
            .await
            .unwrap();
        capture
            .read("npm error code E404\n下载失败\n".as_bytes(), "stderr", None)
            .await
            .unwrap();
        #[cfg(unix)]
        let failed = ExitStatus::from_raw(42 << 8);
        #[cfg(windows)]
        let failed = ExitStatus::from_raw(42);
        let error = capture.attach(
            ApiError::new("TEST", "failed"),
            "npm ci",
            &tools(),
            Some(&failed),
        );
        let detail = &error.details["processDiagnostics"];
        assert_eq!(detail["exitCode"], 42);
        assert!(detail["signal"].is_null());
        assert_eq!(detail["command"], "npm ci");
        assert_eq!(detail["nodeVersion"], "24.22.0");
        assert_eq!(detail["stdout"], "stdout context\n");
        assert!(detail["stderr"].as_str().unwrap().contains("下载失败"));
        assert_eq!(detail["truncated"], false);
        let signalled = capture.attach(
            ApiError::new("TEST", "failed"),
            "npm ci",
            &tools(),
            Some(&ExitStatus::from_raw(15)),
        );
        #[cfg(unix)]
        {
            assert!(signalled.details["processDiagnostics"]["exitCode"].is_null());
            assert_eq!(signalled.details["processDiagnostics"]["signal"], 15);
        }
        #[cfg(windows)]
        {
            assert_eq!(signalled.details["processDiagnostics"]["exitCode"], 15);
            assert!(signalled.details["processDiagnostics"]["signal"].is_null());
        }
    }

    #[test]
    fn excerpts_are_bounded_and_keep_the_last_error_after_noisy_output() {
        let mut tail = Tail::default();
        tail.append("前面的普通输出\n".repeat(5000).as_bytes());
        tail.append(b"npm error code E404\nnpm error dependency missing\n");
        let (text, truncated) = tail.excerpt();
        assert!(truncated);
        assert!(text.len() <= EXCERPT_BYTES);
        assert!(text.ends_with("npm error dependency missing\n"));
    }

    #[test]
    fn credentials_split_between_chunks_are_scrubbed_before_display() {
        let mut tail = Tail::default();
        tail.append(b"\x1b[31mnpm error\x1b[0m\nAuthorization: Bea");
        tail.append(b"rer artificial-header-value\nAPI_KEY=artificial-key-value\nhttps://user:artificial-password@registry.example/pkg?token=artificial-query-value\nnpm_abcdefghijklmnopqrstuvwxyz\n");
        let (text, _) = tail.excerpt();
        for secret in [
            "artificial-header-value",
            "artificial-key-value",
            "artificial-password",
            "artificial-query-value",
            "npm_abcdefghijklmnopqrstuvwxyz",
            "\x1b",
        ] {
            assert!(!text.contains(secret), "{text}");
        }
        assert!(text.contains("npm error"));
        assert!(text.contains("已隐藏"));
    }

    #[test]
    fn truncating_a_long_credential_line_does_not_publish_its_unlabelled_tail() {
        let mut tail = Tail::default();
        tail.append(b"Authorization: Bearer ");
        tail.append("s".repeat(RAW_BYTES * 2).as_bytes());
        tail.append(b"\nnpm error code E401\n");
        let (text, truncated) = tail.excerpt();
        assert!(truncated);
        assert!(text.ends_with("npm error code E401\n"));
        assert!(!text.contains("ssss"));
    }

    #[test]
    fn long_and_unclosed_private_key_blocks_never_enter_the_retained_tail() {
        for closed in [true, false] {
            let mut tail = Tail::default();
            tail.append(b"npm error before key\n-----BEGIN RSA PRIVATE ");
            tail.append(b"KEY-----\n");
            for _ in 0..1000 {
                tail.append(b"artificial-private-key-material-in-many-lines\n");
            }
            if closed {
                tail.append(b"-----END RSA PRIVATE KEY-----\nnpm error code E401\n");
            }
            let (text, _) = tail.excerpt();
            assert!(!text.contains("artificial-private-key-material"));
            assert!(text.contains("已隐藏私钥"));
            assert_eq!(text.contains("npm error code E401"), closed);
            assert!(tail.bytes.len() <= RAW_BYTES);
            assert!(tail.line.len() <= RAW_BYTES);
        }
    }

    #[tokio::test]
    async fn output_limit_still_fails_after_recording_the_available_excerpt() {
        let capture = ProcessDiagnostics::default();
        let error = capture
            .read(&b"npm error oversized\n"[..], "stderr", Some(2))
            .await
            .unwrap_err();
        assert_eq!(error.code, "PROCESS_OUTPUT_LIMIT");
        let error = capture.attach(error, "npm ci", &tools(), None);
        assert!(error.details["processDiagnostics"]["exitCode"].is_null());
        assert_eq!(
            error.details["processDiagnostics"]["stderr"],
            "npm error oversized\n"
        );
    }
}
