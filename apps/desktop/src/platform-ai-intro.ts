import { PROJECT_AI_REQUIREMENTS } from './project-ai-requirements';

/**
 * A project-independent introduction for any external AI: used before a new
 * project starts, or to judge whether a project built elsewhere can join.
 * It never carries a project path, ID or diagnostics.
 */
export function buildPlatformAiIntroPrompt(platformVersion?: string): string {
  const version = platformVersion?.trim() || '未读取，请以用户实际安装版为准';
  return `下面是“熊猫小抽屉”平台的介绍和接入要求。请先读完，再根据我接下来说的情况帮我：
- 如果我准备做一个新项目：按平台的要求帮我规划和开发；
- 如果我已经有一个项目：判断它能不能放进平台，需要改什么。
我不懂代码，请用通俗易懂的中文回答，先给结论。这份说明只是背景材料，在我说清需求之前，不要创建或修改任何文件。

【熊猫小抽屉是什么】
macOS 上的本地小应用平台（当前版本：${version}）。用户在平台里打开、配置和运行一个个小应用，每个应用有自己的窗口，数据保存在这台电脑上。
平台负责：启动和停止应用、保存配置和密钥并在启动时注入、为每个应用分配独立的数据目录。
外部 AI 负责：写代码、构建、用 Git 管理版本、出问题时读日志修复。

【平台支持两种应用】
1. Node 应用：静态网页界面加一个 Node 24 后端服务。适合大多数需要后端逻辑、调用在线 API 或处理文件的项目。通过“添加本地项目”接入时，平台直接运行你文件夹里的代码。
2. 网页应用：纯网页界面，通过平台提供的 SDK 使用 SQLite 数据库和附件。只能在平台里用“新建项目”创建，不能把外部文件夹直接接入。

【怎么把项目放进平台】
- 新建项目：平台生成一个网页应用的起始项目。
- 添加本地项目，选一个空文件夹：平台在里面生成 Node 应用的起始项目（零依赖，可直接运行）。
- 添加本地项目，选已有文件夹：文件夹必须已经符合下面的 Node 应用要求。
- 从 Git 添加：支持 GitHub、Gitee 的公开 HTTPS 仓库。
如果是新项目，最省事的做法是先让平台生成起始项目，再在它上面开发；不要先在别处从零写完再改造。

【请这样回答】
如果我要做新项目：
A. 推荐用哪种应用类型，为什么；
B. 我在平台里先点哪个按钮、选什么文件夹；
C. 大致的功能拆分、数据怎么存、需要哪些配置项（例如 API Key）；
D. 会用到的外部软件、在线服务、账号和可能的费用；
E. 有哪些需求平台目前做不到，替代办法是什么。
如果我已有项目：
A. 一句话结论：可以接入／需要修改／当前不推荐／信息不足；
B. 需要改哪些地方，改动大不大；
C. 会失去哪些功能；
D. 我现在只需做的下一步。
静态判断不能说成“已经验证能运行”。

${PROJECT_AI_REQUIREMENTS}`;
}
