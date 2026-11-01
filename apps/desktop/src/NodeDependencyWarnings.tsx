export function NodeDependencyWarnings({ warnings }: { warnings?: string[] }) {
  if (!warnings?.length) return null;
  return <div className="node-config-warning" role="note" aria-label="运行组件提示">
    <strong>运行组件需要准备</strong>
    {warnings.map((warning, index) => <p key={index}>{warning}</p>)}
  </div>;
}
