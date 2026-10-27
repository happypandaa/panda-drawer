export interface ProcessDiagnostics {
  command: string;
  exitCode: number | null;
  signal: number | null;
  nodeVersion: string;
  npmVersion: string;
  stdout: string;
  stderr: string;
  truncated: boolean;
}

export const MAX_PROCESS_DIAGNOSTIC_BYTES = 4096;
const encoder = new TextEncoder();

// Cover known credential formats before displaying or copying diagnostics.
// This does not prove arbitrary third-party output contains no private data.
export function redactProjectDiagnostic(value: string): string {
  return value
    .replace(/\bBearer\s+[^\s"'<>]+/gi, 'Bearer [已隐藏]')
    .replace(/((?:authorization|set-cookie|cookie)\s*:\s*)[^\r\n]+/gi, '$1[已隐藏]')
    .replace(/(["']?(?:api[_-]?key|access[_-]?token|refresh[_-]?token|token|secret|password|passwd|authorization|set-cookie|cookie)["']?\s*[=:]\s*)([^\r\n,;}]+)/gi, '$1[已隐藏]')
    .replace(/(https?:\/\/)[^\s/@]+:[^\s/@]+@/gi, '$1[已隐藏]@');
}

function cleanText(value: string): string {
  return redactProjectDiagnostic(value)
    .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '')
    .replace(/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/g, '');
}

function bounded(value: string, limit: number): { text: string; truncated: boolean } {
  const text = cleanText(value);
  if (encoder.encode(text).byteLength <= limit) return { text, truncated: false };
  let bytes = 0;
  let output = '';
  for (const character of text) {
    const length = encoder.encode(character).byteLength;
    if (bytes + length > limit) break;
    output += character; bytes += length;
  }
  return { text: output, truncated: true };
}

export function checkedProcessDiagnostics(value: unknown): ProcessDiagnostics | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const input = value as Record<string, unknown>;
  const exitCode = input.exitCode;
  const signal = input.signal;
  if (typeof input.command !== 'string' || typeof input.nodeVersion !== 'string' || typeof input.npmVersion !== 'string'
    || typeof input.stdout !== 'string' || typeof input.stderr !== 'string' || typeof input.truncated !== 'boolean'
    || exitCode !== null && (!Number.isSafeInteger(exitCode) || typeof exitCode !== 'number')
    || signal !== null && (!Number.isSafeInteger(signal) || typeof signal !== 'number' || signal <= 0)) return null;
  const command = bounded(input.command, 256);
  const node = bounded(input.nodeVersion, 80);
  const npm = bounded(input.npmVersion, 80);
  const stdout = bounded(input.stdout, MAX_PROCESS_DIAGNOSTIC_BYTES);
  const stderr = bounded(input.stderr, MAX_PROCESS_DIAGNOSTIC_BYTES);
  return {
    command: command.text, exitCode: exitCode as number | null, signal: signal as number | null,
    nodeVersion: node.text, npmVersion: npm.text, stdout: stdout.text, stderr: stderr.text,
    truncated: input.truncated || command.truncated || node.truncated || npm.truncated || stdout.truncated || stderr.truncated,
  };
}

export function processExitDescription(value: ProcessDiagnostics): string {
  if (value.signal !== null) return `被信号 ${value.signal} 终止`;
  if (value.exitCode !== null) return `退出码 ${value.exitCode}`;
  return '未取得退出码';
}

export function processDiagnosticsText(value: ProcessDiagnostics): string {
  return [
    `执行命令：${value.command}`,
    `执行结果：${processExitDescription(value)}`,
    `Node：${value.nodeVersion}；npm：${value.npmVersion}`,
    value.stderr ? `错误输出：\n${value.stderr}` : '',
    value.stdout ? `标准输出：\n${value.stdout}` : '',
    !value.stderr && !value.stdout ? '进程没有返回文字错误。' : '',
    value.truncated ? '输出较长，部分内容已省略。' : '',
  ].filter(Boolean).join('\n\n');
}
