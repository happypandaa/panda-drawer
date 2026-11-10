// Keep upstream Cactus typography intact and version the small local style patch.
var siteCssFs = require('node:fs');
var siteCssCrypto = require('node:crypto');
var siteCssVersion = siteCssCrypto.createHash('sha256')
  .update(siteCssFs.readFileSync(hexo.source_dir + 'css/site.css'))
  .digest('hex').slice(0, 12);
hexo.extend.filter.register('after_render:html', function (html) {
  return html
    .replace('</head>', '<link rel="stylesheet" href="/css/site.css?v=' + siteCssVersion + '"></head>');
});
