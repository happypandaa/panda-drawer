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
const required = [
  'index.html', 'wiki/index.html', 'wiki/getting-started/index.html',
  'wiki/ai-workspace/index.html', 'wiki/app-types/index.html', 'wiki/faq/index.html',
  'tutorials/index.html', 'tutorials/create-your-app/index.html',
  'tutorials/remix-an-app/index.html', 'apps/index.html', 'apps/radio/index.html',
  'apps/radio/install/index.html', '404.html'
];
for (const file of required) assert(documents.has(path.join(output, file)), `Missing page: ${file}`);

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
  assert.equal(document.documentElement.lang, 'zh-CN', `Wrong language: ${relative}`);
  assert(document.title && document.querySelector('h1'), `Page needs a title: ${relative}`);
  assert(document.querySelector('meta[name="description"]')?.content, `Page needs a description: ${relative}`);
  for (const element of document.querySelectorAll('a[href], link[href], script[src], img[src]')) {
    checkLink(element.getAttribute('href') ?? element.getAttribute('src'), base, relative, element.tagName === 'A');
    if (element.tagName === 'IMG') assert(element.hasAttribute('alt'), `Missing image alt: ${relative}`);
  }
  for (const element of document.querySelectorAll('script[src],link[rel="stylesheet"],img[src]')) {
    const url = new URL(element.getAttribute('src') ?? element.getAttribute('href'), base);
    assert.equal(url.origin, origin, `Unexpected third-party page asset: ${url}`);
  }
  assert(!/\/Users\/|prj_[a-f\d]{20}|TODO|lorem ipsum/i.test(document.body.textContent), `Private path or placeholder in ${relative}`);
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
}

const integrity = JSON.parse(readFileSync(path.join(root, 'theme-integrity.json'), 'utf8'));
for (const [relative, expected] of Object.entries(integrity.sha256)) {
  assert.equal(createHash('sha256').update(readFileSync(path.join(root, 'themes/cactus', relative))).digest('hex'), expected, `Upstream style/font modified: ${relative}`);
}
console.log(`Checked ${htmlFiles.length} pages, ${checkedLinks} local links/assets/anchors and ${Object.keys(integrity.sha256).length} original Cactus style/font files.`);
