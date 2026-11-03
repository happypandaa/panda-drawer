import { t } from './i18n';
import { invoke } from '@tauri-apps/api/core';
import { RefreshCw, Terminal } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { GitRemoteCheckPanel } from './GitRemoteCheckPanel';
import { messageFrom } from './ui-utils';
import { useFrontendPlatform } from './frontend-platform';

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
  const frontendPlatform = useFrontendPlatform();
  const [environment, setEnvironment] = useState<GitEnvironmentInfo | null>(null);
  const [gitPath, setGitPath] = useState('');
  const [loading, setLoading] = useState(false);
  const [remoteLoading, setRemoteLoading] = useState(false);
  const [toolRevision, setToolRevision] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const pending = useRef(false);
  const remotePending = useRef(false);
  const mounted = useRef(true);

  const remotePendingChange = useCallback((value: boolean) => {
    remotePending.current = value;
    if (mounted.current) setRemoteLoading(value);
    onPendingChange?.(pending.current || value);
  }, [onPendingChange]);

  const request = useCallback(async (path?: string | null) => {
    if (!enabled || pending.current || remotePending.current) return;
    pending.current = true;
    setLoading(true);
    setToolRevision(revision => revision + 1);
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
      onPendingChange?.(remotePending.current);
      if (mounted.current) setLoading(false);
    }
  }, [enabled, onPendingChange]);

  useEffect(() => {
    mounted.current = true;
    void request();
    return () => { mounted.current = false; };
  }, [request]);

  const busy = loading || remoteLoading;

  return <section className="diagnostics-card git-environment" aria-labelledby="git-environment-title">
    <div className="section-heading"><div><h2 id="git-environment-title">{t("Git 开发工具")}</h2><p>{t("当前可检测和选择 Git 工具，并明确检查公开仓库访问。仓库获取与版本操作尚未接入；应用日常运行不依赖 Git。")}</p></div><button className="refresh-button" onClick={() => void request()} disabled={!enabled || busy}><RefreshCw size={14} />{loading ? t("检测中…") : t("重新检测")}</button></div>
    <div className="environment-status"><span className={`project-state state-${environment?.ready ? 'ready' : 'unbuilt'}`}><span />{environment ? environment.selection === 'unresolved' ? t("Git 工具尚未核验") : environment.ready ? t("Git 工具可用") : t("Git 工具不可用") : loading ? t("正在检测") : t("尚未检测")}</span></div>
    <p>{t("当前来源：")}{environment ? ({ managed: t("平台受管 Git"), explicit: t("明确选择的本机 Git"), unresolved: t("尚未确定（选择设置无法读取）") })[environment.selection] : t("尚未读取")}{t("。")}</p>
    <dl className="path-list">
      <ToolPath label={`Git ${environment?.git?.version ?? ''}`} value={environment ? environment.git?.path ?? (environment.selection === 'unresolved' ? t("尚未核验工具路径") : t("未找到可用 Git")) : t("尚未读取")} />
      {environment?.managedGitPath && <ToolPath label={t("默认受管 Git 位置")} value={environment.managedGitPath} />}
    </dl>
    {environment?.managedGitPath && <p>{t("默认位置不代表工具可用，以本次检测结果为准。")}</p>}
    {environment?.issues.map(issue => <p key={issue.code} className="environment-issue">{t(issue.message)} ({issue.code})</p>)}
    {environment && !environment.ready && <p className="environment-issue">{({ explicit: t("已保留你明确选择的路径，失效时不会自动更换工具。请修正路径或恢复默认工具。"), managed: t("平台配套 Git 暂不可用。请修复平台安装，或明确选择兼容的本机 Git。"), unresolved: t("无法读取或解析 Git 工具选择设置，尚未选用默认工具。可填写并保存兼容的本机 Git 路径，或明确恢复默认工具。") })[environment.selection]}</p>}
    <form className="node-path-form" onSubmit={event => { event.preventDefault(); const path = gitPath.trim(); if (frontendPlatform.isAbsolutePath(path)) void request(path); }}>
      <label htmlFor="git-path">{t("指定本机 Git 可执行文件的绝对路径")}<span className="optional-label">{t("可选")}</span></label>
      <div className="node-path-controls"><input id="git-path" value={gitPath} onChange={event => setGitPath(event.target.value)} placeholder={t(frontendPlatform.gitPathPlaceholder)} disabled={busy} autoComplete="off" /><button className="primary-button" type="submit" disabled={!enabled || busy || !frontendPlatform.isAbsolutePath(gitPath.trim())}>{t("保存路径")}</button><button className="refresh-button" type="button" disabled={!enabled || busy} onClick={() => void request(null)}>{t("恢复默认工具")}</button></div>
      <p>{t("恢复默认会清除显式路径并使用平台配套 Git。此设置只保存在当前开发版，不修改全局 Git 配置。")}</p>
    </form>
    {environment && <div className="git-authentication">
      <h3>{t("SSH 工具与认证")}</h3>
      <p>{environment.ssh.ready ? t("已找到系统 SSH 工具。") : t("系统 SSH 工具暂不可用。")} {t(" 这不代表账号认证或仓库访问已通过。")}</p>
      {environment.ssh.executable && <dl className="path-list"><ToolPath label={environment.ssh.executable.version} value={environment.ssh.executable.path} /></dl>}
      {environment.ssh.issues.map(issue => <p key={issue.code} className="environment-issue">{t(issue.message)} ({issue.code})</p>)}
      <p>{t("认证状态：未验证。")}{t(environment.authentication.reason)}</p>
    </div>}
    {error && <p role="alert" className="environment-issue">{t(error)}</p>}
    <GitRemoteCheckPanel enabled={enabled && Boolean(environment?.ready) && !loading} toolRevision={toolRevision} onPendingChange={remotePendingChange} />
  </section>;
}

function ToolPath({ label, value }: { label: string; value: string }) {
  return <div className="path-row"><dt><Terminal size={16} />{label}</dt><dd><code>{value}</code></dd></div>;
}
