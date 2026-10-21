export interface ProjectGitVersionsResult {
  repositoryRoot: string;
  head: { oid: string | null; branch: string | null };
  versions: { oid: string; parents: string[]; subject: string; author: { name: string; email: string }; authoredAt: string }[];
  hasMore: boolean;
  limit: number;
}

export function ProjectGitVersions({ value, fresh, reading, error, canRead, onRead }: { value: ProjectGitVersionsResult | null; fresh: boolean; reading: boolean; error: string; canRead: boolean; onRead: () => void }) {
  return <section aria-label="本机源码版本列表">
    <div className="storage-heading"><h4>本机源码版本</h4><button type="button" className="refresh-button" disabled={!canRead || reading} onClick={onRead}>{reading ? '读取版本中…' : value ? '刷新本机版本' : '查看本机版本'}</button></div>
    <p>只读显示当前版本可达的本机提交；不会创建分支、取回源码或访问远端。</p>
    {error && <p className="task-error" role="alert">{error}</p>}
    {value && <>
      {!fresh && <p className="maintenance-warning">下面是上次读取的版本列表，请刷新后核对。</p>}
      <p>读取时的分支：<code>{value.head.branch?.replace(/^refs\/heads\//, '') ?? '未附着分支'}</code></p>
      {value.head.oid && <p>读取时的当前版本：<code>{value.head.oid}</code></p>}
      {!value.versions.length && <p>当前版本没有可列出的提交。</p>}
      <ol>{value.versions.map(version => <li key={version.oid}><p>{version.subject || '无版本说明'}</p><code>{version.oid}</code><p>{version.author.name} · {version.author.email} · <time dateTime={version.authoredAt}>{version.authoredAt}</time></p></li>)}</ol>
      {value.hasMore && <p>仅显示最近 {value.limit} 个可达版本，还有更早版本未列出。本步暂不提供翻页。</p>}
    </>}
  </section>;
}
