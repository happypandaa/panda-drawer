// Keep upstream Cactus typography intact and version the small local style patch.
var siteCssFs = require('node:fs');
var siteCssCrypto = require('node:crypto');
var sitePath = require('node:path');
var siteCssVersion = siteCssCrypto.createHash('sha256')
  .update(siteCssFs.readFileSync(hexo.source_dir + 'css/site.css'))
  .digest('hex').slice(0, 12);
const screenshotPath = 'images/screens';
const screenshotFilename = /^[a-z0-9][a-z0-9-]*\.(?:webp|png|jpe?g)$/i;

function screenshot(filename, width, height, alt, loading = 'lazy') {
  width = Number(width);
  height = Number(height);
  if (!screenshotFilename.test(filename) || !Number.isSafeInteger(width) || width < 1 ||
      !Number.isSafeInteger(height) || height < 1 || !alt?.trim()) {
    throw new Error('Screenshot requires a filename, width, height and description.');
  }
  const description = hexo.extend.helper.get('escape_html')(alt);
  const viewLabel = (this?.page?.lang ?? this?.lang) === 'en' ? 'View full image: ' : '查看大图：';
  const src = `/${screenshotPath}/${filename}`;
  return `<figure class="site-screenshot" data-screenshot="${filename}"><a href="${src}" target="_blank" rel="noopener" aria-label="${viewLabel}${description}"><img src="${src}" width="${width}" height="${height}" alt="${description}" loading="${loading === 'eager' ? 'eager' : 'lazy'}" decoding="async"></a></figure>`;
}

hexo.extend.helper.register('screenshot', screenshot);
hexo.extend.tag.register('screenshot', function ([filename, width, height, ...description]) {
  return screenshot.call(this, filename, width, height, description.join(' '));
});
hexo.extend.helper.register('alternate_language_page', function () {
  if (this.is_home()) return '/en/';
  const path = (this.page.path ?? '').replace(/^\//, '');
  if (this.page.lang === 'en') return '/' + path.replace(/^en\//, '');
  const target = 'en/' + (path.endsWith('/') ? path + 'index.html' : path);
  return hexo.model('Page').findOne({ path: target, lang: 'en' }) ? '/' + target : undefined;
});
hexo.extend.helper.register('site_prompt', function (prompt) {
  return hexo.extend.highlight.exec(hexo.config.syntax_highlighter, {
    context: hexo,
    args: [prompt, { lang: 'text', lines_length: prompt.split('\n').length }]
  });
});

hexo.extend.filter.register('after_render:html', function (html) {
  // Markdown rendering is cached. Check files only when rendering a full page.
  if (html.includes('</body>')) {
    html = html.replace(/<figure class="site-screenshot" data-screenshot="([^"<>]+)">[^]*?<\/figure>/g, (figure, filename) => {
      const path = sitePath.join(hexo.source_dir, screenshotPath, filename);
      return screenshotFilename.test(filename) && siteCssFs.lstatSync(path, { throwIfNoEntry: false })?.isFile() ? figure : '';
    });
  }
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
