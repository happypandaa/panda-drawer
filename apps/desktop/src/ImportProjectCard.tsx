import { X } from 'lucide-react';
import { t, useLocale } from './i18n';
import { localizedProject } from './app-localization';
import { launcherMonogram } from './ProjectCard';
import { canDismissGitImport, gitImportLabel, gitImportNeedsAttention } from './git-import-view-state';
import type { GitImportViewState } from './git-import-view-state';

export function ImportProjectCard({ state, onView, onRemove }: {
  state: GitImportViewState;
  onView: () => void;
  onRemove: () => void;
}) {
  const { locale } = useLocale();
  const name = (state.snapshot?.app && localizedProject(state.snapshot.app, locale).name) || state.name || t("正在添加的应用");
  const label = gitImportLabel(state);
  const attention = gitImportNeedsAttention(state);
  const failed = state.view === 'failed';
  const tone = failed ? 'error' : attention ? 'attention' : 'busy';
  return <article className="project-card launcher-tile import-project-card" aria-label={t("正在添加「{p0}」", { p0: name })}>
    <button type="button" className="primary-button launcher-open" onClick={onView} aria-label={attention ? t("查看「{p0}」需要处理的事", { p0: name }) : t("查看「{p0}」的添加进度", { p0: name })}>
      <span className={`project-icon launcher-icon launcher-color-0${tone === 'busy' ? ' is-busy' : ''}`} aria-hidden="true"><span className="launcher-monogram">{launcherMonogram(name)}</span>{tone !== 'busy' && <span className={`launcher-badge is-${tone}`} />}</span>
      <span className="launcher-name" title={name}>{name}</span>
      <span className={`launcher-state is-${tone}`} role="status">{t(label)}</span>
    </button>
    {/* Clicking the tile already opens the progress, confirmation or reason, so the tile keeps one height. */}
    {canDismissGitImport(state) && <button type="button" className="import-remove" title={t("移除")} onClick={onRemove}><X size={12} aria-hidden="true" /><span className="sr-only">{t("移除")}</span></button>}
  </article>;
}
