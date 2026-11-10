# 平台中英双语补全实施报告

日期：2026-11-10。目录 `/Users/galen/dev/panda/panda-drawer`，分支 `develop`。实现提交 `2327b34d89f857526cd9f00284c6258d6009e5e2` 已核对同步至 GitHub/Gitee；本报告的最终更新另提交，最终提交号以 Git 历史及交付消息为准。

## 结果与边界

- 没有保存选择时，macOS 通过 CoreFoundation 首选语言、Windows 通过 `GetUserPreferredUILanguages` 读取首选界面语言，zh 开头映射 zh-CN，其余及读取失败映射 en。保存过的有效选择优先；桌面原生结果在 React 挂载前同步给管理器，浏览器独立运行使用 navigator.language。
- 增加清单文字校验与原文保留的显示投影，覆盖卡片、详情、配置与帮助、添加检查和成功页、网络及设备用途。原生窗口保存两个语言的标题名称，切换平台语言直接更新窗口标题，不重载应用页面、不重启后台。
- 推荐列表读取及缓存保留全部翻译，生成输入维护人工英文文案。SDK、环境变量、应用内部文案及应用自有语言设置均未绑定或修改；未启用 SDK 的应用同样支持清单展示翻译。
- 英文 README 与中文互链，skill/格式文档/React 模板文档增加可选本地化示例；没有恢复旧 local-node 模板或要求应用使用特定 i18n 库。
- 官网范围已由用户确认：新增 /en/ 英文入口，页眉、页脚和语言元数据同步英文，中文页面增加 English 链接；保留主题 CSS 和现有站点身份/权限，中文 Wiki、教程和应用详情不翻译。

## 最终字段格式

清单版本仍是 2。localization 可选，只允许 zh-CN/en；语言下 app、configuration、runtimeRequirements 均可选：

```json
{
  "localization": {
    "en": {
      "app": { "name": "Example", "description": "A personal tool." },
      "configuration": {
        "MODE": {
          "label": "Mode", "description": "Choose a mode.",
          "placeholder": "Choose", "howToGet": ["Select the mode you need."],
          "options": { "standard": "Standard" }
        }
      },
      "runtimeRequirements": {
        "network": {
          "connect": {
            "https://api.example.com": { "purpose": "Search", "dataSent": "Search text" }
          }
        },
        "devices": { "camera": { "purpose": "Take a photo" } }
      }
    }
  }
}
```

示例是待合并到完整清单的片段；原清单必须已经声明 MODE 及 standard 选项、对应 connect origin 和 camera。network 另支持 images/media，devices 另支持 microphone。只覆盖名称、简介和用户说明，不更改字段标识、配置默认值/实际值、权限、地址及路径。缺项逐字段回退，howToGet 整数组替换，options 逐 value 覆盖。拒绝未知语言/字段、null、错误类型、重复键、空文字/空步骤、超长文字及不存在的引用。结构约束由 Rust 导出 Schema/前端类型，跨字段引用由运行时校验。

catalog 版本仍是 1，应用可选 i18n.zh-CN/en，每种语言支持可选 name/summary/needs/note。缺项回退基础字段，needs 为完整数组。生成输入提供 name/summary/note 译文；name 可回退清单同语言 app.name，再回退原名；needs 从各必填字段的译文 label/原 label 按字段顺序生成并去重，不接受手写的部分 needs 数组。

## 英文推荐文案待审

| 应用 | 英文名称 | 英文简介草稿 |
| --- | --- | --- |
| 拾闻 | Gleaner | Your AI gathers the news; you read it here. Get daily updates on topics you choose, with bookmarks and feedback kept on your computer. |
| Clack | Clack | Typing practice in Chinese and English: choose a text, check each line as you type, and track your speed by language. |
| Readio | Readio | Learn English through listening and reading: sentence-by-sentence audio, translation and word lookup, with saved sentences and vocabulary. |
| Radio Bob | Radio Bob | Your personal AI radio: request a song in a sentence and hear Bob introduce it in Chinese. |

拾闻备注：Requires an AI tool that can read and write local files and search the web。Readio 备注：Both services charge by usage。

这些文案尚待用户审核。Readio 的“豆包语音 API Key”没有作者清单英文 label，按约定保留原文回退；不在此任务修改外部应用或加入额外翻译规则。推荐条目的英文名称不意味着外部应用自身清单和内部界面已翻译。

## 实际验证

验证按套件串行执行，Node 文件并发 1，Rust 构建作业最多 2，测试线程 2，本地 dev/test debug=0、incremental=0。初始产物 target 5.4 GiB、.test-artifacts 1.0 GiB，扩大验证前检查内存，执行中系统空闲内存最低记录约 41%。未改变 release 参数。

| 检查 | 结果 |
| --- | --- |
| Schema 与 catalog 定向 Node 检查 | 95 通过 |
| 首批本地化/配置/推荐/默认语言定向 UI 检查 | 20 通过 |
| Rust panda_manifest 集成检查 | 26 通过 |
| 原生语言映射及实际本机首选语言读取 | 2 通过 |
| macOS 本地 CI：生成项目名/AI 提示词/Schema 与类型、TypeScript/Vite、SDK、脚本、平台检查、资源准备、Rust 全目标编译 | 均通过；SDK 16 通过；脚本 195 通过、11 按原规则跳过 |
| macOS 本地 CI：完整工作区 Rust 测试 | 840 通过、0 失败、12 按原规则忽略，含 26 项 manifest 集成检查 |
| 修复后的完整 UI 套件 | 351 通过、0 失败，20.26 秒 |
| 修复后 TypeScript 与翻译静态检查 | 均通过；1550 处界面调用、53 文件、83 处原生调用，占位参数一致 |
| 官网 pnpm 构建及静态检查 | 通过；20 页面、702 本地链接/资源/锚点、106 原主题样式/字体文件 |
| 官网本地英文入口 HTTP | /en/ 返回 200；已请求 Codex 打开预览，未将其视为浏览器视觉实测 |

完整 CI 首轮退出 1，仅 All UI regressions 失败；其余步骤全部完成且通过。修复后仅复验发生变化的完整 UI、TypeScript 和翻译检查，均通过，没有重复已通过且代码未变的 Rust 全量套件。当前各项所需检查均已有通过结果，不把首轮 CI 进程称为退出 0。日志位于 /tmp/panda-bilingual-*，不提交本地验证证据。

首轮 UI 套件受新默认语言影响，旧 JSDOM 中文断言在英语默认值下失败；其中等待中文控件的测试子进程已针对本次 PID 终止，没有停止其他应用。已显式固定旧中文夹具的保存语言，新默认语言测试保持独立。随后完整 CI 的 UI 又发现无译文时不应以 manifest 元数据替换接口原展示字段，已修正逐字段回退并新增回归；其余 CI 结果按实际分别记录，不把失败首轮称为整体成功。

## 官网发布

用户确认范围后使用 Sites skill，复用项目 `appgprj_6ad9a35f8a648191b1b674465b9b714b`。读取访问配置确认只有所有者可访问，发布没有改变权限、站点身份或域名。使用 source helper 打开现有站点源码、同步通常目录的已提交官网源码及已检查的静态产物、推送来源并打包，未把平台源码/凭据/业务数据加入站点。沿用源码中已提交的 Qoder 使用说明，因此同步了站点旧源码尚未包含的该段中文说明；本任务没有重写或翻译中文正文。

发布来源提交 `c3c28448ddb4b567bf555e0f19caf5954aa99eac`；部署 `appgdep_6af2b2d1d03481919a2d4a5920ea2ff7`；保存版本 `appgprj_6ad9a35f8a648191b1b674465b9b714b~appgver_cbfd2cf1d4f08191adb6ad96588a2b4a`。原生发布返回 status=succeeded，网址 https://panda-app-drawer.ethan8899.chatgpt.site；英文入口 https://panda-app-drawer.ethan8899.chatgpt.site/en/。本次不是公开官网或安装包发布，不增加下载文件。已请求在 Codex 打开部署后的英文入口，未声称移动端或浏览器视觉实机验收。

完成后移除本次 CI 隔离目录、临时项目、被中止测试的转译夹具和已发布的站点临时 checkout/归档，关闭本次本地预览服务器。.test-artifacts 回到约 1.0 GiB；target 约 5.7 GiB，保留当前编译输出及既有应用包，没有清理历史开发数据或既有验证材料。

## 验收限制与原有工作

未在英文系统干净用户上执行 macOS 全新安装，没有执行真实应用窗口逐屏语言切换验收；原生读取测试和 DOM 测试不能替代这两项。Windows 编译、CI、英文系统安装和实机切换由 Windows 机器另行验收，本机不等待或代称其通过。

未清理、迁移或访问现有应用业务数据，未修改四个外部应用的源码、文案或数据库。保留任务开始时已存在的 AppConfigurationForm.tsx 必填提示、ProjectCard.tsx 统计状态、ProjectDetails.css 样式和 en-apps.ts 翻译改动；这些原有修改不纳入本任务提交。
