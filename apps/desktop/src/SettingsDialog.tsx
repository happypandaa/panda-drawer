import { t, useLocale, supportedLocales } from './i18n';
import { invoke } from '@tauri-apps/api/core';
import { Activity, Check, CircleAlert, Copy, Database, Folder, FolderOpen, Moon, Package, RefreshCw, ShieldCheck, Sun, Terminal } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { GitEnvironmentPanel } from './GitEnvironmentPanel';
import { LocalDataCleanupPanel } from './LocalDataCleanupPanel';
import { Modal } from './Modal';
import { PlatformAiIntroButton } from './PlatformAiIntroButton';
import { AiToolSettings } from './AiToolsProvider';
import { DownloadRouteSettings } from './DownloadRouteSettings';
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
  const { locale, setLocale, localeError } = useLocale();
  const frontendPlatform = useFrontendPlatform();
  const resourcesReady = info && Object.values(info.resources).every(Boolean);
  const [storagePending, setStoragePending] = useState(false);
  const settingsBlocked = props.closeDisabled || storagePending;
  const reportStoragePending = useCallback((pending: boolean) => { setStoragePending(pending); props.onStoragePending?.(pending); }, [props.onStoragePending]);
  const [copyNotice, setCopyNotice] = useState('');
  async function copyDiagnostics() {
    const lines = [
      t("程序：{value0} {value1}", { value0: t(info?.displayName ?? "尚未读取"), value1: info?.version ?? '' }), t("应用标识：{value0}", { value0: info?.identifier ?? t("尚未读取") }), t("实例 ID：{value0}", { value0: info?.instanceId ?? t("尚未读取") }),
      t("接口版本：{value0}", { value0: info ? `v${info.apiVersion}` : t("尚未读取") }), t("本地服务：{value0}", { value0: info ? info.baseUrl : t("未读取") }), t("内置 SQLite：{value0}", { value0: info?.sqliteVersion ?? t("未读取") }),
      t("开发数据目录：{value0}", { value0: info?.dataRoot ?? t("尚未读取") }), t("项目工作区：{value0}", { value0: info?.workspaceRoot ?? t("尚未读取") }), t("CLI 路径：{value0}", { value0: info ? info.cliPath ?? t("未找到包内 CLI") : t("尚未读取") }),
      t("包内资源：{value0}", { value0: info ? (Object.entries(resourceNames) as [keyof BootstrapInfo['resources'], string][]).map(([key, label]) => `${t(label)} ${info.resources[key] ? t("正常") : t("缺失")}`).join(t('，')) : t("尚未读取") }),
    ];
    try { await navigator.clipboard.writeText(lines.join('\n')); setCopyNotice(t("诊断信息已复制。")); } catch { setCopyNotice(t("无法自动复制，请选中下方内容手动复制。")); }
  }
  const locked = props.localDataLocked;
  const panel = (id: SettingsSection, children: ReactNode) => locked && id !== 'uninstall' ? null : <div className="settings-panel" role="tabpanel" id={`settings-${id}-panel`} aria-labelledby={locked ? undefined : `settings-${id}-tab`} hidden={!locked && section !== id}>{children}</div>;
  return <Modal title={t("设置")} className="radio-settings-modal" wide closeDisabled={settingsBlocked} onClose={props.onClose}>
    {props.feedback}
    {/* One stable tree: a terminal local-data lock must not remount its panel. */}
    <div className={`settings-layout${props.localDataLocked ? ' is-locked' : ''}`}>
      {!props.localDataLocked && <nav className="settings-nav" aria-label={t("设置分区")}><div role="tablist" aria-orientation="vertical" aria-label={t("设置分区")}>
        {sections.map(({ id, label }) => <button key={id} id={`settings-${id}-tab`} type="button" role="tab" aria-selected={section === id} aria-controls={`settings-${id}-panel`} disabled={settingsBlocked} onClick={() => props.onSection(id)}>{t(label)}</button>)}
        <hr />
        <button id="settings-uninstall-tab" type="button" role="tab" className="settings-nav-danger" aria-selected={section === 'uninstall'} aria-controls="settings-uninstall-panel" disabled={settingsBlocked} onClick={() => props.onSection('uninstall')}>{t("卸载前清除数据")}</button>
      </div></nav>}
      <div className="settings-content">
        {panel('general', <section className="settings-section" aria-labelledby="settings-general-title">
          <h3 id="settings-general-title">{t("通用")}</h3>
          <div className="settings-row"><div><strong>{t("外观")}</strong><p>{t("设置窗口和项目列表使用同一主题。")}</p></div><div className="radio-theme-switch" role="group" aria-label={t("外观主题")}>
            <button aria-label={t("切换亮色主题")} title={t("浅色")} aria-pressed={props.theme === 'light'} onClick={() => props.onTheme('light')}><Sun size={15} aria-hidden="true" />{t("浅色")}</button>
            <button aria-label={t("切换暗色主题")} title={t("深色")} aria-pressed={props.theme === 'dark'} onClick={() => props.onTheme('dark')}><Moon size={15} aria-hidden="true" />{t("深色")}</button>
          </div></div>
          <div className="settings-row"><div><strong>{t("语言")}</strong><p>{t("选择后立即生效，不跟随系统语言。")}</p></div><select aria-label={t("界面语言")} value={locale} onChange={event => setLocale(event.target.value as typeof locale)}>{supportedLocales.map(language => <option key={language.id} value={language.id}>{language.label}</option>)}</select></div>
          {localeError && <p className="environment-issue" role="alert">{t(localeError)}</p>}
          <DownloadRouteSettings disabled={settingsBlocked} />
          <details className="radio-settings-guide"><summary>{t("使用指南")}</summary><FirstStartGuide enabled={props.guideEnabled} onCreate={props.onCreateProject} onEnvironment={() => { if (!settingsBlocked) props.onSection('runtime'); }} onStorage={() => { if (!settingsBlocked) props.onSection('storage'); }} /></details>
          <div className="runtime-explainer"><ShieldCheck size={17} /><p>{t(frontendPlatform.exitExplanation)}</p></div>
        </section>)}
        {panel('storage', <WorkspaceStoragePanel instanceId={info?.instanceId} workspace={props.workspace} dataRoot={info?.dataRoot} revision={props.revision} enabled={Boolean(info) && !props.localDataLocked} active={section === 'storage'} migrating={props.workspaceBlocked} projects={props.projects ?? []} nodeApps={props.nodeApps ?? []} isProjectBusy={props.isProjectBusy} onTask={props.onTask} onProjectTask={props.onProjectTask} onRefresh={props.onRefresh} onPendingChange={reportStoragePending} />)}
        {panel('runtime', <section className="settings-section runtime-view" aria-labelledby="settings-runtime-title">
          <h3 id="settings-runtime-title">{t("运行环境")}</h3>
          <p className="settings-lead">{t("只影响项目构建和从 Git 添加；应用运行使用平台自带的 Node，不受这里影响。")}</p>
          <BuildEnvironment enabled={Boolean(info)} onPendingChange={props.onEnvironmentPending} />
          <GitEnvironmentPanel enabled={Boolean(info)} onPendingChange={props.onGitPending} />
          <section className="settings-section" aria-labelledby="settings-diagnostics-title">
          <div className="section-heading"><div><h3 id="settings-diagnostics-title">{t("诊断信息")}</h3><p>{t("遇到问题时复制给开发者或 AI。")}</p></div><button className="refresh-button" onClick={() => void copyDiagnostics()}><Copy size={14} />{t("复制全部诊断信息")}</button></div>
          {copyNotice && <p className="workspace-inline-notice" role="status">{t(copyNotice)}</p>}
          <dl className="radio-settings-summary"><div><dt>{t("平台服务")}</dt><dd>{info ? t("已就绪") : props.loading ? t("读取中…") : t("未连接")}</dd></div><div><dt>{t("包内资源")}</dt><dd>{info ? resourcesReady ? t("读取正常") : t("需要检查") : t("尚未读取")}</dd></div></dl>
          <section className="status-grid" aria-label={t("运行状态")}>
            <StatusCard icon={<Activity size={18} />} label={t("本地服务")} value={info ? t("已启动") : props.loading ? t("读取中") : t("未读取")} detail={info ? info.baseUrl : t("由 Rust 提供本机服务")} good={Boolean(info)} />
            <StatusCard icon={<Database size={18} />} label={t("内置 SQLite")} value={info ? `SQLite ${info.sqliteVersion}` : props.loading ? t("读取中") : t("未读取")} detail={t("每个项目使用独立数据库文件")} good={Boolean(info)} />
            <StatusCard icon={<Package size={18} />} label={t("包内资源")} value={info ? resourcesReady ? t("读取正常") : t("需要检查") : props.loading ? t("读取中") : t("未读取")} detail={t("CLI、SDK 发布包与 React 模板")} good={Boolean(resourcesReady)} />
          </section>
          <section className="diagnostics-card" aria-labelledby="diagnostics-title">
            <div className="section-heading"><div><h2 id="diagnostics-title">{t("运行环境")}</h2><p>{t("实际目录与实例信息，供开发和打包验证使用。")}</p></div><span className="updated-at">{props.checkedAt ? t("读取于 {value0}", { value0: props.checkedAt }) : t("等待桌面连接")}</span></div>
            <dl className="path-list"><PathRow icon={<Folder size={16} />} label={t("开发数据目录")} value={info?.dataRoot} /><PathRow icon={<FolderOpen size={16} />} label={t("项目工作区")} value={info?.workspaceRoot} /><PathRow icon={<Terminal size={16} />} label={t("CLI 路径")} value={info ? info.cliPath ?? t("未找到包内 CLI") : undefined} /></dl>
            <div className="runtime-details"><div><span>{t("运行程序")}</span><code>{t(info?.displayName ?? "尚未读取")}</code></div><div><span>{t("应用标识")}</span><code>{info?.identifier ?? t("尚未读取")}</code></div><div><span>{t("实例 ID")}</span><code>{info?.instanceId ?? t("尚未读取")}</code></div><div><span>{t("接口版本")}</span><code>{info ? `v${info.apiVersion}` : t("尚未读取")}</code></div></div>
            <div className="resource-strip"><span>{t("资源探测")}</span>{(Object.entries(resourceNames) as [keyof BootstrapInfo['resources'], string][]).map(([key, label]) => <span className={`resource-item ${info ? info.resources[key] ? 'resource-ok' : 'resource-missing' : ''}`} key={key}>{info ? info.resources[key] ? <Check size={13} /> : <CircleAlert size={13} /> : <span className="unknown-dot" />}{t(label)}</span>)}</div>
          </section>
          </section>
        </section>)}
        {panel('ai', <section className="settings-section" aria-labelledby="settings-ai-title"><AiToolSettings /><h3 id="settings-ai-title">{t('给 AI 的平台说明')}</h3><p>{t('新项目开工前，或判断已有项目能否接入时，把平台说明交给 AI。')}</p><PlatformAiIntroButton platformVersion={info?.version} /></section>)}
        {panel('about', <section className="settings-section radio-settings-about" aria-labelledby="settings-about-title">
          <h3 id="settings-about-title">{t("关于与更新")}</h3>
          <div className="radio-about-brand"><img src="/panda-app-drawer.png" alt="" /><div>{t("熊猫小抽屉")}<span>{info?.version ?? t("尚未读取")} {t(" · 开发版")}</span></div></div>
          <p>{t("项目数据保存在本机；联网功能由具体应用决定。")}</p>
          <AboutPlatform info={info} />
        </section>)}
        {panel('uninstall', <section className="settings-section" aria-labelledby="settings-uninstall-title">
          <h3 id="settings-uninstall-title">{t("卸载前清除数据")}</h3>
          <p className="settings-lead">{t("通常在卸载熊猫小抽屉之前使用。只想释放空间请用“存储与清理”。")}</p>
          <LocalDataCleanupPanel managedProjects={[...(props.projects ?? []).map(project => ({ name: project.name, path: project.projectRoot })), ...(props.nodeApps ?? []).filter(app => app.ownership === 'managed').map(app => ({ name: app.name, path: app.objectLayout?.source.path ?? app.sourcePath }))]} externalProjectCount={(props.nodeApps ?? []).filter(app => app.ownership !== 'managed').length} instanceId={info?.instanceId} scopeIdentity={JSON.stringify([info?.dataRoot, info?.workspaceRoot])} disabled={props.localDataUnavailable} onPendingChange={props.onLocalDataPending} onTerminalLock={props.onLocalDataLock} />
        </section>)}
      </div>
    </div>
    {!props.localDataLocked && <div className="radio-settings-footer"><span>{t("熊猫小抽屉")}</span><button className="refresh-button" disabled={settingsBlocked} onClick={props.onClose}>{t("完成")}</button></div>}
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
    <div className="section-heading"><div><h2 id="build-environment-title">{t("Node 构建环境")}</h2><p>{t("构建使用配套 Node/npm，可明确选择兼容的本机环境。应用运行 Node 不受这里的构建选择影响。")}</p></div><button className="refresh-button" onClick={() => void probe()} disabled={!enabled || loading}><RefreshCw size={14} />{loading ? t("检测中…") : t("重新检测")}</button></div>
    <div className="environment-status"><span className={`project-state state-${environment?.ready ? 'ready' : 'unbuilt'}`}><span />{environment ? environment.ready ? t("可以构建") : t("构建环境需要配置") : loading ? t("正在检测") : t("尚未检测")}</span><span>{t("最低 22.12 · 支持 22 / 24 · 推荐 24 LTS")}</span></div>
    <p>{t("当前来源：")}{environment ? ({ managed: t("平台受管 Node/npm"), explicit: t("明确选择的本机 Node/npm"), detected: t("本机自动探测") })[environment.selection] : t("尚未读取")}{t("。")}</p>
    <dl className="path-list">
      <PathRow icon={<Terminal size={16} />} label={`Node ${environment?.node?.version ?? ''}`} value={environment ? environment.node?.path ?? t("未找到可用 Node") : undefined} />
      <PathRow icon={<Terminal size={16} />} label={`npm ${environment?.npm?.version ?? ''}`} value={environment ? environment.npm?.path ?? t("未找到配套 npm") : undefined} />
      {environment?.managedNodePath && <PathRow icon={<Terminal size={16} />} label={t("默认受管 Node 位置")} value={environment.managedNodePath} />}
    </dl>
    {environment?.managedNodePath && <p>{t("默认受管位置只说明工具所在位置；是否可用以本次检测结果为准。")}</p>}
    {environment?.issues.map((issue) => <p key={issue.code} className="environment-issue">{t(issue.message)} ({issue.code})</p>)}
    {environment && !environment.ready && (environment.selection === 'managed'
      ? <p className="environment-issue">{t("平台配套 Node/npm 暂不可用。请修复或重新安装平台，或明确选择兼容的本机 Node/npm。")}</p>
      : <p className="environment-issue">{t("如需安装兼容的本机 Node/npm，可参考：")}<code>{environment.downloadUrl}</code></p>)}
    {environment?.selection === 'explicit' && !environment.ready && <p className="environment-issue">{t("已保留你明确选择的路径，失效时不会自动改用其他工具。请修正路径，或点击“恢复默认工具”。")}</p>}
    <form className="node-path-form" onSubmit={(event) => { event.preventDefault(); const path = nodePath.trim(); if (frontendPlatform.isAbsolutePath(path)) void configure(path); }}>
      <label htmlFor="node-path">{t("指定本机 Node 可执行文件的绝对路径")}<span className="optional-label">{t("可选")}</span></label>
      <div className="node-path-controls"><input id="node-path" value={nodePath} onChange={(event) => setNodePath(event.target.value)} placeholder={t(frontendPlatform.nodePathPlaceholder)} disabled={loading} autoComplete="off" /><button className="primary-button" type="submit" disabled={!enabled || loading || !frontendPlatform.isAbsolutePath(nodePath.trim())}>{t("保存路径")}</button><button className="refresh-button" type="button" disabled={!enabled || loading} onClick={() => void configure(null)}>{t("恢复默认工具")}</button></div>
      <p>{environment ? environment.managedNodePath ? t("恢复默认工具会清除显式路径，使用平台配套 Node/npm。") : t("此环境没有提供受管工具位置；恢复默认工具会清除显式路径并自动探测本机环境。") : t("读取环境后可查看默认工具来源。")} {t(" 路径只保存在此开发版的本机设置中。")}</p>
    </form>
    {error && <p role="alert" className="environment-issue">{t(error)}</p>}
  </section>;
}

function StatusCard({ icon, label, value, detail, good }: { icon: ReactNode; label: string; value: string; detail: string; good: boolean }) {
  return <div className="status-card"><div className="status-card-label">{icon}<span>{t(label)}</span>{good && <Check size={14} className="status-check" aria-label={t("已验证")} />}</div><strong>{value}</strong><p>{detail}</p></div>;
}
function PathRow({ icon, label, value }: { icon: ReactNode; label: string; value: string | undefined }) {
  return <div className="path-row"><dt>{icon}{label}</dt><dd><code>{value ?? t("尚未读取")}</code></dd></div>;
}
