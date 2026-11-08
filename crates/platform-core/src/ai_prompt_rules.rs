// Generated from scripts/project-ai-requirements.json.
// Run node scripts/generate-project-ai-requirements.mjs after editing the source.
//! Rules and handoff templates shared by prompts that the platform copies for external AI.
pub const PLATFORM_AI_SUMMARY: &str = "【平台要求摘要】\n- 两种应用：网页应用（普通静态网页，可以是 Vite、React 的构建结果或单个 HTML 文件，可选用平台 SDK 存 SQLite 数据和附件）；Node 应用（静态网页加一个 Node 24 后端）。不支持 SSR、多个服务、Python、Java、Docker 等其他运行时；只支持 npm。\n- 项目根目录放一个 panda.json，写明名称、类型和入口，不能有多余字段。\n  网页最小示例：{\"manifestVersion\":1,\"runtime\":\"web\",\"app\":{\"name\":\"我的网页\"},\"web\":{\"root\":\".\",\"entry\":\"index.html\"}}\n  Node 最小示例：{\"manifestVersion\":1,\"runtime\":\"node\",\"app\":{\"name\":\"我的应用\"},\"web\":{\"root\":\"web\",\"entry\":\"index.html\"},\"backend\":{\"root\":\"backend\",\"entry\":\"server.mjs\",\"nodeMajor\":24,\"protocolVersion\":1}}\n- 平台直接运行项目文件夹，不另存副本。网页需要构建时，在 panda.json 的 development.build 声明 npm 脚本（需要 package-lock.json），由 AI 或平台执行；Node 应用的运行文件要事先准备好，平台不为它构建。\n- 网页在固定的本机地址根路径运行，普通 Vite、React、纯 HTML 项目一般不用改；允许内联脚本和 https 上的脚本、样式、图片。可用 /_platform/sdk.js（SDK）和 /_platform/config.json（用户配置）。页面主动请求外部地址要在 panda.json 声明；网页不能保存密钥，需要密钥的功能放在 Node 后端。\n- Node 后端遵守平台控制协议：stdout 只输出协议消息，日志写 stderr；收到关闭指令要保存并退出。\n- 文件数据放在项目的 data/；配置项在 panda.json 声明，由用户在平台表单填写，不用 .env。\n- 接入方式：新建项目（平台生成网页起始项目）；添加本地项目（选空文件夹生成 Node 起始项目，或选已有 panda.json 的文件夹）；从 GitHub、Gitee 公开仓库添加。";
pub const GIT_SAVE: &str = "修改前先用 Git 保存当前版本，保留已有未提交改动；我要求回退时，用 Git 恢复到之前的版本。Git 只保存代码，不能代替数据备份。";
pub const DATA_CONSENT: &str = "任何会删除或清空应用数据、数据库、上传文件的操作，先说明影响并取得我明确同意。";
pub const SECRETS: &str = "不读取或输出真实 .env 和密钥。";
pub const STDOUT: &str = "stdout 只能输出平台控制协议消息；普通日志一律写 stderr（用 console.error，或把 console.log 重定向到 stderr），否则平台会判定协议错误并终止应用。";
pub const CHANGE_SIZE: &str = "小改动（文字、颜色、布局）说一句要改什么就直接改；涉及数据、表结构或较大的改动，先说计划，等我同意。";
pub const TOOLS: &str = "需要 Node、npm 或 Git 时，用平台命令行的 env doctor --json 和 env git doctor --json 查平台自带的。";
pub const RELOAD: &str = "改完告诉我点应用窗口上的「重新加载」看效果，并说明验证了什么。";
pub const CONTINUE_CHAT: &str = "最后告诉我：想继续改，直接在这个对话里说。";
pub const SCHEMA: &str = "改表结构时，同时改程序里的升级逻辑、docs/database.md 和 AGENTS.md 里受影响的步骤，只加列、加表。";
pub const TEMPLATE_MODIFY_WEB: &str = "请先读懂这个应用，暂时不要修改任何文件。\n\n{nameLine}项目文件夹：{projectRoot}\n平台说明：{skillPath}\n平台命令行：{cliPath}\n\n先读平台说明及其 references，再读项目的 panda.json、README 和开发文档；读不到说明时先告诉我。读完用一两句话说说你理解的这个应用，然后问我想改哪里；我问起时再细讲代码和数据。\n\n修改时请遵守：\n- 只修改上面的项目文件夹，其余规则以平台说明为准。{secrets}\n- {gitSave}\n- {changeSize}\n- {dataConsent}\n- 需要构建时按 panda.json 构建。{tools}\n- {reload}\n- {continueChat}\n";
pub const TEMPLATE_MODIFY_NODE: &str = "请先读懂这个应用，暂时不要修改任何文件。\n\n{nameLine}项目文件夹：{projectRoot}\n平台说明：{skillPath}\n平台命令行：{cliPath}\n日志目录：{logsPath}（current.log 本次、previous.log 上次）\n\n先读平台说明及其 references，再读项目的 panda.json、README 和日志；读不到说明时先告诉我。读完用一两句话说说你理解的这个应用，然后问我想改哪里；我问起时再细讲代码和数据。\n\n修改时请遵守：\n- 只修改上面的项目文件夹，其余规则以平台说明为准。{secrets}\n- {gitSave}\n- {changeSize}\n- {dataConsent}\n- {stdout}\n- {tools}\n- {reload}\n- {continueChat}\n";
pub const TEMPLATE_MODIFY_AGENT_WEB: &str = "请先读懂这个应用，暂时不要修改任何文件。这次是要修改应用本身，不是日常使用。\n\n{nameLine}项目文件夹：{projectRoot}\n平台说明：{skillPath}\n平台命令行：{cliPath}\n\n先读平台说明及其 references，再读 panda.json、{instructions} 和其中提到的文档；读不到说明时先告诉我。用一两句话告诉我这个应用做什么、你平时替我做什么，然后问我想改哪里。如果我说的其实是日常操作（比如加一个关注主题），就按 {instructions} 做，不改代码。\n\n修改时请遵守：\n- 只修改上面的项目文件夹，其余规则以平台说明为准。{secrets}\n- {gitSave}\n- {changeSize}\n- {dataConsent}\n- {schema}\n- 改 {instructions} 前先告诉我改什么，它是你平时替我做事的规则。\n- 需要构建时按 panda.json 构建。{tools}\n- 改完告诉我点应用窗口上的「重新加载」；改了表结构的，提醒我先重新加载一次，再让 AI 处理日常事务。\n- {continueChat}\n";
pub const TEMPLATE_MODIFY_AGENT_NODE: &str = "请先读懂这个应用，暂时不要修改任何文件。这次是要修改应用本身，不是日常使用。\n\n{nameLine}项目文件夹：{projectRoot}\n平台说明：{skillPath}\n平台命令行：{cliPath}\n日志目录：{logsPath}（current.log 本次、previous.log 上次）\n\n先读平台说明及其 references，再读 panda.json、{instructions} 和其中提到的文档；读不到说明时先告诉我。用一两句话告诉我这个应用做什么、你平时替我做什么，然后问我想改哪里。如果我说的其实是日常操作（比如加一个关注主题），就按 {instructions} 做，不改代码。\n\n修改时请遵守：\n- 只修改上面的项目文件夹，其余规则以平台说明为准。{secrets}\n- {gitSave}\n- {changeSize}\n- {dataConsent}\n- {schema}\n- 改 {instructions} 前先告诉我改什么，它是你平时替我做事的规则。\n- {stdout}\n- {tools}\n- 改完告诉我点应用窗口上的「重新加载」；改了表结构的，提醒我先重新加载一次，再让 AI 处理日常事务。\n- {continueChat}\n";
pub const TEMPLATE_CREATE: &str = "我想在熊猫小抽屉里做一个新应用，请先和我聊清楚要做什么，暂时不要写代码。\n\n{nameLine}{descriptionLine}项目文件夹：{projectRoot}（平台已经生成了起始项目）\n平台说明：{skillPath}\n平台命令行：{cliPath}\n\n先读平台说明及其 references 和起始项目的 README。然后用通俗的话问我：这个应用给谁用、最想先做的一件事是什么。问清楚后，说明第一版做哪些、怎么做，等我同意再动手。\n\n做的时候请遵守：\n- 只修改上面的项目文件夹，其余规则以平台说明为准。{secrets}\n- {gitSave}\n- {dataConsent}\n- 需要 API Key 时，在 panda.json 声明配置项，由我在平台里填写，不要让我把 Key 发给你；需要后台或密钥时，按平台说明把应用改成 Node 应用。\n- 需要构建时按 panda.json 构建。{tools}\n- 每完成一步，告诉我点应用窗口上的「重新加载」看效果。\n";

/// Rule placeholders available to every template.
pub const RULES: [(&str, &str); 9] = [
    ("gitSave", GIT_SAVE),
    ("dataConsent", DATA_CONSENT),
    ("secrets", SECRETS),
    ("stdout", STDOUT),
    ("changeSize", CHANGE_SIZE),
    ("tools", TOOLS),
    ("reload", RELOAD),
    ("continueChat", CONTINUE_CHAT),
    ("schema", SCHEMA),
];

/// Fill `{name}` placeholders in one pass. Rules come first; unknown names stay as written,
/// and values are inserted verbatim, so a value containing braces is never expanded again.
pub fn render(template: &str, values: &[(&str, &str)]) -> String {
    let mut output = String::with_capacity(template.len() + 256);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        output.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let name_end = after.find('}');
        let name = name_end.map(|end| &after[..end]);
        let valid = name.is_some_and(|name| {
            name.chars().next().is_some_and(|first| first.is_ascii_alphabetic())
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        let value: Option<&str> = match name {
            Some(name) if valid => {
                let rule: Option<&str> = RULES
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| *value);
                rule.or_else(|| {
                    values
                        .iter()
                        .find(|(key, _)| *key == name)
                        .map(|(_, value)| *value)
                })
            }
            _ => None,
        };
        match (value, name_end) {
            (Some(value), Some(end)) => {
                output.push_str(value);
                rest = &after[end + 1..];
            }
            _ => {
                output.push('{');
                rest = after;
            }
        }
    }
    output.push_str(rest);
    output
}
