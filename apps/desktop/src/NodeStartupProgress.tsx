import type { NodeStartupProgress as Progress } from './node-app-types';
import { t } from './i18n';

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
    <p>{t(progress ? phases[progress.phase] ?? '正在启动应用…' : '正在启动应用…')}</p>
    {progress && <p>{t('已等待 {seconds} 秒', { seconds: Math.floor(progress.elapsedMs / 1000) })}{progress.phase !== 'cleanup' && <>{t(' · 启动总时限 {seconds} 秒', { seconds: Math.floor(progress.timeoutMs / 1000) })}</>}</p>}
    {progress?.phase === 'checking-health' && <p>{t('连接检查最多 {seconds} 秒，包含在启动总时限内。', { seconds: Math.floor(progress.healthTimeoutMs / 1000) })}</p>}
  </div>;
}
