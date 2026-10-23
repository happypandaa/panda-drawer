// Keep upstream Cactus typography intact; localize document metadata and
// add only a keyboard-focus and translated mobile-menu target correction.
var siteCssFs = require('node:fs');
var siteCssCrypto = require('node:crypto');
var siteCssVersion = siteCssCrypto.createHash('sha256')
  .update(siteCssFs.readFileSync(hexo.source_dir + 'css/site.css'))
  .digest('hex').slice(0, 12);
hexo.extend.filter.register('after_render:html', function (html) {
  return html
    .replace('<html lang=zh>', '<html lang="zh-CN">')
    .replace('</head>', '<link rel="stylesheet" href="/css/site.css?v=' + siteCssVersion + '"></head>');
});
