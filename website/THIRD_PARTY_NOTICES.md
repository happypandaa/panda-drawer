# Third-party notices

本网站直接使用 [Cactus](https://github.com/probberechts/hexo-theme-cactus) 的主题源码、样式和随附资源。字体、字号、文章排版与响应式布局来自原主题；网站内容、站点配置和简体中文栏目名称作本地调整。

## Cactus 来源

- 仓库：<https://github.com/probberechts/hexo-theme-cactus>
- 版本：主题 package.json 标识 3.0.0
- 固定源码提交：`7a6074d830ac5d524582f142ac524c041077aad7`
- 本地位置：`themes/cactus/`；不包含上游 `.git`。
- 许可证：MIT；原始许可证完整保留在 `themes/cactus/LICENSE`，并随静态网站分发为 [cactus-MIT.txt](source/licenses/cactus-MIT.txt)。

- Copyright (c) 2016 Pieter Robberechts
- Copyright (c) 2017 Gabriela Thumé (light colorscheme)
- Copyright (c) 2017 Natalya Kosenko (white colorscheme)

## 随主题分发的第三方资源

以下资源保留原文件与版权注释，不将 Cactus 的 MIT 许可代替各资源自己的许可。即使当前页面未加载某项资源，只要仍随主题分发，就保留其许可。

| 资源与本地目录（相对于 themes/cactus/source/lib/） | 版本与版权 | 许可与完整文本 |
| --- | --- | --- |
| Meslo LG，`meslo-LG/` | 官方 Meslo LG 1.2.1；字体内部版本 1.210。Copyright 2009, 2010, 2013 André Berg | 上游声明 Apache-2.0；[原始 README 与版权声明](source/licenses/meslo-upstream-README.txt)、[Apache-2.0 全文](source/licenses/APACHE-2.0.txt) |
| Font Awesome Free，`font-awesome/` | 6.4.0；Copyright 2023 Fonticons, Inc. | CSS/code：MIT；字体：SIL OFL 1.1；上游完整许可还说明 SVG/JS 图标的 CC BY 4.0（本网站复制的是 CSS 与字体）。[完整许可](source/licenses/font-awesome-free-LICENSE.txt) |
| jQuery，`jquery/` | 3.6.4；Copyright OpenJS Foundation and other contributors | [MIT](source/licenses/jquery-MIT.txt) |
| ClipboardJS，`clipboard/` | 2.0.11；Copyright (c) Zeno Rocha | [MIT](source/licenses/clipboard-MIT.txt) |
| Justified Gallery，`justified-gallery/` | 3.8.1；Copyright (c) 2020 Miro Mannino | [MIT](source/licenses/justified-gallery-MIT.txt) |
| Vazir，`vazir-font/` | 30.1.0；Copyright 2015 The Vazir Project Authors；字体另保留 Saber Rastikerdar 的原始版权元数据 | [SIL OFL 1.1](source/licenses/vazir-OFL.txt) |

ClipboardJS 的浏览器 bundle 同时包含 good-listener、select、delegate（Zeno Rocha，MIT）和 tiny-emitter（Scott Corgan，MIT）。Zeno Rocha 的 MIT 文本见 [clipboard-MIT.txt](source/licenses/clipboard-MIT.txt)；tiny-emitter 的单独版权及 MIT 全文见 [tiny-emitter-MIT.txt](source/licenses/tiny-emitter-MIT.txt)。

Meslo 上游说明及字体二进制同时保留以下继承版权和商标信息：Copyright © 2009 Apple Inc.；Copyright © 2006 by Tavmjong Bah.；Copyright © 2003 by Bitstream, Inc. All Rights Reserved.；Menlo is a Trademark of Apple Inc.；Bitstream Vera is a trademark of Bitstream, Inc., designed by Jim Lyles.

## 许可证原文来源

- Cactus：上述固定提交中的 `LICENSE`。
- Meslo：[上游 README](https://github.com/andreberg/Meslo-Font/blob/master/README.textile)。随附的 12 个 TTF 文件已逐一与该仓库 `dist/v1.2.1/Meslo LG v1.2.1.zip` 中的文件比较，字节一致。
- Apache-2.0：[Apache Software Foundation](https://www.apache.org/licenses/LICENSE-2.0.txt)。
- Font Awesome：[6.4.0 LICENSE.txt](https://github.com/FortAwesome/Font-Awesome/blob/6.4.0/LICENSE.txt)。
- jQuery：[3.6.4 LICENSE.txt](https://github.com/jquery/jquery/blob/3.6.4/LICENSE.txt)。
- ClipboardJS：[v2.0.11 LICENSE](https://github.com/zenorocha/clipboard.js/blob/v2.0.11/LICENSE)。其依赖的作者/许可记录来自对应 npm 包与 README；tiny-emitter 原文来自官方 npm 2.1.0 包。
- Justified Gallery：[3.8.1 LICENSE](https://github.com/miromannino/Justified-Gallery/blob/3.8.1/LICENSE)。
- Vazir：[v30.1.0 OFL.txt](https://github.com/rastikerdar/vazir-font/blob/v30.1.0/OFL.txt)。

`source/licenses/` 下的完整文本会与网站一起生成到 `/licenses/`，以便访问者读取。构建工具依赖的其他许可保留在对应安装包中；此清单针对复制进主题与静态网站的资源。
