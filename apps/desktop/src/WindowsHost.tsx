import { invoke, isTauri } from '@tauri-apps/api/core';
import { CircleAlert, FolderLock, Monitor, RefreshCw, ShieldCheck } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';

interface HostReport {
  schemaVersion: 1;
  identifier: string;
  environment: string;
  platform: 'windows';
  architecture: string;
  stage: 'readOnly';
  runtimeAvailable: false;
  probe: {
    dataRoot: string | null;
    status: 'missing' | 'retained' | 'blocked' | 'unavailable';
    checks: { code: string; status: 'passed' | 'blocked' | 'unavailable'; message: string }[];
    evidence: { kind: string; count: number }[];
  };
}

const probeLabels: Record<HostReport['probe']['status'], string> = {
  missing: '未发现数据根',
  retained: '发现保留数据',
  blocked: '检查受阻',
  unavailable: '暂无法检查',
};
const checkLabels = { passed: '通过', blocked: '受阻', unavailable: '未完成' };
const evidenceLabels: Record<string, string> = {
  environmentMarker: '环境标记文件', settings: '工作区设置文件',
  migrationEntries: '迁移目录内条目', cleanupEntries: '清理目录内条目',
  workspaceEntries: '默认工作区顶层条目', runtimeDirectory: '运行时目录（仅观察存在）',
  unknownTopLevelEntries: '其他保留条目',
};

function errorMessage(cause: unknown): string {
  if (typeof cause === 'object' && cause && 'message' in cause && typeof cause.message === 'string') return cause.message;
  return typeof cause === 'string' ? cause : '只读检查未完成，请重试。';
}

export function WindowsHost() {
  const [report, setReport] = useState<HostReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [checkedAt, setCheckedAt] = useState<string | null>(null);
  const request = useRef(0);
  const refresh = useCallback(async () => {
    const current = ++request.current;
    setLoading(true);
    setError(null);
    try {
      if (!isTauri()) throw new Error('请从 Windows 桌面应用打开此页面，以读取本机检查结果。');
      const value = await invoke<HostReport>('host_status');
      if (current !== request.current) return;
      setReport(value);
      setCheckedAt(new Date().toLocaleTimeString('zh-CN', { hour12: false }));
    } catch (cause) {
      if (current === request.current) setError(errorMessage(cause));
    } finally {
      if (current === request.current) setLoading(false);
    }
  }, []);
  useEffect(() => {
    void refresh();
    return () => { request.current += 1; };
  }, [refresh]);

  return <div className="windows-host">
    <header className="host-header">
      <div className="host-brand"><span className="host-logo"><Monitor size={21} aria-hidden="true" /></span><strong>本地项目</strong><span className="host-edition">Windows 只读内部版</span></div>
      <span className="host-stage">W1 · 宿主验证</span>
    </header>
    <main className="host-main">
      <section className="host-intro" aria-labelledby="host-title">
        <div><p className="host-eyebrow">WINDOWS COMPATIBILITY</p><h1 id="host-title">桌面窗口已启动</h1><p>查看本机目录与保留记录的只读检查结果。项目运行时尚未开放。</p></div>
        <button className="host-refresh" onClick={() => void refresh()} disabled={loading}><RefreshCw size={16} aria-hidden="true" />{loading ? '正在检查…' : '刷新检查'}</button>
      </section>
      <div className="host-protection"><ShieldCheck size={22} aria-hidden="true" /><div><strong>现有项目保持保护</strong><p>本窗口不创建工作区、不启动项目服务，也不执行恢复、迁移或清理。目录检查通过仍不代表业务功能已开放。</p></div></div>
      {error && <div className="host-error" role="alert"><CircleAlert size={18} aria-hidden="true" /><span>{error}{report ? ' 下方保留上次成功检查结果。' : ''}</span></div>}
      <div className="host-columns">
        <section className="host-card" aria-labelledby="host-root-title" aria-busy={loading}>
          <div className="host-card-heading"><h2 id="host-root-title"><FolderLock size={19} aria-hidden="true" />本机目录检查</h2>{report && <span className={`host-pill ${report.probe.status}`}>{probeLabels[report.probe.status]}</span>}</div>
          <p className="host-caption">系统目录 API 解析的固定开发数据位置</p>
          <div className="host-path">{report ? (report.probe.dataRoot ?? '未能解析数据目录') : (loading ? '正在读取本机状态…' : '尚未取得检查结果')}</div>
          {report && <ul className="host-checks">{report.probe.checks.map((check, index) => <li key={`${check.code}-${index}`}><span className={`host-check-mark ${check.status}`} aria-hidden="true">{check.status === 'passed' ? '✓' : '!'}</span><div><p>{check.message}</p><span className={`host-check-label ${check.status}`}>{checkLabels[check.status]}</span></div></li>)}</ul>}
          <div className="host-check-time" role="status">{loading ? '检查进行中' : checkedAt ? `最近成功检查 ${checkedAt}` : '尚无成功检查记录'}</div>
        </section>
        <aside className="host-card host-capabilities" aria-labelledby="host-capabilities-title">
          <div className="host-card-heading"><h2 id="host-capabilities-title">当前可用范围</h2></div>
          <ul><li><div><strong>桌面窗口与状态刷新</strong><p>查看本次真实检查结果</p></div><span className="host-capability available">已开放</span></li><li><div><strong>项目运行与数据读写</strong><p>创建、打开、构建和数据库操作</p></div><span className="host-capability">未开放</span></li><li><div><strong>维护与历史操作</strong><p>备份、恢复、回收、迁移和清理</p></div><span className="host-capability">未开放</span></li></ul>
          <p className="host-next-step">后续完成相应文件安全与业务验证后，再逐项开放功能。</p>
        </aside>
      </div>
      <section className="host-card host-evidence" aria-labelledby="host-evidence-title">
        <div className="host-card-heading"><h2 id="host-evidence-title">保留现场</h2><span className="host-caption">只读观察，不执行处理</span></div>
        {!report ? <p className="host-empty">{loading ? '等待本机检查结果…' : '尚未取得保留记录信息。'}</p> : report.probe.evidence.length === 0 ? <p className="host-empty">本次未列出现场记录。是否完成检查请以上方目录检查结果为准。</p> : <div className="host-evidence-grid">{report.probe.evidence.map((entry) => <div key={entry.kind}><span>{evidenceLabels[entry.kind] ?? entry.kind}</span><strong>{entry.count}</strong></div>)}</div>}
        <p className="host-caption">条目数量不代表项目数量、操作完成或当前可恢复状态。</p>
      </section>
      <footer className="host-footer"><span>开发环境 · 项目运行时未启动</span>{report && <details><summary>诊断信息</summary><dl><dt>应用身份</dt><dd>{report.identifier}</dd><dt>平台与架构</dt><dd>{report.platform} / {report.architecture}</dd><dt>环境 / 阶段</dt><dd>{report.environment} / {report.stage}</dd></dl></details>}</footer>
    </main>
  </div>;
}
