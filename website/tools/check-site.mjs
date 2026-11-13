import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { JSDOM } from 'jsdom';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const output = path.join(root, 'dist');
const origin = new URL(readFileSync(path.join(root, '_config.yml'), 'utf8').match(/^url: (.+)$/m)[1]).origin;
const files = (dir) => readdirSync(dir).flatMap(name => {
  const file = path.join(dir, name);
  return statSync(file).isDirectory() ? files(file) : [file];
});
assert(existsSync(output), 'Build the site before running this check.');
const allFiles = files(output);
const htmlFiles = allFiles.filter(file => file.endsWith('.html'));
const documents = new Map(htmlFiles.map(file => [file, new JSDOM(readFileSync(file, 'utf8')).window.document]));
const rulePages = ['platform-rules', 'panda-json', 'app-types', 'data-and-config', 'adding-apps'];
const localizedPages = [
  'index.html', 'download/index.html', 'licenses/index.html', 'wiki/index.html', 'wiki/getting-started/index.html',
  'wiki/how-it-works/index.html',
  'wiki/ai-workspace/index.html', 'wiki/faq/index.html',
  'tutorials/index.html', 'tutorials/create-your-app/index.html',
  'tutorials/remix-an-app/index.html', 'apps/index.html', 'apps/clack/index.html',
  'apps/readio/index.html', 'apps/radio/index.html', 'apps/gleaner/index.html',
  ...rulePages.map(slug => `wiki/${slug}/index.html`)
];
const required = [...localizedPages, ...localizedPages.map(file => `en/${file}`), '404.html'];
for (const file of required) assert(documents.has(path.join(output, file)), `Missing page: ${file}`);
assert(!existsSync(path.join(output, 'apps/radio/install/index.html')), 'Obsolete Radio install page is still generated.');

const llmsPath = path.join(output, 'llms.txt');
assert(existsSync(llmsPath), 'Missing llms.txt');
const llms = readFileSync(llmsPath, 'utf8');
for (const slug of rulePages) {
  assert(llms.includes(`${origin}/wiki/${slug}/`), `llms.txt is missing rule page: ${slug}`);
  assert(llms.includes(`${origin}/en/wiki/${slug}/`), `llms.txt is missing English rule page: ${slug}`);
  const document = documents.get(path.join(output, 'wiki', slug, 'index.html'));
  const content = document.querySelector('article.post .content');
  assert(content, `Rule page needs article content: ${slug}`);
  assert.equal(content.querySelectorAll('h1').length, 1, `Rule page needs one main heading: ${slug}`);
  assert(/更新.*\d{4}-\d{2}-\d{2}/.test(content.textContent), `Rule page needs an update date: ${slug}`);
  const sources = [...content.querySelectorAll('a[href]')].map(link => link.href);
  assert(sources.some(href => /^https:\/\/github\.com\/happypandaa\/panda-drawer\/blob\/develop\/(?:skills\/local-projects-app\/|schemas\/panda\.schema\.json)/.test(href)), `Rule page needs a current skill or Schema source: ${slug}`);
}

let checkedLinks = 0;
function checkLink(value, base, label, checkAnchor = false) {
  if (!value || /^(data:|mailto:|tel:|javascript:)/.test(value) || value === '#') return;
  const url = new URL(value, base);
  if (url.origin !== origin) return;
  const relative = decodeURIComponent(url.pathname).replace(/^\/+/, '');
  const exact = path.resolve(output, relative);
  assert(exact.startsWith(`${output}${path.sep}`) || exact === output, `Path escapes output: ${label}`);
  const target = existsSync(exact) && statSync(exact).isFile() ? exact : path.join(exact, 'index.html');
  assert(existsSync(target), `Broken internal link in ${label}: ${value}`);
  if (checkAnchor && url.hash && documents.has(target)) {
    const id = decodeURIComponent(url.hash.slice(1));
    const document = documents.get(target);
    assert(document.getElementById(id) || document.getElementsByName(id).length, `Missing anchor in ${label}: ${value}`);
  }
  checkedLinks++;
}

for (const [file, document] of documents) {
  const relative = path.relative(output, file).split(path.sep).join('/');
  const base = new URL(relative, `${origin}/`);
  const isEnglish = relative.startsWith('en/');
  assert.equal(document.documentElement.lang, isEnglish ? 'en' : 'zh-CN', `Wrong language: ${relative}`);
  assert(document.title && document.querySelector('h1'), `Page needs a title: ${relative}`);
  assert(document.querySelector('meta[name="description"]')?.content, `Page needs a description: ${relative}`);
  const headerLinks = ['/', '/apps/', '/tutorials/', '/wiki/', '/download/'].map(href => isEnglish ? `/en${href}` : href);
  for (const href of headerLinks) {
    assert(document.querySelector(`#header #site-nav a[href="${href}"]`), `Missing header link ${href}: ${relative}`);
  }
  for (const element of document.querySelectorAll('a[href], link[href], script[src], img[src]')) {
    checkLink(element.getAttribute('href') ?? element.getAttribute('src'), base, relative, element.tagName === 'A');
    if (element.tagName === 'IMG') assert(element.hasAttribute('alt'), `Missing image alt: ${relative}`);
  }
  for (const element of document.querySelectorAll('script[src],link[rel="stylesheet"],img[src]')) {
    const url = new URL(element.getAttribute('src') ?? element.getAttribute('href'), base);
    assert.equal(url.origin, origin, `Unexpected third-party page asset: ${url}`);
  }
  for (const screenshot of document.querySelectorAll('.site-screenshot img')) {
    assert(screenshot.alt.trim() && screenshot.width > 0 && screenshot.height > 0, `Incomplete screenshot metadata: ${relative}`);
    if (isEnglish) assert(screenshot.parentElement.getAttribute('aria-label')?.startsWith('View full image'), `Untranslated image link: ${relative}`);
  }
  if (isEnglish) {
    for (const link of document.querySelectorAll('article a[href]')) {
      const url = new URL(link.href, base);
      if (url.origin !== origin) continue;
      const target = url.pathname.replace(/^\//, '') + (url.pathname.endsWith('/') ? 'index.html' : '');
      if (documents.has(path.join(output, target))) assert(url.pathname.startsWith('/en/'), `English content links to a Chinese page: ${relative} → ${url.pathname}`);
    }
  }
  assert(!/\/Users\/|prj_[a-f\d]{20}|TODO|lorem ipsum/i.test(document.body.textContent), `Private path or placeholder in ${relative}`);
}

const english = documents.get(path.join(output, 'en/index.html'));
const chinese = documents.get(path.join(output, 'index.html'));
assert(chinese.querySelector('#writing .highlight table .code'), 'Homepage needs a copyable prompt example.');
const downloads = documents.get(path.join(output, 'download/index.html'));
assert(downloads.getElementById('系统要求') && /正式安装包仍在准备中/.test(downloads.body.textContent));
assert(!downloads.querySelector('a[href$=".dmg"], a[href$=".exe"], a[href$=".msi"], a[href$=".pkg"]'));
for (const slug of ['/download/', '/wiki/how-it-works/']) {
  assert(llms.includes(`${origin}${slug}`), `llms.txt is missing ${slug}`);
}
assert.equal(english.title, 'Panda Drawer', 'English entrance needs an English page title.');
assert.equal(english.querySelector('#header .p-name')?.textContent, 'Panda Drawer');
assert.equal(english.querySelector('#site-nav')?.getAttribute('aria-label'), 'Main navigation');
assert.equal(english.querySelector('meta[property="og:locale"]')?.content, 'en_US');
assert.equal(english.querySelector('meta[property="og:site_name"]')?.content, 'Panda Drawer');
assert(!/[\p{Script=Han}]/u.test(english.querySelector('meta[name="description"]').content), 'English metadata must not use the Chinese site description.');
assert(english.querySelector('#about a[href^="/en/apps/clack/"]'), 'English entrance needs a link to the first Clack practice.');
assert.equal(english.querySelectorAll('#projects .project-item').length, 4, 'English entrance needs all four recommended apps.');
assert(english.querySelector('.app-workflow[aria-label^="Panda Drawer opens"]'), 'English entrance needs a localized workflow diagram.');
assert(english.querySelector('#writing .highlight table .code') && english.querySelector('script[src*="clipboard"]'), 'English entrance needs a copyable prompt example.');
if (existsSync(path.join(output, 'images/screens/s01-my-apps-en.webp'))) {
  assert(english.querySelector('.site-screenshot a[aria-label^="View full image"] img[src$="s01-my-apps-en.webp"]'), 'English entrance needs the English overview screenshot when present.');
}
assert(/Public installers are not available yet/.test(english.querySelector('article').textContent), 'English entrance must state the actual download status.');
assert(/macOS 15/.test(english.querySelector('article').textContent) && /Windows 10 22H2/.test(english.querySelector('article').textContent), 'English entrance needs the current system requirements.');
assert(/manage their own language independently/.test(english.querySelector('article').textContent), 'English entrance must preserve independent app language.');
assert(/Copyright.*Panda Drawer/s.test(english.querySelector('#footer').textContent), 'English footer must use the English brand.');
for (const file of localizedPages) {
  const chinesePath = '/' + file.replace(/index\.html$/, '');
  const englishPath = '/en' + chinesePath;
  for (const [prefix, language, alternatePath] of [['', 'en', englishPath], ['en/', 'zh-CN', chinesePath]]) {
    const document = documents.get(path.join(output, prefix + file));
    for (const selector of ['#header .site-tools a', '#footer a']) {
      assert(document.querySelector(`${selector}[href="${alternatePath}"][hreflang="${language}"]`), `Missing language switch on ${prefix + file}`);
    }
    for (const [lang, url] of [['zh-CN', chinesePath], ['en', englishPath]]) {
      assert(document.querySelector(`link[rel="alternate"][hreflang="${lang}"][href="${origin + url}"]`), `Wrong alternate metadata on ${prefix + file}`);
    }
  }
}
assert(![...english.querySelectorAll('a[href]')].some(link => /\.(?:dmg|exe|msi|pkg)(?:$|[?#])/i.test(link.getAttribute('href'))), 'Do not invent installer downloads for the English entrance.');

const userGuides = [
  'index.html', 'wiki/getting-started/index.html', 'wiki/ai-workspace/index.html',
  'wiki/faq/index.html', 'tutorials/index.html', 'tutorials/create-your-app/index.html',
  'tutorials/remix-an-app/index.html', 'apps/index.html', 'apps/clack/index.html',
  'apps/readio/index.html', 'apps/radio/index.html', 'apps/gleaner/index.html'
];
for (const file of userGuides) {
  const document = documents.get(path.join(output, file));
  const content = document.querySelector('article.post') ?? document.querySelector('.content.index');
  const prose = `${content.textContent} ${document.querySelector('meta[name="description"]').content}`;
  assert(!/ZIP|部分版本|仓库|构建|\bNode\b|清单|验收|核对了|需要先适配|\bGit\b|Schema|依赖|后端|源码|网页应用/i.test(prose), `Technical or historical wording in user guide: ${file}`);
}

for (const file of allFiles.filter(file => file.endsWith('.css'))) {
  const relative = path.relative(output, file).split(path.sep).join('/');
  for (const match of readFileSync(file, 'utf8').matchAll(/url\(\s*['"]?([^)'"\s]+)['"]?\s*\)/g)) {
    checkLink(match[1], new URL(relative, `${origin}/`), relative);
  }
}

for (const slug of ['create-your-app', 'remix-an-app']) {
  const document = documents.get(path.join(output, 'tutorials', slug, 'index.html'));
  assert(document.querySelector('article.post .posttitle'), `Not using Cactus post layout: ${slug}`);
  assert(document.querySelector('#toc .toc-link'), `Missing Cactus article TOC: ${slug}`);
  assert(document.querySelector('.highlight table .code'), `Missing copyable highlighted example: ${slug}`);
  assert(document.querySelector('#footer-post #actions-footer'), `Missing mobile article toolbar: ${slug}`);
  assert(/Codex/.test(document.querySelector('article').textContent) && /WorkBuddy/.test(document.querySelector('article').textContent));
  const englishTutorial = documents.get(path.join(output, 'en', 'tutorials', slug, 'index.html'));
  assert(englishTutorial.querySelector('.highlight table .code') && englishTutorial.querySelector('script[src*="clipboard"]'), `English tutorial needs a copyable prompt: ${slug}`);
  assert(englishTutorial.querySelector('#toc .toc-link') && englishTutorial.querySelector('#footer-post #actions-footer'), `English tutorial needs a table of contents and mobile toolbar: ${slug}`);
  for (const link of englishTutorial.querySelectorAll('#header-post #nav a, #nav-footer a')) {
    assert(link.getAttribute('href').startsWith('/en/'), `English tutorial navigation needs localized links: ${slug}`);
  }
}

const integrity = JSON.parse(readFileSync(path.join(root, 'theme-integrity.json'), 'utf8'));
for (const [relative, expected] of Object.entries(integrity.sha256)) {
  assert.equal(createHash('sha256').update(readFileSync(path.join(root, 'themes/cactus', relative))).digest('hex'), expected, `Upstream style/font modified: ${relative}`);
}
console.log(`Checked ${htmlFiles.length} pages, ${checkedLinks} local links/assets/anchors and ${Object.keys(integrity.sha256).length} original Cactus style/font files.`);
