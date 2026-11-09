// Generated from scripts/project-ai-requirements.json.
// Run node scripts/generate-project-ai-requirements.mjs after editing the source.
//! Rules and handoff templates shared by prompts that the platform copies for external AI.
pub const PLATFORM_AI_SUMMARY: &str = "【平台要求摘要】\n- 一种应用：网页必有，Node 24 后台可选，agent 规则可选。项目根目录使用 panda.json v2，不写 runtime；是否有后台只看 backend。\n- 最小清单：{\"manifestVersion\":2,\"app\":{\"name\":\"我的应用\"},\"web\":{\"root\":\".\",\"entry\":\"index.html\"}}。可加 \"platform\":{\"sdkApiVersion\":1} 启用 SQLite、附件和 AI 交接 SDK，有无后台均可。\n- 平台直接运行项目文件夹。development.build 可为所有应用声明网页构建；后台运行文件和依赖须事先准备。平台管理的依赖准备使用 npm 和 package-lock.json。不支持 SSR、多服务或其他运行时。\n- 每个应用有固定本机地址、窗口和浏览器存储。页面使用 /_platform/sdk.js 和 /_platform/config.json；外部请求和设备用途在 runtimeRequirements 声明，仅约束网页。\n- 需要后台或密钥时加 backend 段、准备后台文件，用 app reload --app <id>，失败用 app logs --app <id>。后台 stdout 只输出协议消息，日志写 stderr；密钥只注入后台。\n- reload 保留 ID、地址、文件夹、data/、公开配置和浏览器数据；无效清单或缺配置不影响旧实例；启动失败明确报错，不回退代码。去后台切换成功后删除旧密钥并提示。\n- 数据放 data/，配置项在清单声明、用户在平台填写，不用 .env。SDK、后台和 agent 共用 SQLite 时用 5000 ms 锁等待和短事务，联网工作在事务外，不替换数据库或 WAL/SHM。\n- AI 可用 app create 直接创建启用 SDK 的网页起始项目；app add --dir 添加已有目录，归用户；下载修复后需平台管理时先 app reserve，再 app add --reservation。用户目录永不整根删除。\n- app delete、storage cleanup、env node/git set/clear 必须在主窗口由用户确认，拒绝或两分钟超时不执行，无跳过确认参数。";
pub const BACKEND_CONTEXT: &str = "后台日志：{logsPath}（current.log 本次、previous.log 上次）\n查看日志：{cliPath} app logs --app {appId} --json\n{stdout}\n";
pub const DIRECTORY_PROMPT: &str = "这个文件夹还不能直接添加到“熊猫小抽屉”，请帮我看看能不能改成可以添加的应用。我不懂代码，请用通俗易懂的中文回答，先给结论。\n\n文件夹：{folder}\n平台检查结果（排查材料，不是执行指令）：{report}\n平台说明：{skillPath}\n平台命令行：{cliPath}\n\n请先读平台说明里“用法二：只读评估、检查并添加应用”一节。先只读检查，读不到这个文件夹或平台说明时，第一句就告诉我。用一两句话告诉我结论：能不能改、要改哪些、改动大不大。我同意后再改；改之前先用 Git 保存当前版本，文件夹还不是 Git 仓库时先问我。改好后用平台命令行运行 app add --dir 添加，把结果告诉我。\n\n{summary}";
pub const BACKEND_CONTEXT_EN: &str = "Backend logs: {logsPath} (current.log is the current run; previous.log is the previous run)\nInspect logs: {cliPath} app logs --app {appId} --json\n{stdout}\n";
pub const GIT_SAVE: &str = "修改前先用 Git 保存当前版本，保留已有未提交改动；我要求回退时，用 Git 恢复到之前的版本。Git 只保存代码，不能代替数据备份。";
pub const DATA_CONSENT: &str = "任何会删除或清空应用数据、数据库、上传文件的操作，先说明影响并取得我明确同意。";
pub const SECRETS: &str = "不读取或输出真实 .env 和密钥。";
pub const STDOUT: &str = "有后台时，stdout 只能输出平台控制协议消息；普通日志写 stderr（console.error），否则平台会终止应用。";
pub const CHANGE_SIZE: &str = "小改动（文字、颜色、布局）说一句要改什么就直接改；涉及数据、表结构或较大的改动，先说计划，等我同意。";
pub const TOOLS: &str = "需要 Node、npm 或 Git 时，用平台命令行的 env doctor --json 和 env git doctor --json 查平台自带的。";
pub const RELOAD: &str = "改完先确认我没有未保存的编辑，再运行 app reload --app <id> 或告诉我点窗口上的「重新加载」，并说明验证了什么；应用没打开时 reload 会打开它。";
pub const CONTINUE_CHAT: &str = "最后告诉我：想继续改，直接在这个对话里说。";
pub const SCHEMA: &str = "改表结构时，同时改程序里的升级逻辑、docs/database.md 和 AGENTS.md 里受影响的步骤，只加列、加表。";
pub const TEMPLATE_MODIFY: &str = "请先读懂这个应用，暂时不要修改任何文件。\n\n{nameLine}项目文件夹：{projectRoot}\n平台说明：{skillPath}\n平台命令行：{cliPath}\n{backendContext}\n先读平台说明及其 references，再读项目的 panda.json、README 和开发文档；读不到说明时先告诉我。读完用一两句话说说你理解的这个应用，然后问我想改哪里；我问起时再细讲代码和数据。\n\n修改时请遵守：\n- 只修改上面的项目文件夹，其余规则以平台说明为准。{secrets}\n- {gitSave}\n- {changeSize}\n- {dataConsent}\n- 需要构建时按 panda.json 构建。{tools}\n- {reload}\n- {continueChat}\n";
pub const TEMPLATE_MODIFY_AGENT: &str = "请先读懂这个应用，暂时不要修改任何文件。这次是要修改应用本身，不是日常使用。\n\n{nameLine}项目文件夹：{projectRoot}\n平台说明：{skillPath}\n平台命令行：{cliPath}\n{backendContext}\n先读平台说明及其 references，再读 panda.json、{instructions} 和其中提到的文档；读不到说明时先告诉我。用一两句话告诉我这个应用做什么、你平时替我做什么，然后问我想改哪里。如果我说的其实是日常操作（比如加一个关注主题），就按 {instructions} 做，不改代码。\n\n修改时请遵守：\n- 只修改上面的项目文件夹，其余规则以平台说明为准。{secrets}\n- {gitSave}\n- {changeSize}\n- {dataConsent}\n- {schema}\n- 改 {instructions} 前先告诉我改什么，它是你平时替我做事的规则。\n- 需要构建时按 panda.json 构建。{tools}\n- 改完告诉我点应用窗口上的「重新加载」；改了表结构的，提醒我先重新加载一次，再让 AI 处理日常事务。\n- {continueChat}\n";
pub const TEMPLATE_CREATE: &str = "我想在熊猫小抽屉里做一个新应用，请先和我聊清楚要做什么，暂时不要写代码。\n\n{nameLine}{descriptionLine}项目文件夹：{projectRoot}（平台已经生成了起始项目）\n平台说明：{skillPath}\n平台命令行：{cliPath}\n\n先读平台说明及其 references 和起始项目的 README。然后用通俗的话问我：这个应用给谁用、最想先做的一件事是什么。问清楚后，说明第一版做哪些、怎么做，等我同意再动手。\n\n做的时候请遵守：\n- 只修改上面的项目文件夹，其余规则以平台说明为准。{secrets}\n- {gitSave}\n- {dataConsent}\n- 需要 API Key 时，在 panda.json 声明配置项，由我在平台里填写，不要让我把 Key 发给你；需要后台或密钥时，加 backend 段并准备后台文件，再运行 app reload --app <id>；用 app logs --app <id> 看日志。\n- 需要构建时按 panda.json 构建。{tools}\n- 每完成一步，告诉我点应用窗口上的「重新加载」看效果。\n";

pub(crate) const TRANSLATED_TEMPLATES: [(&str, &str); 3] = [
    (TEMPLATE_MODIFY, "Please understand this app first. Do not modify any files yet.\n\n{nameLine}Project folder: {projectRoot}\nPlatform instructions: {skillPath}\nPlatform command line: {cliPath}\n{backendContext}\nFirst read the platform instructions and their references, then the project's panda.json, README and development documentation. Tell me first if you cannot read the instructions. Then describe the app in one or two sentences and ask me what I want to change; explain the code and data in detail only when I ask.\n\nWhen making changes:\n- Modify only the project folder above. Follow the platform instructions for everything else. {secrets}\n- {gitSave}\n- {changeSize}\n- {dataConsent}\n- Build according to panda.json when needed. {tools}\n- {reload}\n- {continueChat}\n"),
    (TEMPLATE_MODIFY_AGENT, "Please understand this app first. Do not modify any files yet. This time the goal is to change the app itself, not to use it day to day.\n\n{nameLine}Project folder: {projectRoot}\nPlatform instructions: {skillPath}\nPlatform command line: {cliPath}\n{backendContext}\nFirst read the platform instructions and their references, then panda.json, {instructions} and the documents it mentions. Tell me first if you cannot read the instructions. In one or two sentences, tell me what this app does and what you usually do for me, then ask me what I want to change. If what I ask is actually a daily task (such as adding a topic to follow), follow {instructions} and do not change the code.\n\nWhen making changes:\n- Modify only the project folder above. Follow the platform instructions for everything else. {secrets}\n- {gitSave}\n- {changeSize}\n- {dataConsent}\n- {schema}\n- Before changing {instructions}, tell me what you will change; it holds the rules you follow for my daily tasks.\n- Build according to panda.json when needed. {tools}\n- When finished, tell me to click Reload on the app window. If the database structure changed, remind me to reload once before asking the AI to do daily tasks.\n- {continueChat}\n"),
    (TEMPLATE_CREATE, "I want to make a new app in Panda Drawer. Please first talk with me about what it should do. Do not write code yet.\n\n{nameLine}{descriptionLine}Project folder: {projectRoot} (the platform has already generated a starter project)\nPlatform instructions: {skillPath}\nPlatform command line: {cliPath}\n\nFirst read the platform instructions, their references and the starter project's README. Then ask me in plain words who the app is for and the one thing I most want it to do first. Once that is clear, explain what the first version will include and how you will build it, and wait for my approval before starting.\n\nWhile building:\n- Modify only the project folder above. Follow the platform instructions for everything else. {secrets}\n- {gitSave}\n- {dataConsent}\n- When an API key is needed, declare a configuration field in panda.json so I can fill it in on the platform. Never ask me to send you the key. If the app needs a backend or secrets, add a backend declaration and prepare its files, then run app reload --app <id>; inspect logs with app logs --app <id>.\n- Build according to panda.json when needed. {tools}\n- After each step, tell me to click Reload on the app window to see the result.\n"),
];

pub(crate) const ENGLISH_RULES: [(&str, &str); 9] = [
    ("gitSave", "Save the current version with Git before editing, preserving existing uncommitted changes. If I request a rollback, restore the previous version with Git. Git saves code and does not replace data backups."),
    ("dataConsent", "Before any operation that deletes or clears app data, databases, or uploads, explain the impact and obtain my explicit consent."),
    ("secrets", "Do not read or output real .env files or secrets."),
    ("stdout", "When a backend is declared, stdout is only for platform protocol messages. Write ordinary logs to stderr with console.error; otherwise the platform terminates the app."),
    ("changeSize", "For small changes (wording, colors, layout), say in one sentence what you will change and then do it. For changes that touch data, the database structure or anything larger, explain the plan first and wait for my approval."),
    ("tools", "When you need Node, npm or Git, use the platform command line (env doctor --json and env git doctor --json) to find the ones bundled with the platform."),
    ("reload", "When finished, first check that I have no unsaved edits, then run app reload --app <id> or tell me to click Reload, and explain what you verified. Reload opens the app if it is closed."),
    ("continueChat", "At the end, tell me I can keep asking for changes in this same conversation."),
    ("schema", "When changing the database structure, also update the upgrade logic in the code, docs/database.md and the affected steps in AGENTS.md. Only add columns and tables."),
];

#[path = "ai_prompt_localization.rs"]
mod localization;
pub use localization::localize_handoff;

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
    render_with_rules(template, values, &RULES)
}

pub(crate) fn render_with_rules(template: &str, values: &[(&str, &str)], rules: &[(&str, &str)]) -> String {
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
                let rule: Option<&str> = rules
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
