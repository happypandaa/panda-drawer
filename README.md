# 熊猫小抽屉

[English](README.en.md) · [GitHub](https://github.com/happypandaa/panda-drawer) · [Gitee](https://gitee.com/mycolorfuldays/panda-drawer)

熊猫小抽屉是一个开源桌面平台，用来收纳、运行和定制自己的小工具。

- **找个工具用。** 从推荐列表挑一个，也可以导入 GitHub、Gitee 仓库或添加本机文件夹。
- **在桌面上运行。** 每个网页应用都有独立窗口，需要后台的应用可以由平台管理 Node.js 服务。
- **做一个自己的工具。** 用你常用的 AI 工具从模板创建应用，或继续修改已有应用的文件夹。

## 从源码运行

项目目前处于开发测试阶段，试用请从源码启动。

- macOS 15 及以上，Apple Silicon。
- Windows 10 22H2 或 Windows 11，x64，需 WebView2 Evergreen。

开发需要 Node.js 22.12+（22.x）或 24.x、[仓库指定版本的 pnpm](https://github.com/happypandaa/panda-drawer/blob/develop/package.json) 和 Rust。macOS 还需要 Xcode 工具链，Windows 需要 MSVC 和 Windows SDK。

```sh
git clone --branch develop https://github.com/happypandaa/panda-drawer.git
cd panda-drawer
pnpm install --frozen-lockfile
pnpm run dev
```

启动后，在 **我的应用 → 添加应用** 试试 Clack 打字练习、拾闻资讯、Readio 英语听读或 Radio Bob 电台。[查看推荐应用](https://github.com/happypandaa/panda-drawer/blob/develop/catalog/apps.json)

应用的构建脚本和 Node.js 后台会在本机执行，请使用可信来源。

## 为熊猫小抽屉开发应用

一个 HTML 页面和一份 `panda.json` 就可以起步。需要本地数据库或附件时，可以使用网页 SDK。[应用格式](https://github.com/happypandaa/panda-drawer/blob/develop/docs/panda-project-format.md) · [AI 开发指南](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/SKILL.md) · [网页 SDK](https://github.com/happypandaa/panda-drawer/blob/develop/packages/sdk/README.md)

欢迎报告问题或讨论改进。[GitHub Issues](https://github.com/happypandaa/panda-drawer/issues)

开发版源码采用 [Apache-2.0](https://github.com/happypandaa/panda-drawer/blob/develop/LICENSE) 许可证，第三方代码与资源遵循各自的许可。
