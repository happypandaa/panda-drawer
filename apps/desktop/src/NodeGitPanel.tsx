import { invoke } from '@tauri-apps/api/core';
import { useEffect, useId, useRef, useState } from 'react';
import type { ProjectGitStatus } from './ProjectGitPanel';
import type { ProjectGitVersionsResult } from './ProjectGitVersions';
import { nodeGitFailureMessage } from './node-git-error';
import { NodeSourceRestorePanel } from './NodeSourceRestorePanel';

interface NodeGitPanelProps { appId: string; instanceId?: string; sourceRoot?: string; disabled: boolean; runtimeIsSource?: boolean; running?: boolean; onDraftStateChange: (value: { dirty: boolean; pending: boolean }) => void }
export function NodeGitPanel(props: NodeGitPanelProps) {
  return <NodeGitPanelInstance key={props.appId} {...props} />;
}
function NodeGitPanelInstance({ appId, instanceId, sourceRoot, disabled, runtimeIsSource, running, onDraftStateChange }: NodeGitPanelProps) {
  const id = useId();
  const [status, setStatus] = useState<ProjectGitStatus | null>(null);
  const [restoreOpen, setRestoreOpen] = useState(false);
  const [restoreDraft, setRestoreDraft] = useState({ dirty: false, pending: false });
  const [versions, setVersions] = useState<ProjectGitVersionsResult | null>(null);
  const [message, setMessage] = useState('');
  const [name, setName] = useState('');
  const [email, setEmail] = useState('');
  const [branch, setBranch] = useState('');
  const [acceptedAuthor, setAcceptedAuthor] = useState({ name: '', email: '' });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const mounted = useRef(false);
  const pending = useRef(false);
  const generation = useRef(0);
  const callback = useRef(onDraftStateChange); callback.current = onDraftStateChange;
  const current = (sequence: number) => mounted.current && sequence === generation.current;
  const read = async (sequence: number) => {
    const value = await invoke<ProjectGitStatus & { sourceRestore?: { pending: unknown[]; issues: unknown[] } }>('node_development_git', { appId, operation: 'status' });
    if (current(sequence)) { setStatus(value); if (value.sourceRestore?.pending.length || value.sourceRestore?.issues.length) setRestoreOpen(true); }
    return value;
  };
  useEffect(() => {
    mounted.current = true;
    const sequence = ++generation.current;
    pending.current = true; setBusy(true); setStatus(null); setVersions(null); setError(''); setNotice('');
    void read(sequence).catch(cause => { if (current(sequence)) setError(nodeGitFailureMessage(cause, { operation: 'status' })); }).finally(() => { if (current(sequence)) { pending.current = false; setBusy(false); } });
    return () => { mounted.current = false; generation.current++; callback.current({ dirty: false, pending: false }); };
  }, [appId, instanceId, sourceRoot]);
  useEffect(() => { callback.current({ dirty: Boolean(message || branch || name !== acceptedAuthor.name || email !== acceptedAuthor.email || restoreDraft.dirty), pending: busy || restoreDraft.pending }); }, [message, branch, name, email, acceptedAuthor, busy, restoreDraft, instanceId, sourceRoot]);
  async function run(operation: 'status' | 'initialize' | 'save' | 'versions' | 'branch') {
    if (disabled || pending.current || restoreDraft.pending || !mounted.current) return;
    if (operation === 'initialize' && !status?.canInitialize) return;
    if (operation === 'save' && (!status?.canSave || !message.trim() || !name.trim() || !email.trim())) return;
    if (operation === 'branch' && (!status?.canCreateBranch || runtimeIsSource || !branch.trim())) return;
    if (operation === 'versions' && !status?.canListVersions) return;
    const sequence = generation.current;
    let failureOperation = operation;
    pending.current = true; setBusy(true); setError(''); setNotice('');
    try {
      if (operation === 'status') await read(sequence);
      else if (operation === 'versions') {
        const value = await invoke<ProjectGitVersionsResult>('node_development_git', { appId, operation, limit: 50 });
        if (current(sequence)) setVersions(value);
      } else {
        if (!status) return;
        const input = operation === 'initialize' ? { expectedSnapshot: status.snapshotDigest, initialBranch: 'main' }
          : operation === 'save' ? { expectedSnapshot: status.snapshotDigest, expectedHead: status.repository.head?.oid ?? null, message, author: { name, email } }
            : { expectedSnapshot: status.snapshotDigest, expectedHead: status.repository.head?.oid, expectedBranch: status.repository.head?.branch, name: branch };
        const result = await invoke<{ operationOutcome: string }>('node_development_git', { appId, operation, input });
        if (!current(sequence)) return;
        if (result.operationOutcome !== 'committed') throw new Error('版本操作结果待核对，请刷新。');
        if (operation === 'save') { setMessage(''); setAcceptedAuthor({ name, email }); }
        if (operation === 'branch') setBranch('');
        setVersions(null); setNotice(operation === 'save' ? '已保存到本机 Git。' : operation === 'branch' ? '已创建并切换分支。' : 'Git 已初始化。');
        failureOperation = 'status';
        await read(sequence);
      }
    } catch (cause) { if (current(sequence)) { setError(nodeGitFailureMessage(cause, { operation: failureOperation })); setStatus(null); } }
    finally { if (current(sequence)) { pending.current = false; setBusy(false); } }
  }
  const locked = disabled || busy || restoreDraft.pending;
  return <section className="node-git-panel" aria-label="Node 源码版本" aria-busy={busy}>
    <div className="section-heading"><h3>源码版本</h3><button className="refresh-button" disabled={locked} onClick={() => void run('status')}>刷新状态</button></div>
    <p className="project-help">保存到源码仓库。数据和密钥不包含在内，远端备份需另行同步。</p>
    {error && <p className="task-error" role="alert">{error}</p>}{notice && <p role="status">{notice}</p>}
    {status && <>
      <p><strong>{status.repository.head?.branch?.replace(/^refs\/heads\//, '') ?? (status.canInitialize ? '尚未启用 Git' : '未附着分支')}</strong>{status.repository.head?.oid && <code> · {status.repository.head.oid.slice(0, 10)}</code>}</p>
      {status.canInitialize && <button className="refresh-button" disabled={locked} onClick={() => void run('initialize')}>启用 Git</button>}
      <details><summary>{status.changes.length} 项源码变更</summary><ul>{status.changes.map(change => <li key={change.path}><code>{change.path}</code> · {{ add: '新增', modify: '修改', delete: '删除' }[change.kind] ?? change.kind}</li>)}</ul>{status.scope.excluded.length > 0 && <p className="project-help">自动排除配置、密钥、依赖与运行数据。</p>}{status.scope.protectedTrackedPaths.map(path => <p className="task-error" key={path}>仓库已跟踪受保护文件：<code>{path}</code></p>)}</details>
      {status.issues.length > 0 && <details><summary>需要检查的事项</summary>{status.issues.map((issue, index) => <p key={index}>{typeof issue === 'object' && issue && 'message' in issue ? String(issue.message) : String(issue)}</p>)}</details>}
      <form onSubmit={event => { event.preventDefault(); void run('save'); }} className="node-git-save">
        <label htmlFor={`${id}-message`}>版本说明</label><input id={`${id}-message`} value={message} onChange={event => setMessage(event.target.value)} disabled={locked} required maxLength={2000} />
        <div className="node-git-author"><label>提交人<input value={name} onChange={event => setName(event.target.value)} disabled={locked} required autoComplete="name" /></label><label>邮箱<input type="email" value={email} onChange={event => setEmail(event.target.value)} disabled={locked} required autoComplete="email" /></label></div>
        <button className="primary-button" disabled={locked || !status.canSave || !message.trim() || !name.trim() || !email.trim()}>保存源码版本</button>
      </form>
      <div className="project-dialog-actions"><button className="refresh-button" disabled={locked || !status.canListVersions} onClick={() => void run('versions')}>查看版本记录</button></div>
      {versions && <ol className="node-git-history">{versions.versions.map(version => <li key={version.oid}><span>{version.subject || '无版本说明'}</span><code>{version.oid}</code><small>{version.author.name} · {version.authoredAt}</small></li>)}</ol>}
      <details><summary>创建分支</summary><form onSubmit={event => { event.preventDefault(); void run('branch'); }}><label htmlFor={`${id}-branch`}>分支名称</label><input id={`${id}-branch`} value={branch} disabled={locked} onChange={event => setBranch(event.target.value)} required /><button className="refresh-button" disabled={locked || !status.canCreateBranch || runtimeIsSource || !branch.trim()}>创建并切换</button>{runtimeIsSource && <p className="project-help">先构建并更新应用，再切换源码分支。</p>}</form></details>
    </>}
    <details open={restoreOpen || restoreDraft.dirty || restoreDraft.pending} onToggle={event => { if ((restoreDraft.dirty || restoreDraft.pending) && !event.currentTarget.open) event.currentTarget.open = true; else setRestoreOpen(event.currentTarget.open); }}><summary>取回已保存源码</summary>{restoreOpen && <NodeSourceRestorePanel appId={appId} instanceId={instanceId} sourceRoot={sourceRoot ?? status?.repository.root ?? undefined} disabled={disabled || busy} runtimeIsSource={runtimeIsSource} running={running} onDraftStateChange={setRestoreDraft} onChanged={() => { setVersions(null); const sequence = generation.current; void read(sequence).catch(cause => { if (current(sequence)) { setError(nodeGitFailureMessage(cause, { operation: 'status' })); setStatus(null); } }); }} />}</details>
  </section>;
}
