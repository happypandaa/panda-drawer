// Prepare source fixtures only. This never opens the platform or registers,
// builds, deletes, or modifies an existing project.
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const args = process.argv.slice(2);
if (args.length !== 2 || args[0] !== '--output') throw new Error('Usage: node scripts/create-panda-acceptance-fixtures.mjs --output <parent-folder>');
const repo = fileURLToPath(new URL('../', import.meta.url));
const server = await readFile(join(repo, 'templates/local-node/server.mjs'), 'utf8');
const root = join(resolve(args[1]), `panda-acceptance-${new Date().toISOString().replace(/[:.]/g, '-')}`);
await mkdir(root, { recursive: false });
const page = `<!doctype html><html lang="zh-CN"><meta charset="utf-8"><title>Panda 可丢弃验收页</title>
<script src="/_platform/sdk.js"></script><script src="/assets/absolute.js"></script>
<script src="https://cdn.jsdelivr.net/npm/dayjs@1.11.13/dayjs.min.js"></script>
<style>body{font:16px system-ui;max-width:760px;margin:32px auto;padding:12px;line-height:1.8}button,input{font:inherit;margin:5px;padding:6px}pre{white-space:pre-wrap;background:#eee;padding:14px}</style>
<h1>Panda 可丢弃验收页</h1><p id="build">BUILD_A</p>
<p id="environment"></p><input id="value" value="只用于验收的文字" aria-label="测试文字">
<button onclick="writeStorage()">保存浏览器数据</button><button onclick="showStorage()">读取浏览器数据</button>
<button onclick="readConfig()">读取配置</button><button onclick="writeData()">写数据库和附件</button><button onclick="readData()">读取数据库和附件</button>
<button onclick="dialogs()">测试系统对话框</button><button onclick="privateFiles()">检查私有文件保护</button>
<button onclick="history.pushState({},'', '/route/deep');location.reload()">进入路由并刷新</button>
<a href="https://example.com/" target="_blank">外部链接</a><button onclick="window.open('https://example.com/')">window.open</button>
<pre id="result">等待操作</pre><script>
const out = value => document.querySelector('#result').textContent = JSON.stringify(value,null,2);
document.querySelector('#environment').textContent='地址：'+location.origin+'；绝对资源：'+(window.absoluteAsset?'成功':'失败')+'；内联脚本：成功；CDN：'+(window.dayjs?'成功':'未加载（核对网络）');
const value = () => document.querySelector('#value').value;
function writeStorage(){ localStorage.setItem('panda-acceptance',value());out({saved:value()}); }
function showStorage(){out({stored:localStorage.getItem('panda-acceptance'),origin:location.origin});}
async function readConfig(){try{const config=await panda.config();out({config,types:Object.fromEntries(Object.entries(config).map(([k,v])=>[k,typeof v]))});}catch(e){out({code:e.code,message:e.message});}}
async function writeData(){try{await panda.db.execute({sql:'CREATE TABLE IF NOT EXISTS acceptance (id INTEGER PRIMARY KEY, value TEXT)'});await panda.db.execute({sql:'INSERT OR REPLACE INTO acceptance(id,value) VALUES (1,?)',params:[value()]});await panda.files.put({path:'acceptance.txt',content:new Blob([value()]),overwrite:true});out({written:value()});}catch(e){out({code:e.code,message:e.message});}}
async function readData(){try{out({database:await panda.db.execute({sql:'SELECT * FROM acceptance'}),attachment:await (await panda.files.get('acceptance.txt')).text()});}catch(e){out({code:e.code,message:e.message});}}
function dialogs(){alert('验收 alert');const confirmed=confirm('验收 confirm：选择确定或取消');const text=prompt('验收 prompt：可输入或取消','验收默认值');out({confirmed,text});}
async function privateFiles(){const paths=['/data/never-serve.txt','/panda.json','/node_modules/never-serve.txt','/.hidden/never-serve.txt'];out(await Promise.all(paths.map(async path=>({path,status:(await fetch(path)).status}))));}
</script></html>`;
async function file(folder, name, text) { const target = join(folder, name); await mkdir(resolve(target, '..'), { recursive: true }); await writeFile(target, text, { flag: 'wx' }); }
async function project(name, extra, built = true) {
  const folder = join(root, name); await mkdir(folder);
  await file(folder, 'panda.json', JSON.stringify({ manifestVersion: 1, runtime: 'web', app: { name: `可丢弃验收 ${name}` }, web: { root: '.', entry: 'index.html' }, ...extra }, null, 2));
  await file(folder, '.gitignore', 'node_modules/\ndata/\n# 想提交数据时，可删除上一行。\n');
  if (built) { await file(folder, 'index.html', page); await file(folder, 'assets/absolute.js', 'window.absoluteAsset=true;'); }
  return folder;
}
const sdk = await project('web-sdk', { platform: { sdkApiVersion: 1 }, configuration: { version: 1, fields: [
  { name: 'LABEL', label: '验收文字', type: 'string', required: true, secret: false, description: '仅用于验收', howToGet: ['填写任意测试文字'] },
  { name: 'COUNT', label: '验收整数', type: 'integer', default: 7, required: false, secret: false, description: '仅用于验收', howToGet: ['使用默认值或填写测试值'] },
  { name: 'ENABLED', label: '验收开关', type: 'boolean', default: true, required: false, secret: false, description: '仅用于验收', howToGet: ['使用默认值或填写测试值'] },
  { name: 'MODE', label: '验收选项', type: 'select', default: 'alpha', options: [{ label: '甲', value: 'alpha' }, { label: '乙', value: 'beta' }], required: false, secret: false, description: '仅用于验收', howToGet: ['选择甲或乙'] },
  { name: 'OPTIONAL', label: '选填文字', type: 'string', required: false, secret: false, description: '仅用于验收', howToGet: ['使用默认值或填写测试值'] },
] } });
for (const name of ['data', 'node_modules', '.hidden']) await file(sdk, `${name}/never-serve.txt`, '仅用于验收，不应由网页服务提供');
await project('web-no-sdk', {});
const build = await project('web-build', { web: { root: 'site', entry: 'index.html' }, development: { build: { kind: 'npm', script: 'bundle:web' } } }, false);
await file(build, 'page.html', page);
await file(build, 'package.json', JSON.stringify({ name: 'panda-disposable-acceptance-build', version: '1.0.0', scripts: { 'bundle:web': 'node build.mjs', build: 'node -e "process.exit(88)"' } }, null, 2));
await file(build, 'package-lock.json', JSON.stringify({ name: 'panda-disposable-acceptance-build', version: '1.0.0', lockfileVersion: 3, packages: { '': { name: 'panda-disposable-acceptance-build', version: '1.0.0' } } }, null, 2));
await file(build, 'build.mjs', "import{mkdirSync,readFileSync,writeFileSync}from'node:fs';mkdirSync('site/assets',{recursive:true});writeFileSync('site/index.html',readFileSync('page.html'));writeFileSync('site/assets/absolute.js','window.absoluteAsset=true;');\n");
await project('web-entry-missing', {}, false);
await mkdir(join(root, 'empty-node'));
const node = join(root, 'node'); await mkdir(node);
await file(node, 'panda.json', JSON.stringify({ manifestVersion: 1, runtime: 'node', app: { name: '可丢弃验收 Node', version: '0.1.0' }, web: { root: 'web', entry: 'index.html' }, backend: { root: 'backend', entry: 'server.mjs', nodeMajor: 24, protocolVersion: 1 } }, null, 2));
await file(node, 'backend/server.mjs', server.replace("return reply(response, 200, { message: '你的本地应用已经连通。' });", "return reply(response, 200, { message: 'Node 已连通', dataDir: process.env.APP_DATA_DIR });"));
await file(node, 'web/index.html', '<!doctype html><meta charset="utf-8"><h1>可丢弃验收 Node</h1><button onclick="localStorage.setItem(\'node-acceptance\',\'saved\');result.textContent=localStorage.getItem(\'node-acceptance\')">保存浏览器数据</button><button onclick="result.textContent=localStorage.getItem(\'node-acceptance\')">读取浏览器数据</button><button onclick="fetch(\'/api/hello\').then(r=>r.json()).then(v=>result.textContent=JSON.stringify(v))">检查数据目录</button><pre id="result"></pre>');
await file(node, '.gitignore', 'node_modules/\ndata/\n');
console.log(JSON.stringify({ root, folders: ['web-sdk', 'web-no-sdk', 'web-build', 'web-entry-missing', 'node', 'empty-node'], note: '仅创建测试源文件；未运行或登记应用。' }, null, 2));
