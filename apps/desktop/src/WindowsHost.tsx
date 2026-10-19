import { invoke, isTauri } from '@tauri-apps/api/core';
import { CircleAlert, FolderLock, Globe, Monitor, Plus, RefreshCw, ShieldCheck, X } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import type { ProjectInfo } from './types';

interface ProjectStatus {
  stage: 'createReadOnly';
  rootStatus: 'missing' | 'ready' | 'blocked';
  message: string;
  dataRoot: string;
  projects: ProjectInfo[];
}

interface HostReport {
  identifier: string;
  platform: string;
  architecture: string;
  probe: {
    checks: { code: string; status: string; message: string }[];
    evidence: { kind: string; count: number }[];
  };
}

const rootLabels: Record<ProjectStatus['rootStatus'], string> = {
  missing: '等待首次创建',
  ready: '工作区已就绪',
  blocked: '现有目录受保护',
};
const evidenceLabels: Record<string, string> = {
  environmentMarker: '环境标记文件', settings: '工作区设置文件',
  migrationEntries: '迁移目录内条目', cleanupEntries: '清理目录内条目',
  workspaceEntries: '默认工作区顶层条目', runtimeDirectory: '运行时目录（仅观察存在）',
  unknownTopLevelEntries: '其他保留条目',
};

function errorMessage(cause: unknown): string {
  if (typeof cause === 'object' && cause && 'message' in cause && typeof cause.message === 'string') return cause.message;
  return typeof cause === 'string' ? cause : '操作未完成，请刷新状态后查看结果。';
}

export function WindowsHost() {
  const [status, setStatus] = useState<ProjectStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [checkedAt, setCheckedAt] = useState<string | null>(null);
  const [statusStale, setStatusStale] = useState(false);
  const [showCreate, setShowCreate] = useState(false);
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [creating, setCreating] = useState(false);
  const [opening, setOpening] = useState<string | null>(null);
  const [diagnostic, setDiagnostic] = useState<HostReport | null>(null);
  const [diagnosticLoading, setDiagnosticLoading] = useState(false);
  const request = useRef(0);
  const createPending = useRef(false);
  const openPending = useRef(false);
  const mounted = useRef(true);
  const refresh = useCallback(async () => {
    const current = ++request.current;
    setLoading(true);
    setError(null);
    try {
      if (!isTauri()) throw new Error('请从 Windows 桌面应用打开此页面，以读取本机项目状态。');
      const value = await invoke<ProjectStatus>('project_status');
      if (current !== request.current) return;
      setStatus(value);
      setStatusStale(false);
      if (value.rootStatus === 'ready') setDiagnostic(null);
      setCheckedAt(new Date().toLocaleTimeString('zh-CN', { hour12: false }));
    } catch (cause) {
      if (current === request.current) { setError(errorMessage(cause)); setStatusStale(true); }
    } finally {
      if (current === request.current) setLoading(false);
    }
  }, []);
  useEffect(() => {
    mounted.current = true;
    void refresh();
    return () => { mounted.current = false; request.current += 1; };
  }, [refresh]);

  async function createProject(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (createPending.current || !name.trim() || status?.rootStatus === 'blocked') return;
    createPending.current = true;
    setCreating(true);
    setError(null);
    setNotice(null);
    try {
      const project = await invoke<ProjectInfo>('project_create', {
        input: { name: name.trim(), description: description.trim() || null },
      });
      if (!mounted.current) return;
      setName(''); setDescription(''); setShowCreate(false);
      setNotice('已创建「' + project.name + '」。可以在桌面或浏览器打开初始应用。');
      await refresh();
    } catch (cause) {
      if (mounted.current) {
        // Never retry a creation after an ambiguous response. A manual refresh
        // lets the user inspect actual projects before making another request.
        setError(errorMessage(cause));
        setStatusStale(true);
      }
    } finally {
      createPending.current = false;
      if (mounted.current) setCreating(false);
    }
  }

  async function openProject(project: ProjectInfo, target: 'desktop' | 'browser') {
    if (openPending.current) return;
    openPending.current = true;
    setOpening(project.id + ':' + target);
    setError(null);
    setNotice(null);
    try {
      await invoke(target === 'desktop' ? 'project_open' : 'project_open_browser', { projectId: project.id });
      if (mounted.current) setNotice(target === 'desktop' ? '已打开「' + project.name + '」项目窗口。' : '已请求默认浏览器打开「' + project.name + '」。');
    } catch (cause) {
      if (mounted.current) {
        setError(errorMessage(cause));
        setStatusStale(true);
      }
    } finally {
      openPending.current = false;
      if (mounted.current) setOpening(null);
    }
  }

  async function readDiagnostic() {
    if (diagnosticLoading || status?.rootStatus === 'ready') return;
    setDiagnosticLoading(true);
    setError(null);
    try {
      const value = await invoke<HostReport>('host_status');
      if (mounted.current) setDiagnostic(value);
    } catch (cause) {
      if (mounted.current) setError(errorMessage(cause));
    } finally {
      if (mounted.current) setDiagnosticLoading(false);
    }
  }

  const blocked = !status || status.rootStatus === 'blocked' || statusStale;
  const busy = loading || creating || Boolean(opening) || diagnosticLoading;
  return <div className="windows-host">
    <header className="host-header">
      <div className="host-brand"><span className="host-logo"><Monitor size={21} aria-hidden="true" /></span><strong>本地项目</strong><span className="host-edition">Windows 项目内部版</span></div>
      <span className="host-stage">W2.1 · 创建与只读打开</span>
    </header>
    <main className="host-main">
      <section className="host-intro" aria-labelledby="host-title">
        <div><p className="host-eyebrow">WINDOWS COMPATIBILITY</p><h1 id="host-title">新建项目，打开初始应用</h1><p>当前仅用于可丢弃样本。创建和打开预编译初始页面无需 Node 或联网。</p></div>
        <div className="host-toolbar"><button className="host-refresh" onClick={() => void refresh()} disabled={busy}><RefreshCw size={16} aria-hidden="true" />{loading ? '正在读取…' : '刷新状态'}</button><button className="host-primary" onClick={() => setShowCreate(true)} disabled={blocked || busy || showCreate}><Plus size={16} aria-hidden="true" />新建验证样本</button></div>
      </section>
      <div className="host-protection"><ShieldCheck size={22} aria-hidden="true" /><div><strong>分阶段开放，现有目录继续保护</strong><p>本阶段只创建样本并读取初始页面和真实数据库结构。SQL 写入、附件、构建、备份、恢复、回收、迁移及清理尚未开放。未知或未解决的旧现场保留并阻断接管。</p></div></div>
      {error && <div className="host-error" role="alert"><CircleAlert size={18} aria-hidden="true" /><span>{error}{status ? ' 下方保留上次成功读取的状态；请刷新确认后再操作。' : ''}</span></div>}
      {notice && <div className="host-notice" role="status">{notice}</div>}
      <section className="host-card host-workspace" aria-labelledby="host-root-title" aria-busy={loading}>
        <div className="host-card-heading"><h2 id="host-root-title"><FolderLock size={19} aria-hidden="true" />固定开发工作区</h2>{status && <span className={'host-pill ' + status.rootStatus}>{rootLabels[status.rootStatus]}</span>}</div>
        <p className="host-caption">系统目录 API 解析的数据位置；业务项目位于其 workspace/projects 下。</p>
        <div className="host-path">{status ? status.dataRoot : loading ? '正在读取本机状态…' : '尚未取得项目状态'}</div>
        {status && <p className="host-root-message">{status.message}</p>}
        <div className="host-check-time" role="status">{loading ? '正在读取项目状态' : checkedAt ? '最近成功读取 ' + checkedAt : '尚无成功读取记录'}</div>
      </section>
      {showCreate && <section className="host-card host-create" aria-labelledby="host-create-title">
        <div className="host-card-heading"><h2 id="host-create-title">新建验证样本</h2><button type="button" className="host-icon-button" aria-label="关闭新建表单" disabled={creating} onClick={() => setShowCreate(false)}><X size={18} aria-hidden="true" /></button></div>
        <form onSubmit={event => void createProject(event)}>
          <label htmlFor="host-project-name">项目名称</label><input id="host-project-name" value={name} onChange={event => setName(event.target.value)} maxLength={200} required disabled={creating} autoFocus placeholder="例如：Windows 可丢弃验证项目" />
          <label htmlFor="host-project-description">用途说明（可选）</label><textarea id="host-project-description" value={description} onChange={event => setDescription(event.target.value)} maxLength={2000} disabled={creating} rows={3} placeholder="填写本次要验证的内容，使用可丢弃数据。" />
          <div className="host-create-footer"><p>复制固定 SDK、匹配源码和初始网页，并建立独立空库。不会安装依赖或运行项目脚本。</p><button className="host-primary" type="submit" disabled={creating || !name.trim() || blocked}>{creating ? '正在创建…' : '创建样本'}</button></div>
        </form>
      </section>}
      <section className="host-card host-projects" aria-labelledby="host-projects-title">
        <div className="host-card-heading"><h2 id="host-projects-title">本轮项目</h2>{status && <span className="host-caption">{status.projects.length} 个项目</span>}</div>
        {!status ? <p className="host-empty">等待真实项目状态…</p> : status.rootStatus === 'blocked' ? <p className="host-empty">当前目录未获准接管。请保留现有文件，并查看上方原因；本窗口不会创建空工作区覆盖它。</p> : status.projects.length === 0 ? <p className="host-empty">尚无项目。点击“新建验证样本”后，即可打开随包提供的初始应用。</p> : <ul className="host-project-list">{status.projects.map(project => <li key={project.id}>
          <div className="host-project-summary"><div className="host-project-title"><h3>{project.name}</h3><span className={'host-pill ' + (project.state === 'ready' ? 'ready' : 'blocked')}>{project.state === 'ready' ? '可只读打开' : '需要检查'}</span></div>{project.description && <p>{project.description}</p>}<span className="host-project-id">{project.id}</span>{project.issues.length > 0 && <p className="host-project-issues">{project.issues.join('；')}</p>}</div>
          <div className="host-project-actions"><button className="host-refresh" disabled={busy || blocked || project.state !== 'ready'} onClick={() => void openProject(project, 'desktop')}><Monitor size={15} aria-hidden="true" />{opening === project.id + ':desktop' ? '正在打开…' : '桌面打开'}</button><button className="host-refresh" disabled={busy || blocked || project.state !== 'ready'} onClick={() => void openProject(project, 'browser')}><Globe size={15} aria-hidden="true" />{opening === project.id + ':browser' ? '正在打开…' : '浏览器打开'}</button></div>
        </li>)}</ul>}
      </section>
      <div className="host-columns">
        <aside className="host-card host-capabilities" aria-labelledby="host-capabilities-title">
          <div className="host-card-heading"><h2 id="host-capabilities-title">当前可用范围</h2></div>
          <ul><li><div><strong>新样本与初始应用</strong><p>创建、桌面打开、普通浏览器打开</p></div><span className="host-capability available">已开放</span></li><li><div><strong>数据库结构只读查询</strong><p>初始页面读取真实 schema；创建时为空库</p></div><span className="host-capability available">已开放</span></li><li><div><strong>业务写入与管理</strong><p>SQL、附件、构建、备份、恢复及清理</p></div><span className="host-capability">未开放</span></li></ul>
        </aside>
        <section className="host-card host-evidence" aria-labelledby="host-diagnostics-title">
          <div className="host-card-heading"><h2 id="host-diagnostics-title">目录诊断</h2></div>
          {status?.rootStatus === 'ready' ? <p className="host-caption">工作区由当前项目服务持有，以上方项目状态为准。独立浅层诊断不会再次打开数据根。</p> : <><p className="host-caption">按需只读观察保留现场，不执行恢复或清理。</p><button className="host-refresh" disabled={diagnosticLoading || busy} onClick={() => void readDiagnostic()}>{diagnosticLoading ? '正在检查…' : '读取目录诊断'}</button></>}
          {diagnostic && <div className="host-diagnostic-result"><ul className="host-checks">{diagnostic.probe.checks.map((check, index) => <li key={check.code + '-' + index}><span className={'host-check-mark ' + check.status} aria-hidden="true">{check.status === 'passed' ? '✓' : '!'}</span><p>{check.message}</p></li>)}</ul>{diagnostic.probe.evidence.map(entry => <p className="host-caption" key={entry.kind}>{evidenceLabels[entry.kind] ?? entry.kind}：{entry.count}</p>)}</div>}
        </section>
      </div>
      <footer className="host-footer"><span>固定开发环境 · Windows W2.1 内部验证 · 仅使用可丢弃项目</span></footer>
    </main>
  </div>;
}
