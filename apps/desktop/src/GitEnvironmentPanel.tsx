import { invoke } from '@tauri-apps/api/core';
import { RefreshCw, Terminal } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { messageFrom } from './ui-utils';

type ToolIssue = { code: string; message: string };
type ExecutableInfo = { path: string; version: string };
export interface GitEnvironmentInfo {
  ready: boolean;
  git: ExecutableInfo | null;
  selection: 'managed' | 'explicit' | 'unresolved';
  configuredGitPath: string | null;
  managedGitPath: string | null;
  ssh: { ready: boolean; selection: 'system'; executable: ExecutableInfo | null; issues: ToolIssue[] };
  authentication: { state: 'not-checked'; reason: string };
  issues: ToolIssue[];
}

export function GitEnvironmentPanel({ enabled, onPendingChange }: { enabled: boolean; onPendingChange?: (pending: boolean) => void }) {
  const [environment, setEnvironment] = useState<GitEnvironmentInfo | null>(null);
  const [gitPath, setGitPath] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pending = useRef(false);
  const mounted = useRef(true);

  const request = useCallback(async (path?: string | null) => {
    if (!enabled || pending.current) return;
    pending.current = true;
    setLoading(true);
    onPendingChange?.(true);
    try {
      const value = path === undefined
        ? await invoke<GitEnvironmentInfo>('environment_git_doctor')
        : await invoke<GitEnvironmentInfo>('environment_git_set', { gitPath: path });
      if (!mounted.current) return;
      setEnvironment(value);
      setGitPath(value.configuredGitPath ?? '');
      setError(null);
    } catch (cause) {
      if (mounted.current) setError(messageFrom(cause));
    } finally {
      pending.current = false;
      onPendingChange?.(false);
      if (mounted.current) setLoading(false);
    }
  }, [enabled, onPendingChange]);

  useEffect(() => {
    mounted.current = true;
    void request();
    return () => { mounted.current = false; };
  }, [request]);

  return <section className="diagnostics-card git-environment" aria-labelledby="git-environment-title">
    <div className="section-heading"><div><h2 id="git-environment-title">Git 开发工具</h2><p>当前可检测和选择 Git 工具。仓库获取与版本操作尚未接入；应用日常运行不依赖 Git。</p></div><button className="refresh-button" onClick={() => void request()} disabled={!enabled || loading}><RefreshCw size={14} />{loading ? '检测中…' : '重新检测'}</button></div>
    <div className="environment-status"><span className={`project-state state-${environment?.ready ? 'ready' : 'unbuilt'}`}><span />{environment ? environment.selection === 'unresolved' ? 'Git 工具尚未核验' : environment.ready ? 'Git 工具可用' : 'Git 工具不可用' : loading ? '正在检测' : '尚未检测'}</span></div>
    <p>当前来源：{environment ? ({ managed: '平台受管 Git', explicit: '明确选择的本机 Git', unresolved: '尚未确定（选择设置无法读取）' })[environment.selection] : '尚未读取'}。</p>
    <dl className="path-list">
      <ToolPath label={`Git ${environment?.git?.version ?? ''}`} value={environment ? environment.git?.path ?? (environment.selection === 'unresolved' ? '尚未核验工具路径' : '未找到可用 Git') : '尚未读取'} />
      {environment?.managedGitPath && <ToolPath label="默认受管 Git 位置" value={environment.managedGitPath} />}
    </dl>
    {environment?.managedGitPath && <p>默认位置不代表工具可用，以本次检测结果为准。</p>}
    {environment?.issues.map(issue => <p key={issue.code} className="environment-issue">{issue.message} ({issue.code})</p>)}
    {environment && !environment.ready && <p className="environment-issue">{({ explicit: '已保留你明确选择的路径，失效时不会自动更换工具。请修正路径或恢复默认工具。', managed: '平台配套 Git 暂不可用。请修复平台安装，或明确选择兼容的本机 Git。', unresolved: '无法读取或解析 Git 工具选择设置，尚未选用默认工具。可填写并保存兼容的本机 Git 路径，或明确恢复默认工具。' })[environment.selection]}</p>}
    <form className="node-path-form" onSubmit={event => { event.preventDefault(); void request(gitPath.trim()); }}>
      <label htmlFor="git-path">指定本机 Git 可执行文件的绝对路径<span className="optional-label">可选</span></label>
      <div className="node-path-controls"><input id="git-path" value={gitPath} onChange={event => setGitPath(event.target.value)} placeholder="例如 /usr/bin/git" disabled={loading} autoComplete="off" /><button className="primary-button" type="submit" disabled={!enabled || loading || !gitPath.trim().startsWith('/')}>保存路径</button><button className="refresh-button" type="button" disabled={!enabled || loading} onClick={() => void request(null)}>恢复默认工具</button></div>
      <p>恢复默认会清除显式路径并使用平台配套 Git。此设置只保存在当前开发版，不修改全局 Git 配置。</p>
    </form>
    {environment && <div className="git-authentication">
      <h3>SSH 工具与认证</h3>
      <p>{environment.ssh.ready ? '已找到系统 SSH 工具。' : '系统 SSH 工具暂不可用。'} 这不代表账号认证或仓库访问已通过。</p>
      {environment.ssh.executable && <dl className="path-list"><ToolPath label={environment.ssh.executable.version} value={environment.ssh.executable.path} /></dl>}
      {environment.ssh.issues.map(issue => <p key={issue.code} className="environment-issue">{issue.message} ({issue.code})</p>)}
      <p>认证状态：未验证。{environment.authentication.reason}</p>
    </div>}
    {error && <p role="alert" className="environment-issue">{error}</p>}
  </section>;
}

function ToolPath({ label, value }: { label: string; value: string }) {
  return <div className="path-row"><dt><Terminal size={16} />{label}</dt><dd><code>{value}</code></dd></div>;
}
