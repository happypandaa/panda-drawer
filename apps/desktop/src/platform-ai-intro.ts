import { t } from './i18n';
import { PROJECT_AI_REQUIREMENTS } from './project-ai-requirements';
import { defaultPlatformAiDescription } from './frontend-platform';

/**
 * A project-independent introduction for any external AI: used before a new
 * project starts, or to judge whether a project built elsewhere can join.
 * It never carries a project path, ID or diagnostics.
 */
export function buildPlatformAiIntroPrompt(platformVersion?: string, platformDescription = defaultPlatformAiDescription, schemaPath?: string): string {
  const version = platformVersion?.trim() || t("未读取，请以用户实际安装版为准");
  return t("下面是“熊猫小抽屉”平台的介绍和接入要求。请先读完，再根据我接下来说的情况帮我：\n- 如果我准备做一个新项目：按平台的要求帮我规划和开发；\n- 如果我已经有一个项目：判断它能不能放进平台，需要改什么。\n我不懂代码，请用通俗易懂的中文回答，先给结论。这份说明只是背景材料，在我说清需求之前，不要创建或修改任何文件。\n\n【熊猫小抽屉是什么】\n{p0}（当前版本：{p1}）。用户在平台里打开、配置和运行一个个小应用，每个应用有自己的窗口，数据保存在这台电脑上。\n平台负责：启动和停止应用、保存配置和密钥并在启动时注入、管理每个应用的窗口和私有运行设置；文件数据在项目 data/。\n外部 AI 负责：写代码、构建、用 Git 管理版本、出问题时读日志修复。\n\n【平台支持两种应用】\n1. Node 应用：静态网页界面加一个 Node 24 后端服务。适合大多数需要后端逻辑、调用在线 API 或处理文件的项目。平台直接运行项目文件夹里的代码，来自本地或 Git 的 Node 项目都遵循这条规则。\n2. 网页应用：普通 Vite、React 构建结果或纯 HTML 页面，只需在项目根添加 panda.json。可选择平台 SDK 的 SQLite 和附件；不使用 SDK 也可以打开。\n\n【怎么把项目放进平台】\n- 新建项目：平台生成一个网页应用的起始项目。\n- 添加本地项目，选一个空文件夹：平台在里面生成 Node 应用的起始项目（零依赖，可直接运行）。\n- 添加本地项目，选已有文件夹：文件夹可以是符合下面要求的网页或 Node 应用。\n- 从 Git 添加：支持 GitHub、Gitee 的公开 HTTPS 仓库。\n网页 Git 项目已有入口时直接添加；需要构建时，先由用户确认“构建并添加”，平台才执行仓库脚本。\nJSON Schema：{p2}\n\n【请这样回答】\n如果我要做新项目：\nA. 推荐用哪种应用类型，为什么；\nB. 我在平台里先点哪个按钮、选什么文件夹；\nC. 大致的功能拆分、数据怎么存、需要哪些配置项（例如 API Key）；\nD. 会用到的外部软件、在线服务、账号和可能的费用；\nE. 有哪些需求平台目前做不到，替代办法是什么。\n如果我已有项目：\nA. 一句话结论：可以接入／需要修改／当前不推荐／信息不足；\nB. 需要改哪些地方，改动大不大；\nC. 会失去哪些功能；\nD. 我现在只需做的下一步。\n静态判断不能说成“已经验证能运行”。\n\n{p3}", { p0: t(platformDescription), p1: version, p2: schemaPath ? JSON.stringify(schemaPath) : t("由平台项目交接提供实际安装路径，请勿编造。"), p3: t(PROJECT_AI_REQUIREMENTS) });
}
