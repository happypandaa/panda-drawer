# 熊猫应用小抽屉：名称与图标

更新日期：2026-10-20

用户已确认产品名称为 **熊猫应用小抽屉**，日常简称 **熊猫抽屉**。名称表达把自己的小应用集中收纳、打开使用；不暗示平台内置 AI 编辑器或所有应用均可离线工作。

当前内部代码已接入完整名称和第一版熊猫抽屉图标。图标为内置 `image_gen` 生成的透明 PNG：稍微拉开的白色抽屉、黑色熊猫耳朵与眼斑、鼻子形状的把手，以及三枚应用方块。名称确认不等于图标最终外观或正式发行已经验收。

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

## 内置图像工具提示词

工具：内置 `image_gen`，`transparent_background: true`；未使用 API/CLI 图像生成回退。

```text
Use case: logo-brand
Asset type: production desktop app icon for 熊猫应用小抽屉.
Primary request: A simple, memorable panda app drawer. One small ivory-white rounded drawer cabinet, slightly open, seen nearly from the front with a very slight top view. Two small black rounded panda ears on its upper corners. The front drawer face forms a friendly minimal panda face using two bold black eye patches, tiny white eye highlights and a small black nose shaped drawer pull. Three simple rounded app tiles in muted teal, sky blue and warm apricot peek out of the drawer opening.
Style: crisp vector-like geometric 2.5D icon with very subtle depth, clean surfaces, very few shapes, strong silhouette, friendly everyday utility, readable at 32px. The cabinet and drawer must be recognizable; avoid a separate panda character.
Composition: a single centered isolated icon, square canvas, consistent 10 percent transparent padding all around. Real transparent alpha background. No enclosing app-background tile, no backdrop, no floor or cast shadow outside the object.
Constraints: no letters, no Chinese text, no numbers, no watermark, no code symbols, no sparkles, no decorative scenery, no bamboo, no fur texture.
```

## 本轮验证边界

- `npm run check` 通过：资源准备、前端构建、Rust workspace 全目标检查。
- `npm test` 通过：329 项通过、2 项按原设置忽略；不把忽略项计作通过。
- `npm run test:ui`：55 项 DOM 回归通过；不等于原生窗口视觉或系统设备验证。
- `node --test scripts/desktop-package-output-test.mjs`：18 项通过，包含新旧名称精确清理、首次改名失败回滚、双品牌失败恢复及旧包运行中拒绝。
- Rust 格式、差异空白检查通过；256px 前端图标与构建产物一致；32/128/256px 尺寸和透明通道已核对，32px 预览已查看。
- 当前 `Local Projects Dev.app` 仍在运行。实际执行 `npm run build:app` 在准备与覆盖旧包前被运行保护拒绝，未停止旧进程、安装或启动新包。

本轮没有生成新的 `.app` / DMG，也未验证中文包名在真实打包后的 Finder、Dock、托盘表现。源码与资源检查不等于已安装版本更新、Windows 运行验证或正式签名公证发行。
