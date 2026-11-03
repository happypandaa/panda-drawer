// Static resource audit only: never imports application code or starts Tauri.
// --list prints localized fields; --json includes full text, locations and dynamic boundaries.
import { readFile, readdir } from 'node:fs/promises';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { scanNativeTranslations } from './i18n-native.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const src = path.join(root, 'apps/desktop/src');
const require = createRequire(path.join(root, 'apps/desktop/package.json'));
const ts = require('typescript');
const flags = new Set(process.argv.slice(2));
if ([...flags].some(flag => !['--list', '--json'].includes(flag))) {
  console.error('Usage: node scripts/check-i18n.mjs [--list | --json]');
  process.exit(2);
}
const modules = new Map();
const problems = [];
const overrides = [];
const fields = [];
const candidates = [];
const exemptions = [];
const placeholderNames = value => [...new Set([...value.matchAll(/\{([A-Za-z][A-Za-z0-9_]*)\}/g)].map(match => match[1]))].sort();
const chinese = /\p{Script=Han}/u;
const relative = file => path.relative(root, file).split(path.sep).join('/');
function location(node) {
  const file = node.getSourceFile();
  const at = file.getLineAndCharacterOfPosition(node.getStart(file));
  return { file: relative(file.fileName), line: at.line + 1, column: at.character + 1 };
}
function issue(code, message, at, extra = {}) { problems.push({ code, message, ...at, ...extra }); }
function unwrap(node) {
  while (node && (ts.isParenthesizedExpression(node) || ts.isAsExpression(node) || ts.isSatisfiesExpression(node) || ts.isTypeAssertionExpression(node) || ts.isNonNullExpression(node))) node = node.expression;
  return node;
}
async function sourceModule(file) {
  file = path.resolve(file);
  if (modules.has(file)) return modules.get(file);
  const text = await readFile(file, 'utf8');
  const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, file.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  for (const diagnostic of source.parseDiagnostics) {
    const at = source.getLineAndCharacterOfPosition(diagnostic.start ?? 0);
    issue('SYNTAX', ts.flattenDiagnosticMessageText(diagnostic.messageText, '\n'), { file: relative(file), line: at.line + 1, column: at.character + 1 });
  }
  const module = { file, source, bindings: new Map(), imports: new Map(), exported: null };
  modules.set(file, module);
  function visit(node) {
    if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && node.initializer) module.bindings.set(node.name.text, node.initializer);
    ts.forEachChild(node, visit);
  }
  visit(source);
  for (const node of source.statements) {
    if (ts.isImportDeclaration(node) && ts.isStringLiteral(node.moduleSpecifier) && node.importClause?.name) module.imports.set(node.importClause.name.text, node.moduleSpecifier.text);
    if (ts.isExportAssignment(node) && !node.isExportEquals) module.exported = node.expression;
  }
  return module;
}
const unknown = Symbol('dynamic expression');
// Only literals, local constants and imported catalog objects are evaluated.
// No eval(), transpilation, application module imports or function execution.
async function evaluate(node, module, seen = new Set()) {
  node = unwrap(node);
  if (!node || seen.has(node)) return unknown;
  seen = new Set(seen).add(node);
  if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) return node.text;
  if (ts.isObjectLiteralExpression(node)) {
    const result = new Map();
    for (const item of node.properties) {
      if (ts.isSpreadAssignment(item)) {
        const spread = await evaluate(item.expression, module, seen);
        if (!(spread instanceof Map)) return unknown;
        for (const [key, entry] of spread) {
          const previous = result.get(key);
          if (previous && previous.value !== entry.value) overrides.push({ key, previous: location(previous.node), effective: location(entry.node) });
          result.set(key, entry);
        }
      } else if (ts.isPropertyAssignment(item) && (ts.isIdentifier(item.name) || ts.isStringLiteral(item.name))) {
        const key = item.name.text;
        if (result.has(key)) issue('DUPLICATE_KEY', '同一个资源对象重复定义词条', location(item.name), { key });
        result.set(key, { value: await evaluate(item.initializer, module, seen), node: item.initializer });
      } else return unknown;
    }
    return result;
  }
  if (ts.isIdentifier(node)) {
    if (module.bindings.has(node.text)) return evaluate(module.bindings.get(node.text), module, seen);
    const imported = module.imports.get(node.text);
    if (imported?.startsWith('.')) {
      const importedFile = path.resolve(path.dirname(module.file), imported + (path.extname(imported) ? '' : '.ts'));
      const target = await sourceModule(importedFile);
      if (target.exported) return evaluate(target.exported, target, seen);
    }
  }
  return unknown;
}
async function walkFiles(directory, suffix, skip = new Set()) {
  const result = [];
  for (const item of (await readdir(directory, { withFileTypes: true })).sort((a, b) => a.name.localeCompare(b.name, 'en'))) {
    if (skip.has(item.name)) continue;
    const file = path.join(directory, item.name);
    if (item.isDirectory()) result.push(...await walkFiles(file, suffix, skip));
    else if (item.isFile() && suffix.test(item.name)) result.push(file);
  }
  return result;
}
function fieldName(node) {
  for (let parent = node.parent; parent; parent = parent.parent) {
    if (ts.isJsxAttribute(parent)) return parent.name.getText();
    if (ts.isJsxElement(parent)) return parent.openingElement.tagName.getText();
    if (ts.isVariableDeclaration(parent) && ts.isIdentifier(parent.name)) return parent.name.text;
    if (ts.isFunctionLike(parent)) break;
  }
  return 'message';
}
function exemption(node) {
  const file = relative(node.getSourceFile().fileName);
  // These values identify data or name a language in its own script, not copy.
  if (file === 'apps/desktop/src/App.tsx' && node.text === '可丢弃'
    && ts.isCallExpression(node.parent) && ts.isPropertyAccessExpression(node.parent.expression)
    && node.parent.expression.name.text === 'includes') return '用于识别可丢弃项目的匹配标记';
  if (file === 'apps/desktop/src/i18n.ts' && node.text === '中文'
    && ts.isPropertyAssignment(node.parent) && node.parent.name.getText() === 'label') return '语言名称使用其本来写法';
  return null;
}
const registry = await sourceModule(path.join(src, 'i18n.ts'));
const catalogMap = await evaluate(registry.bindings.get('catalogs'), registry);
if (!(catalogMap instanceof Map)) throw new Error('Cannot statically read the catalogs registry in apps/desktop/src/i18n.ts.');
const catalogs = new Map();
const resources = [];
for (const [locale, entry] of catalogMap) {
  if (!(entry.value instanceof Map)) throw new Error(`Cannot statically read catalog ${locale}.`);
  if (locale === 'zh-CN') continue; // Chinese source text is the explicit fallback.
  const messages = new Map();
  for (const [key, translation] of entry.value) {
    const at = location(translation.node);
    if (typeof translation.value !== 'string' || !translation.value.trim()) issue('EMPTY_TRANSLATION', '翻译必须是非空字符串', at, { locale, key });
    else {
      const sourceParameters = placeholderNames(key);
      const translatedParameters = placeholderNames(translation.value);
      if (sourceParameters.join('\0') !== translatedParameters.join('\0')) issue('PLACEHOLDERS', '原文与翻译的占位参数不一致', at, { locale, key, expected: sourceParameters, actual: translatedParameters });
    }
    messages.set(key, translation.value);
    resources.push({ locale, key, value: translation.value === unknown ? null : translation.value, ...at });
  }
  catalogs.set(locale, messages);
}
if (!catalogs.size) throw new Error('No target language catalogs are registered.');
const pending = [];
for (const file of await walkFiles(src, /\.(ts|tsx)$/, new Set(['locales']))) {
  const module = await sourceModule(file);
  const visit = node => {
    if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === 't' && node.arguments[0]) pending.push({ node, module });
    if ((ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) && chinese.test(node.text)) {
      const reason = exemption(node);
      if (reason) exemptions.push({ key: node.text, reason, ...location(node) });
      else candidates.push({ key: node.text, kind: 'frontend-source', ...location(node) });
    }
    if (ts.isTemplateExpression(node) && (chinese.test(node.head.text) || node.templateSpans.some(span => chinese.test(span.literal.text)))
      && ts.isCallExpression(node.parent) && node.parent.expression.getText() === 't' && node.parent.arguments[0] === node) issue('DYNAMIC_TEMPLATE_KEY', '动态文案请改为固定词条与占位参数', location(node));
    if (ts.isJsxText(node) && chinese.test(node.text)) issue('UNLOCALIZED_JSX', '界面仍有未经过翻译的中文文本', location(node), { key: node.text.trim() });
    if (ts.isJsxAttribute(node) && node.initializer && ts.isStringLiteral(node.initializer)
      && ['title', 'aria-label', 'placeholder', 'alt', 'description', 'label'].includes(node.name.getText()) && chinese.test(node.initializer.text)) issue('UNLOCALIZED_ATTRIBUTE', '显示属性仍有未经过翻译的中文文本', location(node), { key: node.initializer.text });
    ts.forEachChild(node, visit);
  };
  visit(module.source);
}
for (const { node, module } of pending) {
  const value = await evaluate(node.arguments[0], module);
  const key = typeof value === 'string' ? value : null;
  const field = { ...location(node), field: fieldName(node), expression: node.arguments[0].getText(), key, dynamic: key === null };
  fields.push(field);
  if (key === null) continue;
  candidates.push({ key, kind: 'translation-call', ...location(node) });
  const required = placeholderNames(key);
  if (!required.length) continue;
  const parameters = node.arguments[1] ? await evaluate(node.arguments[1], module) : null;
  if (parameters === null || parameters instanceof Map) {
    const supplied = parameters ? [...parameters.keys()] : [];
    const missing = required.filter(name => !supplied.includes(name));
    const extra = supplied.filter(name => !required.includes(name));
    if (missing.length || extra.length) issue('CALL_PARAMETERS', '翻译调用的占位参数不匹配', location(node), { key, missing, extra });
  } else field.dynamicParameters = true;
}
const native = await scanNativeTranslations(root);
const nativeCatalogs = new Map();
for (const entry of native.entries) {
  if (!nativeCatalogs.has(entry.locale)) nativeCatalogs.set(entry.locale, new Map());
  const catalog = nativeCatalogs.get(entry.locale);
  if (catalog.has(entry.key)) issue('NATIVE_DUPLICATE_KEY', '原生语言资源重复定义词条', entry, { locale: entry.locale });
  catalog.set(entry.key, entry.value);
  if (!entry.value.trim()) issue('NATIVE_EMPTY_TRANSLATION', '原生翻译必须是非空字符串', entry);
  if (placeholderNames(entry.key).join('\0') !== placeholderNames(entry.value).join('\0')) issue('NATIVE_PLACEHOLDERS', '原生词条占位参数不一致', entry);
}
for (const locale of catalogs.keys()) {
  if (!native.locales.includes(locale)) issue('NATIVE_LOCALE', '前端语言尚未在原生语言列表中注册', { file: 'apps/desktop/src-tauri/src/ui_locale.rs', line: 1, column: 1 }, { locale });
  for (const usage of native.usages) {
    if (usage.key !== null && !nativeCatalogs.get(locale)?.has(usage.key)) issue('NATIVE_MISSING_TRANSLATION', '原生文字缺少翻译', usage, { locale });
  }
}
candidates.push(...native.serviceSources);
const seen = new Set();
for (const candidate of candidates) {
  for (const [locale, messages] of catalogs) {
    if (messages.has(candidate.key)) continue;
    const signature = [locale, candidate.file, candidate.line, candidate.column, candidate.key].join('\0');
    if (!seen.has(signature)) { seen.add(signature); issue('MISSING_TRANSLATION', '源文案缺少翻译', candidate, { locale }); }
  }
}
const report = {
  ok: problems.length === 0,
  summary: {
    frontendFiles: new Set(fields.map(field => field.file)).size,
    frontendCalls: fields.length,
    resolvedCalls: fields.filter(field => !field.dynamic).length,
    dynamicCalls: fields.filter(field => field.dynamic).length,
    sourceTexts: new Set(candidates.map(candidate => candidate.key)).size,
    locales: Object.fromEntries([...catalogs].map(([locale, messages]) => [locale, messages.size])),
    nativeCalls: native.usages.length,
    nativeDynamicCalls: native.usages.filter(usage => usage.key === null).length,
    serviceSourceTexts: new Set(native.serviceSources.map(candidate => candidate.key)).size,
    problems: problems.length,
  },
  problems, fields, candidates, resources, native, overrides, exemptions,
  limitations: [
    '变量、状态、后端消息、用户输入与第三方输出无法全部静态推断；dynamic 字段为这些调用点的清单。',
    '额外追踪更新说明、运行恢复提示和删除等待操作名的后端来源；没有宣称覆盖所有后端错误。',
    '本脚本核对资源与参数，不验证译文含义、布局、切换行为或原生编译。',
  ],
};
if (flags.has('--json')) process.stdout.write(JSON.stringify(report, null, 2) + '\n');
else {
  console.log(`界面翻译调用：${report.summary.frontendCalls} 处，${report.summary.frontendFiles} 个文件（可解析 ${report.summary.resolvedCalls}，动态 ${report.summary.dynamicCalls}）。`);
  console.log(`源文案：${report.summary.sourceTexts} 条；语言资源：${[...catalogs].map(([locale, messages]) => `${locale} ${messages.size} 条`).join('；')}。`);
  console.log(`原生调用：${report.summary.nativeCalls} 处（动态 ${report.summary.nativeDynamicCalls}）；追踪后端显示文案：${report.summary.serviceSourceTexts} 条。`);
  if (flags.has('--list')) {
    for (const field of fields) console.log(`${field.file}:${field.line}:${field.column} [${field.field}] ${field.dynamic ? `[动态] ${field.expression}` : JSON.stringify(field.key)}`);
    for (const usage of native.usages) console.log(`${usage.file}:${usage.line}:${usage.column} [原生] ${usage.key === null ? `[动态] ${usage.expression}` : JSON.stringify(usage.key)}`);
  }
  for (const problem of problems) console.error(`${problem.file}:${problem.line}:${problem.column} ${problem.code} ${problem.locale ?? ''} ${JSON.stringify(problem.key ?? problem.message)}`);
  console.log(report.ok ? '静态检查通过：已扫描源文案均有翻译，资源及可解析调用的占位参数一致。' : `静态检查发现 ${problems.length} 项问题。`);
  console.log('动态来源不能据此保证全部覆盖；用 --json 查看完整位置、词条和检查边界。');
}
process.exitCode = report.ok ? 0 : 1;
