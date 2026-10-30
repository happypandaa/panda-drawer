//! Pure provider URL and branch policy; never discovers or executes Git.
use crate::ApiError;
fn error(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
}

pub(crate) fn validate_url(value: &str) -> Result<(String, String, String), ApiError> {
    let bad = || {
        error(
            "SOURCE_URL_INVALID",
            "请输入标准 GitHub 或 Gitee 的公开 HTTPS 仓库地址，不包含凭据、端口、查询或额外路径。",
        )
    };
    let (host, provider, path) = if let Some(path) = value.strip_prefix("https://github.com/") {
        ("github.com", "github", path)
    } else if let Some(path) = value.strip_prefix("https://gitee.com/") {
        ("gitee.com", "gitee", path)
    } else {
        return Err(bad());
    };
    if value.len() > 1024 {
        return Err(bad());
    }
    let mut parts = path.strip_suffix('/').unwrap_or(path).split('/');
    let owner = parts.next().ok_or_else(bad)?;
    let repo = parts.next().ok_or_else(bad)?;
    let safe = |s: &str| {
        (1..=255).contains(&s.len())
            && s.as_bytes()[0].is_ascii_alphanumeric()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    if parts.next().is_some() || !safe(owner) || !safe(repo) {
        return Err(bad());
    }
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if !safe(repo) {
        return Err(bad());
    }
    Ok((
        format!("https://{host}/{owner}/{repo}.git"),
        provider.into(),
        repo.into(),
    ))
}
pub(crate) fn validate_branch(value: &str) -> Result<(), ApiError> {
    if value.is_empty()
        || value.len() > 128
        || value == "HEAD"
        || value.starts_with("refs/")
        || value.contains("..")
        || value.contains("@{")
        || value.ends_with('.')
        || !value.as_bytes()[0].is_ascii_alphanumeric()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/".contains(&byte))
        || value
            .split('/')
            .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"))
        || ([40, 64].contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(error(
            "SOURCE_BRANCH_INVALID",
            "请输入普通分支名称，例如 main 或 codex/tts-providers。",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::AcquireSourceInput;
    use super::*;
    use serde_json::json;
    #[test]
    fn only_canonical_public_provider_urls_are_accepted() {
        assert_eq!(
            validate_url("https://github.com/git/git/").unwrap().0,
            "https://github.com/git/git.git"
        );
        assert_eq!(
            validate_url("https://gitee.com/mirrors/git.git").unwrap().1,
            "gitee"
        );
        for value in [
            "http://github.com/a/b",
            "https://github.com:443/a/b",
            "https://user@github.com/a/b",
            "https://github.com/a/b?token=private",
            "https://github.com/a/b#main",
            "https://github.com/a/b/tree/main",
            "https://github.com/a/%62",
            "https://github.com/a/../b",
            "git@github.com:a/b",
            "file:///tmp/repo",
            " https://github.com/a/b",
        ] {
            assert!(validate_url(value).is_err(), "{value}");
        }
    }
    #[test]
    fn source_branch_is_optional_and_restricted_to_literal_local_branch_names() {
        let input: AcquireSourceInput =
            serde_json::from_value(json!({"url":"https://github.com/example/disposable"})).unwrap();
        assert!(input.branch.is_none());
        for branch in [
            "main",
            "codex/tts-providers",
            "release/v1.2",
            "topic_2/test-3",
        ] {
            assert!(validate_branch(branch).is_ok(), "{branch}");
        }
        for branch in [
            "",
            "HEAD",
            "refs/heads/main",
            "--upload-pack=bad",
            "-main",
            "main~1",
            "main^{commit}",
            "a..b",
            "a//b",
            "a/.hidden",
            "topic.lock",
            "topic/branch.lock",
            "a/",
            "a.",
            "a@{1}",
            "a\\b",
            "a b",
            "main\nnext",
            "main:other",
        ] {
            assert_eq!(
                validate_branch(branch).unwrap_err().code,
                "SOURCE_BRANCH_INVALID",
                "{branch}"
            );
        }
        assert!(validate_branch(&"1".repeat(40)).is_err());
        assert!(validate_branch(&"b".repeat(129)).is_err());
        assert!(serde_json::from_value::<AcquireSourceInput>(
            json!({"url":"https://github.com/example/disposable","branch":true})
        )
        .is_err());
    }
}
