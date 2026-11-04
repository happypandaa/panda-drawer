import { t } from './i18n';
import type { DownloadRoute } from './download-routes';

export function DownloadRouteFailure({ route, disabled = false, onRetry }: {
  route: DownloadRoute; disabled?: boolean; onRetry: () => void;
}) {
  return <div className="workspace-inline-notice" role="alert">
    <p>{route === 'cn'
      ? t('用国内线路下载失败，可以换成国外线路试试。')
      : t('用国外线路下载失败，可以换成国内线路试试。')}</p>
    <button className="refresh-button" disabled={disabled} onClick={onRetry}>
      {route === 'cn' ? t('换成国外线路并重试') : t('换成国内线路并重试')}
    </button>
  </div>;
}
