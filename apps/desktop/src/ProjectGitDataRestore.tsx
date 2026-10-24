import { bytesText } from './storage-utils';
import { messageFrom } from './ui-utils';
import { useEffect, useState } from 'react';

interface DataSummary {
  databaseBytes: number;
  attachmentCount: number;
  attachmentBytes: number;
  directoryCount: number;
}

export interface DataRestorePreview {
  repositoryRoot: string;
  head: { oid: string; branch: string };
  targetCommit: string;
  documentEpoch: string;
  snapshotDigest: string;
  liveDataDigest: string;
  targetSnapshotDigest: string;
  planDigest: string;
  current: DataSummary;
  target: DataSummary;
  attachmentChanges: { path: string; kind: 'add' | 'modify' | 'delete' }[];
  directoryChanges: { path: string; kind: 'add' | 'delete' }[];
  databaseChanged: boolean;
  canApply: boolean;
  applyImplemented: boolean;
  reason: string;
  issues: unknown[];
}

export function ProjectGitDataRestore({ targetCommit, preview, error, disabled, reading, onPreview }: {
  targetCommit: string;
  preview: DataRestorePreview | null;
  error: string;
  disabled: boolean;
  reading: boolean;
  onPreview: () => void;
}) {
  const [visibleChanges, setVisibleChanges] = useState(200);
  const [visibleDirectories, setVisibleDirectories] = useState(200);
  useEffect(() => { setVisibleChanges(200); setVisibleDirectories(200); }, [preview?.planDigest]);
  return <section aria-label="预览业务数据版本" className="operation-confirmation">
    <h4>预览已保存的数据</h4>
    <p>使用上面选择的完整版本号，核对该版本的数据库和附件。预览会短暂停止业务读写以读取当前一致性数据，不替换当前数据、源码或应用。</p>
    {targetCommit && <p>所选版本：<code>{targetCommit}</code></p>}
    <button type="button" className="refresh-button" disabled={disabled || !/^[0-9a-f]{40}$/.test(targetCommit)} onClick={onPreview}>{reading ? '正在核对数据…' : '预览数据版本'}</button>
    {error && <p role="alert" className="task-error">{error}</p>}
    {preview && <>
      <table><thead><tr><th scope="col">数据范围</th><th scope="col">当前业务数据</th><th scope="col">所选版本</th></tr></thead><tbody>
        <tr><th scope="row">数据库</th><td>{bytesText(preview.current.databaseBytes)}</td><td>{bytesText(preview.target.databaseBytes)}</td></tr>
        <tr><th scope="row">附件</th><td>{preview.current.attachmentCount} 个 · {bytesText(preview.current.attachmentBytes)}</td><td>{preview.target.attachmentCount} 个 · {bytesText(preview.target.attachmentBytes)}</td></tr>
        <tr><th scope="row">附件目录</th><td>{preview.current.directoryCount} 个</td><td>{preview.target.directoryCount} 个</td></tr>
      </tbody></table>
      <p>{preview.databaseChanged ? '数据库内容与当前数据不同。' : '数据库内容与当前数据相同。'}</p>
      <details><summary>附件变化（{preview.attachmentChanges.length}）</summary>{preview.attachmentChanges.length ? <ul>{preview.attachmentChanges.slice(0, visibleChanges).map(change => <li key={change.path}><code>{change.path}</code> · {{ add: '新增', modify: '修改', delete: '删除' }[change.kind]}</li>)}</ul> : <p>附件没有变化。</p>}{preview.attachmentChanges.length > visibleChanges && <button type="button" className="text-button" onClick={() => setVisibleChanges(value => value + 200)}>继续显示附件变化（剩余 {preview.attachmentChanges.length - visibleChanges} 项）</button>}</details>
      <details><summary>附件目录变化（{preview.directoryChanges.length}）</summary>{preview.directoryChanges.length ? <ul>{preview.directoryChanges.slice(0, visibleDirectories).map(change => <li key={change.path}><code>{change.path}</code> · {{ add: '新增目录', delete: '删除目录' }[change.kind]}</li>)}</ul> : <p>附件目录没有变化。</p>}{preview.directoryChanges.length > visibleDirectories && <button type="button" className="text-button" onClick={() => setVisibleDirectories(value => value + 200)}>继续显示目录变化（剩余 {preview.directoryChanges.length - visibleDirectories} 项）</button>}</details>
      {preview.issues.map((issue, index) => <p role="alert" key={index}>{messageFrom(issue)}</p>)}
      <p>{preview.reason}</p>
      {!preview.applyImplemented && <p>当前可以预览数据版本，尚不能用它替换业务数据。</p>}
    </>}
  </section>;
}
