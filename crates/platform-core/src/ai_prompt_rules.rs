// Generated from docs/project-compatibility-review-prompt.md and docs/ai-prompt-rules.json.
// Run node scripts/generate-project-ai-requirements.mjs after editing either source.
//! Rules shared by prompts that the platform copies for external AI.
pub const GIT_SAVE: &str = "修改前先用 Git 保存当前版本，保留已有未提交改动；我要求回退时，用 Git 恢复到之前的版本。Git 只保存代码，不能代替数据备份。";
pub const DATA_CONSENT: &str = "任何会删除或清空应用数据、数据库、上传文件的操作，先说明影响并取得我明确同意。";
pub const SECRETS: &str = "不读取或输出真实 .env 和密钥。";
pub const STDOUT: &str = "stdout 只能输出平台控制协议消息；普通日志一律写 stderr（用 console.error，或把 console.log 重定向到 stderr），否则平台会判定协议错误并终止应用。";
