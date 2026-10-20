# 私有 Node 与 radio 首次受管试验

更新日期：2026-10-20

用户已授权开始尝试并允许修改 `/Users/ethan/dev/radio-bob`。本次实现限定为 macOS Apple Silicon 的私有运行时准备、实验清单读取、一次性 Rust 受管进程与 radio 源码适配；不是正式应用安装或平台界面交付。固定开发身份、数据根、既有项目格式和 Windows 运行门禁保持。

## 实现内容

| 部分 | 本次行为 |
| --- | --- |
| 私有 Node | 固定官方 Node 24.22.0 / darwin arm64，核验源包及可执行文件摘要；只保留 Node、完整许可证与资源记录，不分发 npm/头文件 |
| 内部资源准备 | `prepare-desktop` 将引擎放入生成资源 `node-runtime/`，开发构建可复用已核验下载缓存；没有系统 PATH 运行时回退 |
| 实验清单 | 严格读取有限大小的声明子集，检查两类条件、字段说明、保留变量、秘密默认值、版本及入口路径；不接管现有安装器 |
| Rust 试验管理器 | 明确绝对路径、固定摘要/版本、空隔离目录、最小环境、实例认证；就绪、HTTP 健康、ping、停止及超时回收 |
| radio 受管入口 | `server/managed.mjs` 在初始化之后创建隔离存储，同进程调用音乐 SDK，无第二个音乐 HTTP 服务；保留原独立启动方式 |
| 应用配置声明 | radio 的实验清单包含 13 个实际使用字段、中文用途和获取步骤；试验例子只使用非秘密默认值，不读取宿主 Key |

平台侧文件为 `node_app_manifest.rs`、`node_runtime_trial.rs`、一次性 example 和运行时准备脚本。模块显式标记 trial，不由现有项目打开、HTTP、CLI 管理接口或普通网页触发。

## 本次使用的试验协议

这组参数固定于本次内部试验，不等同于完整外部包 v2 或公开 SDK 定稿：

- 入口由 `backend.root / entry` 解析，Node 主版本为 24、控制协议版本为 1；完整分发依赖与数据模式未验收。
- 平台通过 stdin 发送 `initialize`，附本次实例 ID 和随机 token；stdout 仅接受受限 NDJSON，单条最多 8192 字节，启动最多 10 秒。
- `ready` 报告当前实例、`127.0.0.1` 和系统分配端口；平台随后请求 `/_platform/health`，使用本次 Bearer 凭据核对同实例本地响应。该路由不调用 AI、TTS 或音乐上游。
- 固定健康响应使用有长度的 JSON；本次 Rust 探测不接受重定向或分块编码，不以此宣称已具备 SSE/音频业务代理。
- `ping / pong` 核对请求和实例；`shutdown / stopping` 之后仍须确认真实退出。优雅停止最多 5 秒，超限进入强制回收并记为异常；正常退出不能只凭收到 `stopping`。
- 标准错误不回放或保留原文，仅做受限读取与计数，避免环境/SDK 日志进入报告。控制通道失联、消息/输出超限有关闭路径。

## 数据与凭据保护

验证只使用明确命名的新临时目录、人工品味/消息和测试凭据；没有读取 radio 原 `.env`、`.data`、`user` 目录中的实际内容，没有启动或停止原 3000/8080 服务。radio 原有独立入口仍使用原目录与配置，本次不重启它来验证。

受管入口的数据、品味、缓存与 SDK 临时状态均放入平台注入的新目录。SDK 在隔离 cwd/tmp 下延迟加载，避开其默认 `.env` 和临时状态来源。受管 HTTP 统一要求实例认证，不对网页暴露 token，不开放自身静态页面；真实音乐登录入口暂时关闭，因为现有账号 Cookie 仍由业务库保存，尚未接入系统凭据设施。

radio 的 4 份原源码在修改前保全于本机忽略目录 `.test-artifacts/node-radio-trial-2026-10-20/radio-source-before/`。radio 本身没有 Git 仓库；本次未初始化 Git，也未把该应用源码混入平台仓库。平台代码按既有提交推送约定保存，本地源备份、下载包、应用包与试验数据不提交。

## 复现方式与边界

仅供平台开发者在本机可丢弃目录复现：先执行 `npm run prepare:desktop` 准备校验后的资源，并构建 `platform-core` 的 `node_runtime_trial` example；再将三个绝对路径交给该 example：私有 Node、radio 源码目录、已存在且为空的 `node-runtime-trial-*` 目录。程序不发现或初始化平台工作区。

已实际在 `PATH=/no-system-node`、父环境仅有该 PATH 的条件下，使用最终 `.app` 内 Node 启动 radio，完成就绪、ping、健康和正常停止；关闭后的监听端口不可连接。试验数据目录同时包含中文和空格。该实验不需要用户电脑预先安装运行 Node，但复现构建仍需要平台开发工具；这不是 Finder 首启验收。

当前清单是源码适配描述：依赖仍在 radio 源根的 `node_modules`，前端仍使用 `/api` 等根路径，不能交给现有平台安装器。尚未实现配置表单、OS 凭据库存储、平台代理/按钮/类型图标、新类型注册、归档恢复保护、硬崩溃孤儿协调或 Windows 进程管理。

音乐 SDK 的底层 axios 请求没有完整 AbortSignal 接入；超时返回不等于上游已取消。在途真实网络、扫码、AI、TTS 与实际播放未测试，不把本次离线控制链路当成业务兼容或账号验证通过。强制终止不表示未提交数据已保存。

## 验证记录

首次联调发现 JSON 默认分块响应与固定健康探测不匹配，已让 radio JSON helper 写入准确 Content-Length，SSE 和音频路径保持原行为。实际内部打包另发现只读权限阻止 Tauri 的扩展属性清理，已调整生成资源为打包所需的 owner 可写权限；旧 `target/<profile>/resources/node-runtime` 副本仍残留旧权限，又阻止 Tauri 覆盖，已增加仅对已知生成节点的权限修复和回归。修复不删除历史证据、不修改内容，拒绝链接、特殊文件及未知成员。运行时只读属于应用契约和摘要检查，不靠 chmod 声称隔离。

独立复核发现主进程回收后继续使用旧 PGID 的归属风险，已修复：状态观察使用 `waitid / WNOWAIT`，在后代清理完成前保留未回收的 leader；每次异步信号在共享归属锁中重新核对，最终回收前永久解除信号权限。重复查询后延迟停止、延迟 Drop、外部回收后拒绝信号的回归通过；第二次独立复核未发现原风险残留。未进行真实 PID 复用压力或宿主硬崩溃恢复实测。

修复后的实际结果：

| 检查 | 结果及证据边界 |
| --- | --- |
| 私有运行时准备测试 | 15 项通过；包含损坏/链接输入、摘要及架构、输出占用、权限和旧生成副本保护。人工二进制夹具不执行 |
| platform-core 完整库测试 | 212 项通过、1 项默认忽略；新清单与受管进程回归包含其中。默认进程夹具用模拟解释器，不能冒充真实 Node 验证 |
| 显式真实 Node 回归 | 被默认忽略的真实 JavaScript 进程用官方私有 Node 单独执行通过 |
| radio 测试 | 使用私有 Node 执行 38 项通过；网络替身/拦截和人工数据不证明真实上游服务或播放 |
| Rust 静态检查 | `platform-core --all-targets` Clippy 且拒绝 warning、全工作区 rustfmt 检查通过 |
| 内部应用构建 | `npm run build:app` 通过，包含前端类型检查/构建及 Rust release；仅生成 `.app`，未安装或替换正在使用的软件 |
| 最终应用包核对 | Node 仍为官方固定摘要，24.22.0 / darwin arm64；完整 LICENSE 与资源记录吻合，应用 `codesign --verify --deep --strict` 及 Node 独立签名检查通过 |
| 最终应用包 → radio | 无系统 Node PATH、中文/空格试验目录下通过 ready、认证 health、ping、stopping；真实退出码 0、未强杀、退出后端口关闭 |

最终 Node 资源包含可执行文件 122,503,472 字节、LICENSE 157,609 字节、资源记录 556 字节，合计 **122,661,637 字节，约 117 MiB**；这是未压缩文件逻辑大小，不是 DMG 下载增量。应用使用内部 ad-hoc 签名，Node 保留官方 Node.js Foundation 签名且二进制摘要未改变。验证机器为 macOS 27.0.1 / Apple Silicon；没有 macOS 15 最低系统、只读安装位置、互联网下载首启、公证、Windows 或真实账户验收。

本机证据保留在忽略目录：`node-radio-trial-2026-10-20/private-node-radio-final-app-run.json` 与 `node-runtime-package-verification-2026-10-20-386gdwye/`，均位于 `.test-artifacts/` 下，不作为发行内容或提交文件。
