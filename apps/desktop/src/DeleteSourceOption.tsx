import { t } from './i18n';

export function DeleteSourceOption({ sourcePath, preserveSource, disabled, onChange, onReveal }: {
  sourcePath: string;
  preserveSource: boolean;
  disabled: boolean;
  onChange: (preserveSource: boolean) => void;
  onReveal?: () => void;
}) {
  return <div className="delete-source-option">
    <label className="checkbox-label">
      <input type="checkbox" checked={!preserveSource} disabled={disabled} onChange={event => onChange(!event.target.checked)} />
      {t('同时删除电脑上的应用文件夹')}
    </label>
    <div className="delete-source-path">
      <code>{sourcePath}</code>
      {onReveal && <button type="button" className="text-button" disabled={disabled} onClick={onReveal}>{t('打开文件夹')}</button>}
    </div>
    <p className="project-help">{preserveSource
      ? t('代码和 data/ 会留在这个路径。以后可用「本机文件夹」重新添加，届时文件夹归你。')
      : t('删除范围包括你和 AI 改过的代码，以及 data/ 里的数据。')}</p>
  </div>;
}
