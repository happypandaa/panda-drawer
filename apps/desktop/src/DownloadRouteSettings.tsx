import { RefreshCw } from 'lucide-react';
import { t } from './i18n';
import { downloadRoutes, useDownloadRoutes } from './download-routes';
import type { DownloadMode } from './download-routes';

export function DownloadRouteSettings({ disabled = false }: { disabled?: boolean }) {
  const state = useDownloadRoutes();
  return <>
    <div className="settings-row">
      <div><strong>{t('下载线路')}</strong><p>{state.mode === 'auto'
        ? state.detectedRoute === 'cn' ? t('自动 · 当前使用国内线路') : state.detectedRoute === 'global' ? t('自动 · 当前使用国外线路') : t('自动 · 还没检测')
        : t('用于推荐应用下载。')}</p></div>
      <select aria-label={t('下载线路')} value={state.mode} disabled={disabled} onChange={event => downloadRoutes.setMode(event.target.value as DownloadMode)}>
        <option value="auto">{t('自动（推荐）')}</option>
        <option value="cn">{t('国内线路（Gitee）')}</option>
        <option value="global">{t('国外线路（GitHub）')}</option>
      </select>
      {state.mode === 'auto' && <button className="refresh-button" disabled={disabled || state.detecting} onClick={() => void downloadRoutes.detect()}>
        <RefreshCw size={14} />{state.detecting ? t('检测中…') : t('重新检测')}
      </button>}
    </div>
    {state.error && <p role="alert" className="environment-issue">{t(state.error)}</p>}
  </>;
}
