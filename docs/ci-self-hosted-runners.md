# 自托管 CI 运行器

2026-10-31：Windows x64 移植只使用用户自己的 Mac 和 Windows 电脑，绝不选择 GitHub 云端运行器。工作流在 push、同仓库 PR 和手动触发时执行，两项标签分别为 `[self-hosted, macOS, ARM64]` 和 `[self-hosted, Windows, X64]`。外部 fork 的代码不会自动在这两台真实电脑上运行。

## Mac 安装步骤

1. 需要 macOS 15+ Apple Silicon、Xcode Command Line Tools、Rust stable、Node 24/npm 和 Git。先运行 `xcode-select -p`、`rustc -V`、`cargo -V`、`node -v`、`npm -v`。使用专门的 CI 工作目录，不要把 runner 的 `_work` 指向日常开发 checkout、PandaDrawer 工作区或平台数据根。建议使用独立的本机 CI 用户。
2. 在仓库的 **Settings → Actions → Runners → New self-hosted runner** 中选择 **macOS / ARM64**。只使用这个页面给出的 `actions/runner` 官方下载地址和校验值；不要从镜像下载。下载和解压至独立目录，例如 `~/actions-runner-panda`：

   ```sh
   mkdir -p ~/actions-runner-panda
   cd ~/actions-runner-panda
   # 在这里逐条执行 GitHub 页面当前给出的下载、SHA256 校验和解压命令。
   ```

3. 在同一目录执行页面给出的注册命令，增加明确名称；保留默认平台标签。下面的 token 是占位符，替换为该页面的一小时临时注册 token，不要把 token 提交到仓库或发到聊天里：

   ```sh
   ./config.sh --url https://github.com/happypandaa/panda-drawer \
     --token REGISTRATION_TOKEN --name panda-macos-arm64 --work _work
   ```

4. 为当前登录用户安装和启动 launchd 后台服务：

   ```sh
   ./svc.sh install
   ./svc.sh start
   ./svc.sh status
   ```

   服务的 PATH 必须能找到 Node 24、npm、Git、cargo 和 rustc。Homebrew/用户 Rust 不在后台 PATH 时，在 runner 根目录的 `.env` 中设置 `PATH`，并设置 `CARGO_HOME` 和 `RUSTUP_HOME` 的真实绝对路径，然后重启服务。也可用 `PANDA_CI_RUST_BIN` 指定已有 Rust 的 `bin` 目录。不要把这些机器专属路径或 `.credentials*` 提交到仓库。

5. 接通电源，在 **系统设置 → 电池/节能 → 选项** 中开启“使用电源适配器供电且显示器关闭时，防止自动进入睡眠”（文案随系统版本不同）。显示器可以熄灭，电脑不能自动睡眠；MacBook 保持能继续运行的开盖/外接显示器状态。也可按自己原有电源策略临时运行 `caffeinate -is`，该命令运行期间保持电脑唤醒。
6. 回到 GitHub 的 Runners 页面，确认 `panda-macos-arm64` 为 **Idle**，标签包含 `self-hosted`、`macOS`、`ARM64`。排队检查随后自动开始，不需要手动重新触发每个提交。

## Windows 安装与服务

Windows 运行器放在独立的 `C:\actions-runner-panda`，或另一个服务账户可访问的专用目录。需要 Rust `x86_64-pc-windows-msvc`、VS C++ Build Tools、Windows SDK、Node 24/npm、Git 和 WebView2。安装和注册步骤由 Windows 侧执行；安装服务需要管理员 PowerShell，注册需要仓库管理员权限的一小时临时 token。

在上述 GitHub 页面选择 **Windows / x64**，执行页面提供的官方 ZIP 下载、SHA256 校验和解压命令，再执行：

```powershell
.\config.cmd --unattended --url https://github.com/happypandaa/panda-drawer `
  --token REGISTRATION_TOKEN --name panda-windows-x64 --work _work --runasservice
Get-Service 'actions.runner.*'
```

保留默认的 `self-hosted`、`Windows`、`X64` 标签。不要把日常平台工作区作为 `_work`。服务运行账户需要访问编译工具；仓库本地 `.tools` Rust 不会自动出现在新 checkout 中，应在 runner 的 `.env` 配置既有 `CARGO_HOME`、`RUSTUP_HOME`、`PANDA_CI_RUST_BIN`。不能因为注册缺少权限就宣称服务已安装。

本仓库提供 `scripts/install-windows-runner.ps1`：固定官方 `actions/runner` **2.338.2**，校验该 release asset 的 SHA256 `a1f17aa987fc8158bc7521d284912267a0d01b7c2c4684b33c86783b5e8221cc`，拒绝覆盖已注册运行器。管理员 PowerShell 可从只保存在本机的临时 token 文件注册：

```powershell
.\scripts\install-windows-runner.ps1 -RegistrationTokenFile C:\local-temp\runner-token.txt `
  -CargoHome C:\actions-runner-panda\cargo-cache -RustupHome D:\dev\postgresUI\.tools\rustup `
  -RustBin D:\dev\postgresUI\.tools\cargo\bin
```

先创建 `cargo-cache` 目录。`-RustBin` 允许服务只读取现有 Rust 程序与 rustup 工具链，把 Cargo 下载和可写缓存放在独立 runner 目录，不授予服务对日常开发 Cargo 缓存的写权限。`-PrepareOnly` 只下载、校验、解压和核对版本，不要求管理员或 token，不表示服务已安装。注册 token 由子进程环境传入，不写入文档、脚本参数或仓库。注册完成后由提供者处理本机临时 token 文件；脚本不删除任意用户文件。

## 基线、检查与无人值守推进

工作流用当前提交的 CI harness 检查独立的 source checkout。`scripts/ci.mjs` 为测试建立唯一 `.test-artifacts/self-hosted-ci-*`，把测试子进程的 HOME、USERPROFILE、LOCALAPPDATA、APPDATA、XDG 目录及临时目录指向其中；保留原来的 Rust 安装位置。不会启动开发桌面程序或调用操作真实开发项目的 live/restart 脚本，不增加产品的任意数据根覆盖入口。

检查包括锁定依赖安装、参考文件校验、TypeScript/Vite、完整 UI 和 SDK 测试、现有脚本单元测试、桌面资源准备，以及 workspace 全目标 Rust 编译和完整 Rust 测试。一个步骤失败后继续采集其余步骤，最终总状态仍为失败；不删除、放宽或临时忽略 Mac 测试。各测试中的既有平台条件和原本的 ignored 保持原样。

重构前基线固定为 `ac3b5d7`（完整 SHA 见验证记录）。CI 配置在新分支提交后，可在该分支手动触发工作流，`source_ref` 填原始提交完整 SHA：

```sh
gh workflow run self-hosted.yml --ref win/w0-ci -f source_ref=BASELINE_FULL_SHA
gh run list --branch win/w0-ci --workflow self-hosted.yml
gh run watch RUN_ID --exit-status
gh run view RUN_ID --log-failed
```

新工作流尚未进入默认分支时，GitHub 可能不允许 workflow_dispatch；此时使用分支 push 自动检查未改产品代码的首个 CI 提交作为基线，明确记录它相对原始 main 只有 CI/文档变更，不能把“排队”记为基线通过。原始提交检查可在默认分支具备该工作流后补跑。

Mac 离线时保留队列，在当前未合并分支上叠加下一里程碑继续；两套检查绿后按顺序变基、复核、快进合入 main，再删除对应短分支。排队超过 24 小时记录并提醒用户。不为了消除排队改用云端运行器，不把本机探针当作完整 CI。

官方依据：[添加运行器](https://docs.github.com/en/actions/how-tos/manage-runners/self-hosted-runners/add-runners)、[后台服务](https://docs.github.com/en/actions/how-tos/manage-runners/self-hosted-runners/configure-the-application?platform=mac)。


## 项目依赖安装工具

CI 使用 `package.json.packageManager` 固定的 pnpm（当前 `12.16.1`）与 `pnpm-lock.yaml` 执行 `pnpm install --frozen-lockfile`。运行机须预先安装该版本；CI 不通过 npm 安装或引导包管理器。仓库根 `package-lock.json` 不再作为开发依赖安装的依据，应用项目自己的 npm 锁文件与平台受管 npm 运行约定不受影响。

Windows 运行机须让 PATH 包含 pnpm 的 `node_modules/pnpm/bin/pnpm.cjs` 所在安装前缀或独立 `pnpm.exe`。也可将 `PANDA_CI_PNPM_CLI` 指向实际的 `pnpm.cjs` 或可执行文件；不要指向 `pnpm.cmd`。macOS 可从 PATH 的 `pnpm` 解析实际脚本。版本不匹配时 CI 明确失败，不自动安装其他版本。生成的 `panda.json` Schema 和前端类型同样由 CI 的 `test:panda-manifest` 检查。
