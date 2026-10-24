# 熊猫小抽屉官网

独立的 Hexo 静态网站，包含首页、Wiki、两篇教程、应用列表，以及 Radio Bob 的介绍与安装方法。只读取平台资料来编写内容，不连接平台数据、密钥或本机应用服务。

## 开发与检查

需要 Node 22.12+（推荐 Node 24）和 npm，在此目录执行：

```sh
npm ci
npm run build
npm run check
npm run dev
```

本地预览为 `http://127.0.0.1:4173/`。`dist/` 是可重新生成的部署产物，未提交；平台自己的构建和运行不依赖本站。本站使用独立的依赖锁文件，不改变根工程依赖。

`npm run check` 校验必需页面、站内链接与锚点、本地资源、文章目录与移动工具栏、两种 AI 操作内容，以及原版 CSS/字体摘要。浏览器实际交互和视觉检查单独记录，不把静态检查当作移动设备实测。

## 内容维护

- `source/_posts/`：教程，使用 Cactus 原生文章布局。Markdown 代码块自动带原主题复制入口。
- `source/wiki/`：知识库与常见问题。
- `source/apps/`：应用列表、Radio 介绍与安装。
- `source/_data/projects.json`：首页应用列表。
- `_config.yml`：站点信息、导航与 Cactus 配置。

文案依据仓库已确认文档与代码，公共下载未提供时明确说明，不构造下载链接。Codex 与 WorkBuddy 的适用版本、验证范围分别说明；不将 Windows、干净电脑或真实语音等未验场景写成通过。

## Cactus 主题

直接随源码保留 [Cactus](https://github.com/probberechts/hexo-theme-cactus) 的固定提交 `7a6074d830ac5d524582f142ac524c041077aad7`。`themes/cactus/source/css/` 与 Meslo 字体保持原始内容，不重写主题。默认暗色、正文 14px、行高 1.725、页面宽 48rem、Menlo/Meslo 字体栈均为主题默认。中文沿用原版系统字形回退。

本地调整包括中文导航翻译、站点元数据、把代码复制图标改为可用键盘操作的按钮，以及 `source/css/site.css` 中的焦点和中文移动菜单点击区域修正。本站为从左到右排版，不生成未使用的 RTL 样式，原始文件仍保留。原主题及内含字体、脚本的许可证保留在主题与 `source/licenses/` 中，见 `THIRD_PARTY_NOTICES.md`。

## 发布

Sites 身份保存在 `.openai/hosting.json`；复用同一身份，不能再新建替代站点。当前新站默认仅所有者可访问。对公众开放或绑定自有域名需另行配置，不把预览当作公开官网发布完成。

原仓库的官网源码与锁文件按项目协作要求提交推送。Sites 发布只同步此目录的纯网站源码到独立发布工作目录，再构建并打包 `dist/`，避免把平台源码、未完成改动、数据、安装包或验证证据提交到站点仓库。凭据只通过发布工具的标准输入传入，不写入文件。
