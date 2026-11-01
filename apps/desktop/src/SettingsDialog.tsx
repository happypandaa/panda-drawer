import { invoke } from '@tauri-apps/api/core';
import { Activity, Check, CircleAlert, Copy, Database, Folder, FolderOpen, Moon, Package, RefreshCw, ShieldCheck, Sun, Terminal } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { GitEnvironmentPanel } from './GitEnvironmentPanel';
import { LocalDataCleanupPanel } from './LocalDataCleanupPanel';
import { Modal } from './Modal';
import { PlatformAiIntroButton } from './PlatformAiIntroButton';
import { AboutPlatform, FirstStartGuide } from './PlatformPanels';
import { WorkspaceStoragePanel } from './WorkspaceStoragePanel';
import type { Appearance } from './appearance';
import type { BootstrapInfo, EnvironmentInfo, TaskRecord, WorkspaceInfo, ProjectInfo } from './types';
import { messageFrom } from './ui-utils';
import { useFrontendPlatform } from './frontend-platform';
import type { NodeAppDetail } from './node-app-types';

export type SettingsSection = 'general' | 'storage' | 'runtime' | 'ai' | 'about' | 'uninstall';
const sections: { id: SettingsSection; label: string }[] = [
  { id: 'general', label: '通用' },
  { id: 'storage', label: '存储与清理' },
  { id: 'runtime', label: '运行环境' },
  { id: 'ai', label: '给 AI 的平台说明' },
  { id: 'about', label: '关于与更新' },
];
const resourceNames: Record<keyof BootstrapInfo['resources'], string> = {
  sdk: '网页 SDK API 1', template: 'React 项目模板', readme: '开发说明', cli: '原生 CLI',
};

export interface SettingsDialogProps {
  info: BootstrapInfo | null;
  projects?: ProjectInfo[];
  nodeApps?: NodeAppDetail[];
  loading: boolean;
  checkedAt: string | null;
  theme: Appearance;
  onTheme: (theme: Appearance) => void;
  section: SettingsSection;
  onSection: (section: SettingsSection) => void;
  workspace: WorkspaceInfo | null;
  workspaceBlocked: boolean;
  closeDisabled: boolean;
  localDataLocked: boolean;
  localDataUnavailable: boolean;
  guideEnabled: boolean;
  feedback: ReactNode;
  onClose: () => void;
  onTask: (task: TaskRecord) => void;
  onRefresh: () => Promise<void>;
  revision: number;
  onProjectTask: (task: TaskRecord, project: ProjectInfo) => void;
  isProjectBusy?: (project: ProjectInfo) => boolean;
  onStoragePending?: (pending: boolean) => void;
  onCreateProject: () => void;
  onEnvironmentPending: (pending: boolean) => void;
  onGitPending: (pending: boolean) => void;
  onLocalDataPending: (pending: boolean) => void;
  onLocalDataLock: () => void;
}

export function SettingsDialog(props: SettingsDialogProps) {
  const { info, section } = props;
  const frontendPlatform = useFrontendPlatform();
  const resourcesReady = info && Object.values(info.resources).every(Boolean);
  const [storagePending, setStoragePending] = useState(false);
  const settingsBlocked = props.closeDisabled || storagePending;
  const reportStoragePending = useCallback((pending: boolean) => { setStoragePending(pending); props.onStoragePending?.(pending); }, [props.onStoragePending]);
  const [copyNotice, setCopyNotice] = useState('');
  async function copyDiagnostics() {
    const lines = [
      `程序：${info?.displayName ?? '尚未读取'} ${info?.version ?? ''}`, `应用标识：${info?.identifier ?? '尚未读取'}`, `实例 ID：${info?.instanceId ?? '尚未读取'}`,
      `接口版本：${info ? `v${info.apiVersion}` : '尚未读取'}`, `本地服务：${info ? info.baseUrl : '未读取'}`, `内置 SQLite：${info?.sqliteVersion ?? '未读取'}`,
      `开发数据目录：${info?.dataRoot ?? '尚未读取'}`, `项目工作区：${info?.workspaceRoot ?? '尚未读取'}`, `CLI 路径：${info ? info.cliPath ?? '未找到包内 CLI' : '尚未读取'}`,
      `包内资源：${info ? (Object.entries(resourceNames) as [keyof BootstrapInfo['resources'], string][]).map(([key, label]) => `${label} ${info.resources[key] ? '正常' : '缺失'}`).join('，') : '尚未读取'}`,
    ];
    try { await navigator.clipboard.writeText(lines.join('\n')); setCopyNotice('诊断信息已复制。'); } catch { setCopyNotice('无法自动复制，请选中下方内容手动复制。'); }
  }
  const locked = props.localDataLocked;
  const panel = (id: SettingsSection, children: ReactNode) => locked && id !== 'uninstall' ? null : <div className="settings-panel" role="tabpanel" id={`settings-${id}-panel`} aria-labelledby={locked ? undefined : `settings-${id}-tab`} hidden={!locked && section !== id}>{children}</div>;
  return <Modal title="设置" className="radio-settings-modal" wide closeDisabled={settingsBlocked} onClose={props.onClose}>
    {props.feedback}
    {/* One stable tree: a terminal local-data lock must not remount its panel. */}
    <div className={`settings-layout${props.localDataLocked ? ' is-locked' : ''}`}>
      {!props.localDataLocked && <nav className="settings-nav" aria-label="设置分区"><div role="tablist" aria-orientation="vertical" aria-label="设置分区">
        {sections.map(({ id, label }) => <button key={id} id={`settings-${id}-tab`} type="button" role="tab" aria-selected={section === id} aria-controls={`settings-${id}-panel`} disabled={settingsBlocked} onClick={() => props.onSection(id)}>{label}</button>)}
        <hr />
        <button id="settings-uninstall-tab" type="button" role="tab" className="settings-nav-danger" aria-selected={section === 'uninstall'} aria-controls="settings-uninstall-panel" disabled={settingsBlocked} onClick={() => props.onSection('uninstall')}>卸载前清除数据</button>
      </div></nav>}
      <div className="settings-content">
        {panel('general', <section className="settings-section" aria-labelledby="settings-general-title">
          <h3 id="settings-general-title">通用</h3>
          <div className="settings-row"><div><strong>外观</strong><p>设置窗口和项目列表使用同一主题。</p></div><div className="radio-theme-switch" role="group" aria-label="外观主题">
            <button aria-label="切换亮色主题" title="浅色" aria-pressed={props.theme === 'light'} onClick={() => props.onTheme('light')}><Sun size={15} aria-hidden="true" />浅色</button>
            <button aria-label="切换暗色主题" title="深色" aria-pressed={props.theme === 'dark'} onClick={() => props.onTheme('dark')}><Moon size={15} aria-hidden="true" />深色</button>
          </div></div>
          <details className="radio-settings-guide"><summary>使用指南</summary><FirstStartGuide enabled={props.guideEnabled} onCreate={props.onCreateProject} onEnvironment={() => { if (!settingsBlocked) props.onSection('runtime'); }} onStorage={() => { if (!settingsBlocked) props.onSection('storage'); }} /></details>
          <div className="runtime-explainer"><ShieldCheck size={17} /><p>{frontendPlatform.exitExplanation}</p></div>
        </section>)}
        {panel('storage', <WorkspaceStoragePanel instanceId={info?.instanceId} workspace={props.workspace} dataRoot={info?.dataRoot} revision={props.revision} enabled={Boolean(info) && !props.localDataLocked} active={section === 'storage'} migrating={props.workspaceBlocked} projects={props.projects ?? []} nodeApps={props.nodeApps ?? []} isProjectBusy={props.isProjectBusy} onTask={props.onTask} onProjectTask={props.onProjectTask} onRefresh={props.onRefresh} onPendingChange={reportStoragePending} />)}
        {panel('runtime', <section className="settings-section runtime-view" aria-labelledby="settings-runtime-title">
          <h3 id="settings-runtime-title">运行环境</h3>
          <p className="settings-lead">只影响项目构建和从 Git 添加；应用运行使用平台自带的 Node，不受这里影响。</p>
          <BuildEnvironment enabled={Boolean(info)} onPendingChange={props.onEnvironmentPending} />
          <GitEnvironmentPanel enabled={Boolean(info)} onPendingChange={props.onGitPending} />
          <section className="settings-section" aria-labelledby="settings-diagnostics-title">
          <div className="section-heading"><div><h3 id="settings-diagnostics-title">诊断信息</h3><p>遇到问题时复制给开发者或 AI。</p></div><button className="refresh-button" onClick={() => void copyDiagnostics()}><Copy size={14} />复制全部诊断信息</button></div>
          {copyNotice && <p className="workspace-inline-notice" role="status">{copyNotice}</p>}
          <dl className="radio-settings-summary"><div><dt>平台服务</dt><dd>{info ? '已就绪' : props.loading ? '读取中…' : '未连接'}</dd></div><div><dt>包内资源</dt><dd>{info ? resourcesReady ? '读取正常' : '需要检查' : '尚未读取'}</dd></div></dl>
          <section className="status-grid" aria-label="运行状态">
            <StatusCard icon={<Activity size={18} />} label="本地服务" value={info ? '已启动' : props.loading ? '读取中' : '未读取'} detail={info ? info.baseUrl : '由 Rust 提供本机服务'} good={Boolean(info)} />
            <StatusCard icon={<Database size={18} />} label="内置 SQLite" value={info ? `SQLite ${info.sqliteVersion}` : props.loading ? '读取中' : '未读取'} detail="每个项目使用独立数据库文件" good={Boolean(info)} />
            <StatusCard icon={<Package size={18} />} label="包内资源" value={info ? resourcesReady ? '读取正常' : '需要检查' : props.loading ? '读取中' : '未读取'} detail="CLI、SDK 发布包与 React 模板" good={Boolean(resourcesReady)} />
          </section>
          <section className="diagnostics-card" aria-labelledby="diagnostics-title">
            <div className="section-heading"><div><h2 id="diagnostics-title">运行环境</h2><p>实际目录与实例信息，供开发和打包验证使用。</p></div><span className="updated-at">{props.checkedAt ? `读取于 ${props.checkedAt}` : '等待桌面连接'}</span></div>
            <dl className="path-list"><PathRow icon={<Folder size={16} />} label="开发数据目录" value={info?.dataRoot} /><PathRow icon={<FolderOpen size={16} />} label="项目工作区" value={info?.workspaceRoot} /><PathRow icon={<Terminal size={16} />} label="CLI 路径" value={info ? info.cliPath ?? '未找到包内 CLI' : undefined} /></dl>
            <div className="runtime-details"><div><span>运行程序</span><code>{info?.displayName ?? '尚未读取'}</code></div><div><span>应用标识</span><code>{info?.identifier ?? '尚未读取'}</code></div><div><span>实例 ID</span><code>{info?.instanceId ?? '尚未读取'}</code></div><div><span>接口版本</span><code>{info ? `v${info.apiVersion}` : '尚未读取'}</code></div></div>
            <div className="resource-strip"><span>资源探测</span>{(Object.entries(resourceNames) as [keyof BootstrapInfo['resources'], string][]).map(([key, label]) => <span className={`resource-item ${info ? info.resources[key] ? 'resource-ok' : 'resource-missing' : ''}`} key={key}>{info ? info.resources[key] ? <Check size={13} /> : <CircleAlert size={13} /> : <span className="unknown-dot" />}{label}</span>)}</div>
          </section>
          </section>
        </section>)}
        {panel('ai', <section className="settings-section" aria-labelledby="settings-ai-title"><h3 id="settings-ai-title">给 AI 的平台说明</h3><p>新项目开工前，或判断已有项目能否接入时，把平台说明交给 AI。</p><PlatformAiIntroButton schemaPath={typeof info?.platform?.manifestSchemaPath === 'string' ? info.platform.manifestSchemaPath : undefined} platformVersion={info?.version} /></section>)}
        {panel('about', <section className="settings-section radio-settings-about" aria-labelledby="settings-about-title">
          <h3 id="settings-about-title">关于与更新</h3>
          <div className="radio-about-brand"><img src="/panda-app-drawer.png" alt="" /><div>熊猫小抽屉<span>{info?.version ?? '尚未读取'} · 开发版</span></div></div>
          <p>项目数据保存在本机；联网功能由具体应用决定。</p>
          <AboutPlatform info={info} />
        </section>)}
        {panel('uninstall', <section className="settings-section" aria-labelledby="settings-uninstall-title">
          <h3 id="settings-uninstall-title">卸载前清除数据</h3>
          <p className="settings-lead">通常在卸载熊猫小抽屉之前使用。只想释放空间请用“存储与清理”。</p>
          <LocalDataCleanupPanel managedProjects={[...(props.projects ?? []).map(project => ({ name: project.name, path: project.projectRoot })), ...(props.nodeApps ?? []).filter(app => app.ownership === 'managed').map(app => ({ name: app.name, path: app.objectLayout?.source.path ?? app.sourcePath }))]} externalProjectCount={(props.nodeApps ?? []).filter(app => app.ownership !== 'managed').length} instanceId={info?.instanceId} scopeIdentity={JSON.stringify([info?.dataRoot, info?.workspaceRoot])} disabled={props.localDataUnavailable} onPendingChange={props.onLocalDataPending} onTerminalLock={props.onLocalDataLock} />
        </section>)}
      </div>
    </div>
    {!props.localDataLocked && <div className="radio-settings-footer"><span>熊猫小抽屉</span><button className="refresh-button" disabled={settingsBlocked} onClick={props.onClose}>完成</button></div>}
  </Modal>;
}

function BuildEnvironment({ enabled, onPendingChange }: { enabled: boolean; onPendingChange?: (pending: boolean) => void }) {
  const frontendPlatform = useFrontendPlatform();
  const [environment, setEnvironment] = useState<EnvironmentInfo | null>(null);
  const [nodePath, setNodePath] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pending = useRef(false);
  const request = useRef(0);
  const probe = useCallback(async () => {
    if (!enabled || pending.current) return;
    const id = ++request.current;
    pending.current = true;
    onPendingChange?.(true);
    setLoading(true);
    try {
      const value = await invoke<EnvironmentInfo>('environment_doctor');
      if (id !== request.current) return;
      setEnvironment(value);
      setNodePath(value.configuredNodePath ?? '');
      setError(null);
    } catch (cause) { if (id === request.current) setError(messageFrom(cause)); }
    finally { pending.current = false; onPendingChange?.(false); if (id === request.current) setLoading(false); }
  }, [enabled, onPendingChange]);
  useEffect(() => { void probe(); }, [probe]);
  async function configure(path: string | null) {
    if (pending.current) return;
    pending.current = true;
    onPendingChange?.(true);
    setLoading(true);
    try {
      const value = await invoke<EnvironmentInfo>('environment_node_set', { nodePath: path });
      setEnvironment(value);
      setNodePath(value.configuredNodePath ?? '');
      setError(null);
    } catch (cause) { setError(messageFrom(cause)); }
    finally { pending.current = false; onPendingChange?.(false); setLoading(false); }
  }
  return <section className="diagnostics-card build-environment" aria-labelledby="build-environment-title">
    <div className="section-heading"><div><h2 id="build-environment-title">Node 构建环境</h2><p>构建使用配套 Node/npm，可明确选择兼容的本机环境。应用运行 Node 不受这里的构建选择影响。</p></div><button className="refresh-button" onClick={() => void probe()} disabled={!enabled || loading}><RefreshCw size={14} />{loading ? '检测中…' : '重新检测'}</button></div>
    <div className="environment-status"><span className={`project-state state-${environment?.ready ? 'ready' : 'unbuilt'}`}><span />{environment ? environment.ready ? '可以构建' : '构建环境需要配置' : loading ? '正在检测' : '尚未检测'}</span><span>最低 22.12 · 支持 22 / 24 · 推荐 24 LTS</span></div>
    <p>当前来源：{environment ? ({ managed: '平台受管 Node/npm', explicit: '明确选择的本机 Node/npm', detected: '本机自动探测' })[environment.selection] : '尚未读取'}。</p>
    <dl className="path-list">
      <PathRow icon={<Terminal size={16} />} label={`Node ${environment?.node?.version ?? ''}`} value={environment ? environment.node?.path ?? '未找到可用 Node' : undefined} />
      <PathRow icon={<Terminal size={16} />} label={`npm ${environment?.npm?.version ?? ''}`} value={environment ? environment.npm?.path ?? '未找到配套 npm' : undefined} />
      {environment?.managedNodePath && <PathRow icon={<Terminal size={16} />} label="默认受管 Node 位置" value={environment.managedNodePath} />}
    </dl>
    {environment?.managedNodePath && <p>默认受管位置只说明工具所在位置；是否可用以本次检测结果为准。</p>}
    {environment?.issues.map((issue) => <p key={issue.code} className="environment-issue">{issue.message} ({issue.code})</p>)}
    {environment && !environment.ready && (environment.selection === 'managed'
      ? <p className="environment-issue">平台配套 Node/npm 暂不可用。请修复或重新安装平台，或明确选择兼容的本机 Node/npm。</p>
      : <p className="environment-issue">如需安装兼容的本机 Node/npm，可参考：<code>{environment.downloadUrl}</code></p>)}
    {environment?.selection === 'explicit' && !environment.ready && <p className="environment-issue">已保留你明确选择的路径，失效时不会自动改用其他工具。请修正路径，或点击“恢复默认工具”。</p>}
    <form className="node-path-form" onSubmit={(event) => { event.preventDefault(); const path = nodePath.trim(); if (frontendPlatform.isAbsolutePath(path)) void configure(path); }}>
      <label htmlFor="node-path">指定本机 Node 可执行文件的绝对路径<span className="optional-label">可选</span></label>
      <div className="node-path-controls"><input id="node-path" value={nodePath} onChange={(event) => setNodePath(event.target.value)} placeholder={frontendPlatform.nodePathPlaceholder} disabled={loading} autoComplete="off" /><button className="primary-button" type="submit" disabled={!enabled || loading || !frontendPlatform.isAbsolutePath(nodePath.trim())}>保存路径</button><button className="refresh-button" type="button" disabled={!enabled || loading} onClick={() => void configure(null)}>恢复默认工具</button></div>
      <p>{environment ? environment.managedNodePath ? '恢复默认工具会清除显式路径，使用平台配套 Node/npm。' : '此环境没有提供受管工具位置；恢复默认工具会清除显式路径并自动探测本机环境。' : '读取环境后可查看默认工具来源。'} 路径只保存在此开发版的本机设置中。</p>
    </form>
    {error && <p role="alert" className="environment-issue">{error}</p>}
  </section>;
}

function StatusCard({ icon, label, value, detail, good }: { icon: ReactNode; label: string; value: string; detail: string; good: boolean }) {
  return <div className="status-card"><div className="status-card-label">{icon}<span>{label}</span>{good && <Check size={14} className="status-check" aria-label="已验证" />}</div><strong>{value}</strong><p>{detail}</p></div>;
}
function PathRow({ icon, label, value }: { icon: ReactNode; label: string; value: string | undefined }) {
  return <div className="path-row"><dt>{icon}{label}</dt><dd><code>{value ?? '尚未读取'}</code></dd></div>;
}
