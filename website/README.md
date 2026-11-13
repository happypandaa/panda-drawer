# 熊猫小抽屉官网

独立的 Hexo 静态网站，包含中英文入口、Wiki、两篇教程，以及拾闻、Clack、Readio 和 Radio Bob 的介绍。内容依据平台资料和应用作者发布的版本。

中英文入口互链。英文页使用英文标题、页眉、页脚、菜单及语言元数据，链接到中文指南和英文项目 README。

## 开发与检查

需要 Node 22.12+（推荐 Node 24）和 pnpm，在此目录执行：

```sh
pnpm install --frozen-lockfile --ignore-scripts
pnpm run build
pnpm run check
pnpm run dev
```

本地预览为 `http://127.0.0.1:4173/`。`dist/` 是可重新生成的部署产物，未提交；平台自己的构建和运行不依赖本站。本站使用独立的依赖锁文件，不改变根工程依赖。

`pnpm run check` 校验必需页面、站内链接与锚点、本地资源、文章目录与移动工具栏、两种 AI 操作内容、五个规则页的日期与来源、`llms.txt` 的完整地址、使用指南的用词，以及原版 CSS/字体摘要。浏览器实际交互和视觉检查单独记录，不把静态检查当作移动设备实测。

## 内容维护

- `source/_posts/`：教程，使用 Cactus 原生文章布局。Markdown 代码块自动带原主题复制入口。
- `source/wiki/`：入门、AI 交接、常见问题，以及面向 AI／作者的五个规则与格式页面。
- `source/apps/`：四款推荐应用的用法、配置和数据说明。
- `source/en/index.md`：英文介绍、系统要求和四款应用的英文概览。
- `source/_data/projects.json`：首页应用列表，简介与根工程 `catalog/apps.json` 的 summary 一致。
- `source/llms.txt`：供 AI 读取的页面索引，使用站点完整网址；在 `skip_render` 中原样输出。
- `_config.yml`：站点信息、导航与 Cactus 配置。

使用指南写给普通用户，以当前界面名称说明添加、配置、使用和修改；技术细节链接到规则页。规则与格式包含平台规则、panda.json、应用类型与运行方式、数据与配置、添加要求五页，各页顶部标注日期与来源。

规则的事实来源为当前 `develop` 的 `skills/local-projects-app/SKILL.md`、对应 `references/`、`schemas/panda.schema.json`、`platform-prompt.ts` 与实际实现代码。不要从根工程历史 `docs/` 或 README 日期记录取规则。本机安装的平台说明优先；发现 skill 与代码不同，按代码实际行为描述并记录差异。

完整 panda.json 示例位于规则页，维护时须使用根工程的 `schemas/panda.schema.json` 实际校验，不把控制协议消息当作项目清单。官网独立发布目录不包含根工程 Schema，Schema 校验从根工程执行并记录结果。公开下载未提供时不构造下载地址，版本细节集中在下载页，帮助页的「当前版本」保留入口。

## 更新截图

把用于发布的截图存到 `source/images/screens/`。这个目录由 Git 忽略，截图更新时覆盖同名文件。若裁切后的尺寸发生变化，同时更新引用处的宽高。截图采用小写英文文件名，包含字母、数字和连字符。该目录会随站点公开，请只放已检查的图片，截图里的个人路径、账号信息和密钥需要先裁掉或遮住。

在 Markdown 中用 `{% screenshot s02-add-app.webp 1440 810 添加应用面板下方的推荐应用区域 %}`。首页 EJS 可用 `<%- screenshot('s02-add-app.webp', 1440, 810, '添加应用面板下方的推荐应用区域') %>`。页面上的截图可点开查看大图。图像缺失时页面会继续显示文字步骤。更新后刷新本地预览，发布前重新构建。

## Cactus 主题

直接随源码保留 [Cactus](https://github.com/probberechts/hexo-theme-cactus) 的固定提交 `7a6074d830ac5d524582f142ac524c041077aad7`。`themes/cactus/source/css/` 与 Meslo 字体保持原始内容，不重写主题。默认暗色、正文 14px、行高 1.725、页面宽 48rem、Menlo/Meslo 字体栈均为主题默认。中文沿用原版系统字形回退。

本地调整包括中英文站点导航、滚动时固定的共用页眉、站点元数据、代码复制按钮的键盘操作和焦点样式。本站为从左到右排版，不生成未使用的 RTL 样式，原始文件仍保留。原主题及内含字体、脚本的许可证保留在主题与 `source/licenses/` 中，见 `THIRD_PARTY_NOTICES.md`。

## 发布

Sites 身份保存在 `.openai/hosting.json`；复用同一身份，不能再新建替代站点。现有站点当前仅所有者可访问，更新保持原权限。对公众开放或绑定自有域名需另行配置，不把私有站点更新当作公开官网发布完成。

原仓库的官网源码与锁文件按项目协作要求提交，并同步推送 GitHub 和 Gitee；分别核对远端分支。Sites 发布只同步此目录的纯网站源码到独立发布工作目录，再构建并打包 `dist/`，避免把平台源码、未完成改动、数据、安装包或验证证据提交到站点仓库。凭据只通过发布工具的标准输入传入，不写入文件。

在独立发布工作目录构建时，先将已审核截图复制到该工作目录的 `source/images/screens/`。构建会把图片输出到 `dist/images/screens/`，由部署包一起发布。Git 检出本身只包含网页和图片引用，截图素材随发布工作目录单独维护。
