import type { NodeStartupProgress as Progress } from './node-app-types';

const phases: Record<Progress['phase'], string> = {
  preparing: '正在准备运行环境…',
  initializing: '正在初始化应用…',
  'waiting-ready': '正在等待应用准备完成…',
  'checking-health': '正在检查本机连接…',
  'preparing-page': '正在准备应用页面…',
  cleanup: '启动未完成，正在关闭本次启动资源…',
};

export function NodeStartupProgress({ progress }: { progress: Progress | null }) {
  return <div className="project-help node-startup-progress" role="status" aria-live="polite" aria-atomic="true">
    <p>{progress ? phases[progress.phase] ?? '正在启动应用…' : '正在启动应用…'}</p>
    {progress && <p>已等待 {Math.floor(progress.elapsedMs / 1000)} 秒{progress.phase !== 'cleanup' && <> · 启动总时限 {Math.floor(progress.timeoutMs / 1000)} 秒</>}</p>}
    {progress?.phase === 'checking-health' && <p>连接检查最多 {Math.floor(progress.healthTimeoutMs / 1000)} 秒，包含在启动总时限内。</p>}
  </div>;
}
