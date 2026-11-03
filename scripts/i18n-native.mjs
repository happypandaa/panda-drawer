import { readdir, readFile } from 'node:fs/promises';
import path from 'node:path';

const han = /\p{Script=Han}/u;
const groups = { '(': ')', '[': ']', '{': '}' };
const isString = token => token?.kind === 'string';
const punctuation = (token, value) => token?.kind === 'punctuation' && token.value === value;
const opensGroup = token => token?.kind === 'punctuation' && Object.hasOwn(groups, token.value);

// Read Rust literals without evaluating Rust or mistaking comments, character
// literals, byte strings or raw-string contents for calls or match arms.
function tokenize(source, file) {
  const tokens = [];
  let index = 0;
  const fail = message => { throw new Error(`${file}: ${message} at offset ${index}`); };
  const quoted = start => {
    let value = ''; let cursor = start + 1;
    while (cursor < source.length) {
      const character = source[cursor++];
      if (character === '"') return { value, end: cursor };
      if (character !== '\\') { value += character; continue; }
      const escape = source[cursor++];
      const simple = { n: '\n', r: '\r', t: '\t', '0': '\0', '\\': '\\', '"': '"', "'": "'" };
      if (Object.hasOwn(simple, escape)) value += simple[escape];
      else if (escape === 'x') {
        const hex = source.slice(cursor, cursor + 2);
        if (!/^[\da-f]{2}$/i.test(hex)) fail('Invalid hexadecimal string escape');
        value += String.fromCharCode(parseInt(hex, 16)); cursor += 2;
      } else if (escape === 'u' && source[cursor] === '{') {
        const close = source.indexOf('}', cursor + 1);
        const hex = source.slice(cursor + 1, close).replaceAll('_', '');
        if (close < 0 || !/^[\da-f]{1,6}$/i.test(hex)) fail('Invalid Unicode string escape');
        value += String.fromCodePoint(parseInt(hex, 16)); cursor = close + 1;
      } else if (escape === '\n' || escape === '\r' && source[cursor] === '\n') {
        while (/\s/.test(source[cursor] ?? '') && cursor < source.length) cursor++;
      } else fail('Unsupported Rust string escape');
    }
    fail('Unterminated string');
  };
  const characterEnd = start => {
    let cursor = start + 1;
    if (source[cursor] === '\\') {
      cursor++;
      if (source[cursor] === 'u' && source[cursor + 1] === '{') {
        cursor = source.indexOf('}', cursor + 2);
        if (cursor < 0) return null;
        cursor++;
      } else cursor += source[cursor] === 'x' ? 3 : 1;
    } else cursor += (source.codePointAt(cursor) ?? 0) > 0xffff ? 2 : 1;
    return source[cursor] === "'" ? cursor + 1 : null;
  };
  while (index < source.length) {
    const start = index; const character = source[index];
    if (/\s/.test(character)) { index++; continue; }
    if (source.startsWith('//', index)) {
      const end = source.indexOf('\n', index + 2); index = end < 0 ? source.length : end; continue;
    }
    if (source.startsWith('/*', index)) {
      let depth = 1; index += 2;
      while (index < source.length && depth) {
        if (source.startsWith('/*', index)) { depth++; index += 2; }
        else if (source.startsWith('*/', index)) { depth--; index += 2; }
        else index++;
      }
      if (depth) fail('Unterminated block comment');
      continue;
    }
    let raw = character === 'r' ? index + 1 : ['br', 'cr'].includes(source.slice(index, index + 2)) ? index + 2 : -1;
    if (raw >= 0) {
      const hashStart = raw;
      while (source[raw] === '#') raw++;
      if (source[raw] === '"') {
        const closing = '"' + '#'.repeat(raw - hashStart);
        const end = source.indexOf(closing, raw + 1);
        if (end < 0) fail('Unterminated raw string');
        index = end + closing.length;
        tokens.push({ kind: character === 'r' ? 'string' : 'opaque', value: source.slice(raw + 1, end).replaceAll('\r\n', '\n'), start, end: index });
        continue;
      }
    }
    const quote = character === '"' ? index : ['b', 'c'].includes(character) && source[index + 1] === '"' ? index + 1 : -1;
    if (quote >= 0) {
      const result = quoted(quote); index = result.end;
      tokens.push({ kind: quote === start ? 'string' : 'opaque', value: result.value.replaceAll('\r\n', '\n'), start, end: index }); continue;
    }
    const charStart = character === "'" ? index : character === 'b' && source[index + 1] === "'" ? index + 1 : -1;
    const charEnd = charStart >= 0 ? characterEnd(charStart) : null;
    if (charEnd !== null) { index = charEnd; tokens.push({ kind: 'opaque', value: '', start, end: index }); continue; }
    if (/[A-Za-z_]/.test(character)) {
      while (/[A-Za-z_0-9]/.test(source[++index] ?? '')) { /* identifier */ }
      tokens.push({ kind: 'identifier', value: source.slice(start, index), start, end: index }); continue;
    }
    index += ['::', '=>'].includes(source.slice(index, index + 2)) ? 2 : 1;
    tokens.push({ kind: 'punctuation', value: source.slice(start, index), start, end: index });
  }
  return tokens;
}

function closeGroup(tokens, start) {
  if (!opensGroup(tokens[start])) throw new Error('Expected an opening Rust token group');
  const stack = [groups[tokens[start].value]];
  for (let index = start + 1; index < tokens.length; index++) {
    if (tokens[index].kind !== 'punctuation') continue;
    const value = tokens[index].value;
    if (opensGroup(tokens[index])) stack.push(groups[value]);
    else if (value === stack.at(-1)) { stack.pop(); if (!stack.length) return index; }
    else if (Object.values(groups).includes(value)) throw new Error('Mismatched Rust token group');
  }
  throw new Error('Unterminated Rust token group');
}
function argumentsOf(tokens) {
  const result = []; let start = 0;
  for (let index = 0; index < tokens.length; index++) {
    if (opensGroup(tokens[index])) index = closeGroup(tokens, index);
    else if (punctuation(tokens[index], ',')) { result.push(tokens.slice(start, index)); start = index + 1; }
  }
  if (start < tokens.length) result.push(tokens.slice(start));
  return result;
}
// Three-valued cfg evaluation: platform/feature conditions are unknown, while
// test=false lets us omit cfg(test) helpers and modules on every platform.
function cfgValue(tokens) {
  if (tokens.length === 1 && tokens[0].kind === 'identifier' && tokens[0].value === 'test') return false;
  const name = tokens[0]?.value;
  if (!['all', 'any', 'not'].includes(name) || !punctuation(tokens[1], '(') || closeGroup(tokens, 1) !== tokens.length - 1) return null;
  const values = argumentsOf(tokens.slice(2, -1)).map(cfgValue);
  if (name === 'not') return values.length === 1 && values[0] !== null ? !values[0] : null;
  if (name === 'all') return values.includes(false) ? false : values.every(value => value === true) ? true : null;
  return values.includes(true) ? true : values.every(value => value === false) ? false : null;
}
function activeTokens(tokens) {
  const result = [];
  for (let index = 0; index < tokens.length; index++) {
    if (punctuation(tokens[index], '#') && punctuation(tokens[index + 1], '[')) {
      const end = closeGroup(tokens, index + 1); const attribute = tokens.slice(index + 2, end);
      if (attribute[0]?.value === 'cfg' && punctuation(attribute[1], '(') && cfgValue(attribute.slice(2, -1)) === false) {
        let item = end + 1;
        while (punctuation(tokens[item], '#') && punctuation(tokens[item + 1], '[')) item = closeGroup(tokens, item + 1) + 1;
        let declaration = item;
        if (tokens[declaration]?.value === 'pub') { declaration++; if (punctuation(tokens[declaration], '(')) declaration = closeGroup(tokens, declaration) + 1; }
        while (['async', 'unsafe', 'const', 'extern'].includes(tokens[declaration]?.value)) { declaration++; if (isString(tokens[declaration])) declaration++; }
        // Struct fields/initializers end with a comma, whereas modules and
        // functions end with their body. Do not swallow the production impl.
        const field = tokens[declaration]?.kind === 'identifier' && punctuation(tokens[declaration + 1], ':');
        index = item;
        for (; index < tokens.length; index++) {
          if (punctuation(tokens[index], ';') || field && punctuation(tokens[index], ',')) break;
          if (field && punctuation(tokens[index], '}')) { index--; break; }
          if (punctuation(tokens[index], '{')) { index = closeGroup(tokens, index); if (!field) break; }
          else if (opensGroup(tokens[index])) index = closeGroup(tokens, index);
        }
        continue;
      }
    }
    result.push(tokens[index]);
  }
  return result;
}
async function rustFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = await Promise.all(entries.sort((a, b) => a.name.localeCompare(b.name)).map(async entry => {
    const file = path.join(directory, entry.name);
    if (/(?:^|[_./-])tests?(?:[_./-]|$)/i.test(entry.name)) return [];
    return entry.isDirectory() ? rustFiles(file) : entry.isFile() && entry.name.endsWith('.rs') ? [file] : [];
  }));
  return files.flat();
}

/** Scan only native text calls/catalogs and declared backend UI message sources.
 * This is a static inventory, not a claim that all backend errors are localized.
 */
export async function scanNativeTranslations(root) {
  if (!path.isAbsolute(root)) throw new Error('scanNativeTranslations requires an absolute repository root');
  const result = { locales: [], entries: [], usages: [], serviceSources: [] };
  const nativeDirectory = path.join(root, 'apps/desktop/src-tauri/src');
  const coreDirectory = path.join(root, 'crates/platform-core/src');
  const [nativeFiles, coreFiles] = await Promise.all([rustFiles(nativeDirectory), rustFiles(coreDirectory)]);
  for (const absolute of [...nativeFiles, ...coreFiles]) {
    const source = await readFile(absolute, 'utf8'); const file = path.relative(root, absolute).split(path.sep).join('/');
    const tokens = activeTokens(tokenize(source, file)); const isLocale = file === 'apps/desktop/src-tauri/src/ui_locale.rs';
    const isNative = absolute.startsWith(nativeDirectory + path.sep);
    const lineStarts = [0];
    for (let index = 0; index < source.length; index++) if (source[index] === '\n') lineStarts.push(index + 1);
    const location = token => {
      let low = 0; let high = lineStarts.length;
      while (low + 1 < high) { const middle = (low + high) >> 1; if (lineStarts[middle] <= token.start) low = middle; else high = middle; }
      return { file, line: low + 1, column: token.start - lineStarts[low] + 1 };
    };
    for (let index = 0; index < tokens.length; index++) {
      const token = tokens[index]; const next = offset => tokens[index + offset]?.value;
      if (isLocale && token.kind === 'identifier' && token.value === 'SUPPORTED_LOCALES' && punctuation(tokens[index + 1], ':')) {
        let array = index + 1; while (tokens[array] && !(punctuation(tokens[array], '=') || punctuation(tokens[array], ';'))) array++;
        if (!punctuation(tokens[array], '=')) throw new Error(`${file}: SUPPORTED_LOCALES needs a literal array`);
        array++; if (punctuation(tokens[array], '&')) array++;
        if (!punctuation(tokens[array], '[')) throw new Error(`${file}: SUPPORTED_LOCALES needs a literal array`);
        const items = argumentsOf(tokens.slice(array + 1, closeGroup(tokens, array)));
        if (!items.every(item => item.length === 1 && isString(item[0]))) throw new Error(`${file}: SUPPORTED_LOCALES contains a nonliteral locale`);
        result.locales.push(...items.map(item => item[0].value));
      }
      if (isLocale && punctuation(token, '(')) {
        const end = closeGroup(tokens, index); const args = argumentsOf(tokens.slice(index + 1, end));
        if (punctuation(tokens[end + 1], '=>')) {
          if (args.length !== 2 || !args.every(arg => arg.length === 1 && isString(arg[0])) || !isString(tokens[end + 2]) || ![',', '}'].some(value => punctuation(tokens[end + 3], value))) {
            throw new Error(`${file}:${location(token).line}: Unsupported native translation arm; use ("locale", "source") => "translation"`);
          }
          result.entries.push({ locale: args[0][0].value, key: args[1][0].value, value: tokens[end + 2].value, ...location(args[1][0]) });
        }
      }
      const qualified = isNative && token.kind === 'identifier' && token.value === 'ui_locale' && next(1) === '::' && next(2) === 'text' && next(3) === '(';
      const unqualified = isLocale && token.kind === 'identifier' && token.value === 'text' && next(1) === '(' && !['fn', '.', '::'].includes(tokens[index - 1]?.value);
      if (qualified || unqualified) {
        const open = index + (qualified ? 3 : 1); const args = argumentsOf(tokens.slice(open + 1, closeGroup(tokens, open))); const first = args[0] ?? [];
        result.usages.push({ key: first.length === 1 && isString(first[0]) ? first[0].value : null, expression: first.length ? source.slice(first[0].start, first.at(-1).end) : '', ...location(first[0] ?? token) });
      }
      if (punctuation(token, '.') && ['admit_operation', 'admit_window_operation'].includes(next(1)) && next(2) === '(') {
        const args = argumentsOf(tokens.slice(index + 3, closeGroup(tokens, index + 2))); const label = args[1];
        if (label?.length === 1 && isString(label[0]) && han.test(label[0].value)) result.serviceSources.push({ key: label[0].value, ...location(label[0]), kind: 'operation-label' });
      }
      const kind = file === 'crates/platform-core/src/platform_info.rs' ? 'platform-info' : file === 'crates/platform-core/src/node_run_marker.rs' ? 'node-run-marker' : null;
      if (kind && isString(token) && han.test(token.value)) result.serviceSources.push({ key: token.value, ...location(token), kind });
    }
  }
  return result;
}
