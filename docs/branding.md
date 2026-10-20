# 熊猫应用小抽屉：名称与图标

更新日期：2026-10-20

用户已确认产品名称为 **熊猫应用小抽屉**，日常简称 **熊猫抽屉**。名称表达把自己的小应用集中收纳、打开使用；不暗示平台内置 AI 编辑器或所有应用均可离线工作。

当前图标已按用户反馈替换为**圆脸、可爱的熊猫头像**，带轻微歪头和微笑，去掉抽屉、把手和应用方块。产品名称继续使用「熊猫应用小抽屉」，图标不再需要直接描绘名称中的抽屉。图标调整与名称确认不等于正式发行已经验收。

## 显示与固定开发身份

- 安装包产品名、管理界面、关于信息与新生成的 AI 接手说明使用「熊猫应用小抽屉」。窗口、托盘等内部运行入口保留开发版标记。
- 内部生成路径改为 `target/release/bundle/macos/熊猫应用小抽屉.app`；DMG 使用 `熊猫应用小抽屉_<版本>_aarch64.dmg`。
- 固定开发应用标识继续为 `dev.localprojects.platform.m0`，数据根仍为 `~/Library/Application Support/dev.localprojects.platform.m0/`。CLI `local-projects-dev`、SDK `@local-projects/sdk`、协议及项目格式不改名。
- 不改写已有项目的源码、SDK、README、图标或数据库；新品牌不会引起工作区迁移。已有安装也不会被自动重命名、替换或启动。
- 标准打包清理只在受管生成目录中识别新名称与历史 `Local Projects Dev` 两个确切产品名，分别核验同一应用标识、各自包名、可执行文件及签名；运行中的任一版本均阻止覆盖清理。失败仍恢复原生成包。具体边界见[开发说明](development.md)。

## 图标资源

- 主文件：[panda-app-drawer.png](../apps/desktop/src-tauri/icons/panda-app-drawer.png)，1254 × 1254，透明背景。
- 管理界面与 favicon：[panda-app-drawer.png](../apps/desktop/public/panda-app-drawer.png)，256 × 256。
- macOS 使用 `apps/desktop/src-tauri/icons/` 下的 `32x32.png`、`128x128.png`、`128x128@2x.png` 和 `icon.icns`；由 Tauri 图标工具从主文件转换。原 `source.svg` 仅保留作历史资源，不是当前图标的来源。

重新生成尺寸时，先将 Tauri 图标工具输出到单独的 `.test-artifacts/` 子目录，再复制以上所需文件；无需把其他平台的额外产物全部纳入本轮。尺寸转换不重新生成图案，保留透明通道。

## 本轮熊猫头像提示词

工具：内置 `image_gen` 编辑模式，`transparent_background: true`。输入为上一版熊猫抽屉图标；新版沿用现有资源文件名，历史图案保存在 Git 历史中。

```text
Use case: logo-brand
Asset type: desktop application icon for 熊猫应用小抽屉.
Edit target: the provided current panda drawer icon.
Primary request: Replace the entire drawer-shaped object with JUST A CUTE PANDA FACE. A soft round, slightly chubby ivory-white panda head with two round black ears, two friendly rounded black eye patches, bright tiny eyes, a very small natural black nose and a tiny gentle smiling mouth. The head silhouette is round and organic, never rectangular. A little head tilt gives it a playful, lovable expression. Keep a simple polished soft 2.5D illustration style, with clean broad shapes and subtle depth; it should be recognizable and charming at 32px.
Composition: one centered isolated panda head, square canvas, about 10 percent transparent padding on every side. Genuine transparent alpha background, no background tile, no external shadow.
Changes required: completely remove the drawer, cabinet, front panel, opening, handle, carrying straps and the three colored application tiles. The panda is an animal face, not a bag, box, drawer or product with a face.
Constraints: no objects, no accessories, no bamboo, no paws holding anything, no colored tiles, no text, no letters, no watermark, no gritty texture, no loose pixels around the silhouette. Keep the design minimal, friendly, cute and clearly a panda.
```

## 上一版图标提示词（历史记录）

工具：内置 `image_gen`，`transparent_background: true`；未使用 API/CLI 图像生成回退。

```text
Use case: logo-brand
Asset type: production desktop app icon for 熊猫应用小抽屉.
Primary request: A simple, memorable panda app drawer. One small ivory-white rounded drawer cabinet, slightly open, seen nearly from the front with a very slight top view. Two small black rounded panda ears on its upper corners. The front drawer face forms a friendly minimal panda face using two bold black eye patches, tiny white eye highlights and a small black nose shaped drawer pull. Three simple rounded app tiles in muted teal, sky blue and warm apricot peek out of the drawer opening.
Style: crisp vector-like geometric 2.5D icon with very subtle depth, clean surfaces, very few shapes, strong silhouette, friendly everyday utility, readable at 32px. The cabinet and drawer must be recognizable; avoid a separate panda character.
Composition: a single centered isolated icon, square canvas, consistent 10 percent transparent padding all around. Real transparent alpha background. No enclosing app-background tile, no backdrop, no floor or cast shadow outside the object.
Constraints: no letters, no Chinese text, no numbers, no watermark, no code symbols, no sparkles, no decorative scenery, no bamboo, no fur texture.
```

## 上一轮名称与首版图标验证（历史结果）

以下是上一轮名称接入及首版熊猫抽屉图标的验证记录，不能作为本轮熊猫头像调整已经通过检查的依据。

- `npm run check` 通过：资源准备、前端构建、Rust workspace 全目标检查。
- `npm test` 通过：329 项通过、2 项按原设置忽略；不把忽略项计作通过。
- `npm run test:ui`：55 项 DOM 回归通过；不等于原生窗口视觉或系统设备验证。
- `node --test scripts/desktop-package-output-test.mjs`：18 项通过，包含新旧名称精确清理、首次改名失败回滚、双品牌失败恢复及旧包运行中拒绝。
- Rust 格式、差异空白检查通过；256px 前端图标与构建产物一致；32/128/256px 尺寸和透明通道已核对，32px 预览已查看。
- 上一轮验证时 `Local Projects Dev.app` 仍在运行。实际执行 `npm run build:app` 在准备与覆盖旧包前被运行保护拒绝，未停止旧进程、安装或启动新包。

该轮没有生成新的 `.app` / DMG，也未验证中文包名在真实打包后的 Finder、Dock、托盘表现。源码与资源检查不等于已安装版本更新、Windows 运行验证或正式签名公证发行。

## 本轮熊猫头像调整验证

- 内置图像工具编辑完成后，替换主文件、管理界面/favicon PNG，以及 Tauri 的 32/128/256px PNG 和 macOS ICNS；沿用现有引用与资源文件名，不改产品名称、开发身份或数据位置。
- 主文件仍为 1254 × 1254，界面图标为 256 × 256；32/128/256px 各 PNG 的尺寸与透明通道均已核对，32px 放大预览已查看。
- `npm run build --workspace @local-projects/desktop` 通过；构建产物中的 PNG 与界面源文件字节一致。
- `node scripts/rust.mjs check --locked -p platform-desktop --all-targets` 通过。
- `iconutil --convert iconset` 成功解析新 ICNS；差异空白检查通过。

本轮仅替换图标与更新说明，未重跑上一轮完整业务测试，未打包、替换或启动应用，也未验证原生 Dock/托盘的实际显示。不复用上一轮测试结果作为本轮验证。
