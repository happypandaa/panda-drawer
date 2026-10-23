import { invoke } from '@tauri-apps/api/core';
import { useEffect, useRef, useState } from 'react';
import { messageFrom } from './ui-utils';

type Kind = 'node' | 'git';
type Tool = { path: string; version: string; selection?: 'managed' | 'configured' };
type Issue = { code: string; message: string };
type Report = { ready: boolean; node: Tool | null; npm: Tool | null; git: Tool | null; issues: Issue[] };
type Operation = 'doctor' | 'choose' | 'configure';
function localPath(value: unknown): value is string {
  return typeof value === 'string' && /^[a-z]:[\\/]/i.test(value) && !value.includes('\0')
    && !value.split(/[\\/]/).some(part => part === '.' || part === '..');
}
function checkedReport(value: unknown): Report {
  const record = value as Partial<Report> | null;
  const tool = (item: unknown, selection: boolean) => {
    if (item === null) return true;
    if (!item || typeof item !== 'object') return false;
    const candidate = item as Tool;
    return localPath(candidate.path) && typeof candidate.version === 'string' && Boolean(candidate.version.trim())
      && (!selection || ['managed', 'configured'].includes(candidate.selection ?? ''));
  };
  if (!record || typeof record.ready !== 'boolean' || !tool(record.node, true) || !tool(record.npm, false) || !tool(record.git, true)
    || !Array.isArray(record.issues) || !record.issues.every(issue => issue && typeof issue === 'object' && typeof issue.code === 'string' && Boolean(issue.code.trim()) && typeof issue.message === 'string' && Boolean(issue.message.trim())))
    throw new Error('开发工具检查结果不完整，请重新检查；当前不能确认工具已就绪。');
  if (record.ready !== Boolean(record.node && record.npm && record.git && record.issues.length === 0))
    throw new Error('开发工具就绪状态与检查结果不符，请重新检查。');
  return record as Report;
}

export function WindowsDevelopmentTools({ disabled, onPendingChange, requiresRecheck = false, onRecheckRequiredChange }: {
  disabled: boolean;
  onPendingChange?: (pending: boolean) => void;
  requiresRecheck?: boolean;
  onRecheckRequiredChange?: (required: boolean) => void;
}) {
  const [report, setReport] = useState<Report | null>(null);
  const [pending, setPending] = useState<Operation | null>(null);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [needsRecheck, setNeedsRecheck] = useState(requiresRecheck);
  const [draft, setDraft] = useState<{ kind: Kind; path: string } | null>(null);
  const alive = useRef(false); const operation = useRef(false);
  const recheckRequired = useRef(requiresRecheck);
  const callbacks = useRef({ onPendingChange, onRecheckRequiredChange }); callbacks.current = { onPendingChange, onRecheckRequiredChange };
  useEffect(() => { alive.current = true; return () => { alive.current = false; callbacks.current.onPendingChange?.(false); }; }, []);
  useEffect(() => { recheckRequired.current = requiresRecheck; setNeedsRecheck(requiresRecheck); }, [requiresRecheck]);
  function requireRecheck(required: boolean) {
    recheckRequired.current = required; setNeedsRecheck(required); callbacks.current.onRecheckRequiredChange?.(required);
  }
  async function run(kind: Operation, action: () => Promise<void>) {
    if (disabled || operation.current || (kind === 'configure' && recheckRequired.current)) return;
    operation.current = true; setPending(kind); setError(''); setNotice(''); callbacks.current.onPendingChange?.(true);
    if (kind !== 'choose') setReport(null);
    try { await action(); }
    catch (cause) {
      if (alive.current) {
        setReport(null); setError(messageFrom(cause));
        if (kind !== 'choose') requireRecheck(true);
      }
    } finally {
      operation.current = false;
      if (alive.current) { callbacks.current.onPendingChange?.(false); setPending(null); }
    }
  }
  function doctor() {
    void run('doctor', async () => {
      const value = checkedReport(await invoke<unknown>('development_environment_doctor'));
      if (alive.current) { setReport(value); requireRecheck(false); }
    });
  }
  function configure(kind: Kind, path: string | null) {
    void run('configure', async () => {
      const value = checkedReport(await invoke<unknown>('development_environment_configure', { kind, path }));
      if (alive.current) { setReport(value); setDraft(null); requireRecheck(false); }
    });
  }
  function choose(kind: Kind) {
    if (disabled || operation.current || recheckRequired.current) return;
    setDraft(null);
    void run('choose', async () => {
      const path = await invoke<unknown>('development_choose_tool', { kind });
      if (!alive.current) return;
      if (path === null) { setNotice('已取消选择，工具配置未提交。'); return; }
      if (!localPath(path) || path.split(/[\\/]/).at(-1)?.toLowerCase() !== `${kind}.exe`) throw new Error(`选择器未返回有效的 ${kind}.exe 本机路径，请重新选择。`);
      setDraft({ kind, path });
    });
  }
  const blocked = disabled || pending !== null;
  return <section className="diagnostics-card" aria-label="Windows 开发工具" aria-busy={pending !== null}>
    <div className="section-heading"><div><h3>开发工具</h3><p>开发 Node/npm 与 Git 供源码获取和候选构建使用。内置工具经过固定摘要校验；显式选择失效时需要重新选择。</p></div><button className="refresh-button" disabled={blocked} onClick={doctor}>{pending === 'doctor' ? '正在检查开发工具…' : '检查开发工具'}</button></div>
    {report && <><p role="status">{report.ready ? '开发工具已就绪' : '开发工具需要处理'}</p><dl className="path-list">{(['node', 'npm', 'git'] as const).map(kind => <div className="path-row" key={kind}><dt>{kind === 'node' ? '开发 Node' : kind === 'npm' ? 'npm' : 'Git'}</dt><dd>{report[kind] ? <><span>{report[kind]!.version}{kind !== 'npm' && (report[kind]!.selection === 'configured' ? ' · 已选择' : ' · 内置')}</span><code>{report[kind]!.path}</code></> : '不可用'}</dd></div>)}</dl>{report.issues.map((issue, index) => <p role="alert" key={index}>{messageFrom(issue)}</p>)}</>}
    <div className="task-actions">{(['node', 'git'] as const).map(kind => <span key={kind}><button className="refresh-button" disabled={blocked || needsRecheck} onClick={() => choose(kind)}>选择{kind === 'node' ? '开发 Node' : 'Git'}</button><button className="text-button" disabled={blocked || needsRecheck} onClick={() => configure(kind, null)}>使用内置{kind === 'node' ? 'Node' : 'Git'}</button></span>)}</div>
    {draft && <div className="node-inspection-preview"><p>确认后将检查并保存此工具的完整文件树，后续使用前会重新核对。</p><code>{draft.path}</code><button className="primary-button" disabled={blocked || needsRecheck} onClick={() => configure(draft.kind, draft.path)}>确认使用所选工具</button><button className="text-button" disabled={pending !== null} onClick={() => setDraft(null)}>取消选择</button></div>}
    {error && <p role="alert" className="task-error">{error}</p>}
    {needsRecheck && <p role="status">请先重新检查开发工具，再决定下一步；系统不会自动重试保存。</p>}
    {notice && <p role="status">{notice}</p>}
  </section>;
}
