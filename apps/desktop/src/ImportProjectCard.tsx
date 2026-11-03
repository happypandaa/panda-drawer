import { t } from './i18n';
import { launcherMonogram } from './ProjectCard';
import { canDismissGitImport, gitImportLabel, gitImportNeedsAttention } from './git-import-view-state';
import type { GitImportViewState } from './git-import-view-state';

export function ImportProjectCard({ state, onView, onRemove }: {
  state: GitImportViewState;
  onView: () => void;
  onRemove: () => void;
}) {
  const name = state.snapshot?.app?.name || state.name || t("正在添加的应用");
  const label = gitImportLabel(state);
  const attention = gitImportNeedsAttention(state);
  return <article className="project-card launcher-tile import-project-card" aria-label={t("正在添加「{p0}」", { p0: name })}>
    <button type="button" className="primary-button launcher-open" onClick={onView} aria-label={t("查看「{p0}」的添加进度", { p0: name })}>
      <span className="project-icon launcher-icon launcher-color-0" aria-hidden="true"><span className="launcher-monogram">{launcherMonogram(name)}</span></span>
      <span className="launcher-name" title={name}>{name}</span>
      <span className={`launcher-state is-${attention ? 'attention' : 'busy'}`} role="status"><span className="launcher-state-dot" />{t(label)}</span>
    </button>
    {attention && <button type="button" className="text-button" onClick={onView}>{state.view === 'waiting' || state.task?.status === 'waitingConfirmation' ? t("查看确认") : t("查看原因")}</button>}
    {canDismissGitImport(state) && <button type="button" className="text-button" onClick={onRemove}>{t("移除")}</button>}
  </article>;
}
