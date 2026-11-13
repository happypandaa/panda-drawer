// Keep upstream Cactus typography intact and version the small local style patch.
var siteCssFs = require('node:fs');
var siteCssCrypto = require('node:crypto');
var sitePath = require('node:path');
var siteCssVersion = siteCssCrypto.createHash('sha256')
  .update(siteCssFs.readFileSync(hexo.source_dir + 'css/site.css'))
  .digest('hex').slice(0, 12);
hexo.extend.filter.register('after_render:html', function (html) {
  return html
    .replace('</head>', '<link rel="stylesheet" href="/css/site.css?v=' + siteCssVersion + '"></head>');
});

hexo.extend.generator.register('site-light-theme', async function () {
  const path = sitePath.join(hexo.theme_dir, 'source/css/style.styl');
  const render = hexo.extend.renderer.get('styl');
  const context = {
    config: hexo.config,
    theme: { config: { ...hexo.theme.config, colorscheme: 'classic' } },
    execFilterSync: hexo.execFilterSync.bind(hexo)
  };
  const css = await render.call(context, { path, text: siteCssFs.readFileSync(path, 'utf8') }, {});
  return { path: 'css/style-light.css', data: css };
});
