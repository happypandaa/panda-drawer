import { t } from './i18n';
export function NodeDependencyWarnings({ warnings }: { warnings?: string[] }) {
  if (!warnings?.length) return null;
  return <div className="node-config-warning" role="note" aria-label={t("运行组件提示")}>
    <strong>{t("运行组件需要准备")}</strong>
    {warnings.map((warning, index) => <p key={index}>{t(warning)}</p>)}
  </div>;
}
