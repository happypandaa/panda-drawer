import { invoke } from '@tauri-apps/api/core';
import { TriangleAlert, Trash2 } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { Modal } from './Modal';
import type { NodeAppDeletePreview, NodeAppDeleteResult } from './node-app-types';
import { messageFrom } from './ui-utils';

function validPreview(value: NodeAppDeletePreview, appId: string): boolean {
  return Boolean(value && value.appId === appId && typeof value.name === 'string' && value.name
    && ['managed', 'external'].includes(value.ownership) && typeof value.expectedRevision === 'string' && value.expectedRevision
    && typeof value.platformRoot === 'string' && value.platformRoot.startsWith('/') && typeof value.sourcePath === 'string' && value.sourcePath.startsWith('/')
    && Array.isArray(value.deletePaths) && value.deletePaths.length > 0 && value.deletePaths.every(path => typeof path === 'string' && path.startsWith('/'))
    && Array.isArray(value.preservedPaths) && value.preservedPaths.every(path => typeof path === 'string' && path.startsWith('/')));
}

function trimSlash(path: string): string { return path.length > 1 ? path.replace(/\/+$/, '') : path; }
function parentOf(path: string): string { const value = trimSlash(path); const index = value.lastIndexOf('/'); return index <= 0 ? '/' : value.slice(0, index); }
function baseName(path: string): string { const value = trimSlash(path); return value.slice(value.lastIndexOf('/') + 1) || value; }

function commonDir(paths: string[]): string {
  const parts = paths.map(path => parentOf(path).split('/').filter(Boolean));
  if (!parts.length) return '/';
  let length = 0;
  while (parts.every(segments => segments.length > length && segments[length] === parts[0][length])) length++;
  return `/${parts[0].slice(0, length).join('/')}`;
}

/** Lists full paths grouped by parent directory, relative to a base directory shown once above the list. */
function PathGroups({ paths, base }: { paths: string[]; base: string }) {
  const groups = new Map<string, string[]>();
  for (const path of new Set(paths)) { const dir = parentOf(path); groups.set(dir, [...(groups.get(dir) ?? []), path]); }
  const relative = (dir: string) => dir === base ? '' : dir.startsWith(base === '/' ? '/' : `${base}/`) ? `${dir.slice(base === '/' ? 1 : base.length + 1)}/` : `${dir}/`;
  return <ul className="node-delete-groups">{[...groups].map(([dir, items]) => <li key={dir}>
    {relative(dir) && <code className="node-delete-dir" title={dir}>{relative(dir)}</code>}
    <ul className="node-delete-names">{items.map(path => <li key={path} data-path={path} title={path}><code>{baseName(path)}</code></li>)}</ul>
  </li>)}</ul>;
}

export function NodeAppDeleteDialog({ app, enabled, instanceId, onClose, onPendingChange, onDeleted }: {
  app: { id: string; name: string };
  enabled: boolean;
  instanceId?: string;
  onClose: () => void;
  onPendingChange: (pending: boolean) => void;
  onDeleted: (appId: string, result: NodeAppDeleteResult) => void;
}) {
  const [preview, setPreview] = useState<NodeAppDeletePreview | null>(null);
  const [verified, setVerified] = useState(false);
  const [acknowledged, setAcknowledged] = useState(false);
  const [error, setError] = useState('');
  const [pending, setPending] = useState<'preview' | 'delete' | null>(null);
  const alive = useRef(true);
  const operation = useRef<'preview' | 'delete' | null>(null);
  const sequence = useRef(0);
  const approval = useRef<{ appId: string; instanceId?: string; revision: string; sequence: number } | null>(null);
  const context = useRef({ appId: app.id, instanceId, enabled }); context.current = { appId: app.id, instanceId, enabled };
  const callbacks = useRef({ onClose, onPendingChange, onDeleted }); callbacks.current = { onClose, onPendingChange, onDeleted };
  const blocked = !enabled || pending !== null;

  function close() {
    if (operation.current) return;
    sequence.current++; approval.current = null;
    callbacks.current.onClose();
  }
  async function readPreview() {
    if (!context.current.enabled || operation.current) return;
    operation.current = 'preview'; setPending('preview'); callbacks.current.onPendingChange(true);
    approval.current = null; setVerified(false); setAcknowledged(false); setError('');
    const revision = ++sequence.current; const origin = context.current;
    const current = () => alive.current && sequence.current === revision && context.current.appId === origin.appId && context.current.instanceId === origin.instanceId;
    try {
      const value = await invoke<NodeAppDeletePreview>('node_app_delete_preview', { appId: origin.appId });
      if (!current() || !context.current.enabled) return;
      if (!validPreview(value, origin.appId)) throw new Error('删除范围不完整或项目身份不符，请重新核对。');
      approval.current = { appId: origin.appId, instanceId: origin.instanceId, revision: value.expectedRevision, sequence: revision };
      setPreview(value); setVerified(true);
    } catch (cause) { if (current() && context.current.enabled) setError(messageFrom(cause)); }
    finally {
      if (current()) { operation.current = null; setPending(null); callbacks.current.onPendingChange(false); }
    }
  }
  async function deleteApp() {
    const approvedContext = approval.current;
    if (blocked || operation.current || !preview || !verified || !acknowledged || preview.appId !== context.current.appId
      || !approvedContext || approvedContext.appId !== context.current.appId || approvedContext.instanceId !== context.current.instanceId
      || approvedContext.revision !== preview.expectedRevision || approvedContext.sequence !== sequence.current) return;
    const approved = preview; const origin = context.current; const revision = sequence.current;
    const current = () => alive.current && sequence.current === revision && context.current.appId === origin.appId && context.current.instanceId === origin.instanceId;
    operation.current = 'delete'; setPending('delete'); callbacks.current.onPendingChange(true); setError('');
    try {
      const result = await invoke<NodeAppDeleteResult>('node_app_delete', { appId: approved.appId, input: { expectedRevision: approved.expectedRevision } });
      if (!current()) return;
      if (!result || result.appId !== approved.appId || result.operationOutcome !== 'deleted' || result.ownership !== approved.ownership || result.sourcePreserved !== true || result.evidenceRetained !== true) throw new Error('删除结果尚不能确认。请保留当前记录并重新核对，不要重复提交。');
      approval.current = null; setVerified(false); setAcknowledged(false);
      callbacks.current.onDeleted(approved.appId, result);
    } catch (cause) {
      if (current()) { approval.current = null; setError(messageFrom(cause)); setVerified(false); setAcknowledged(false); }
    } finally {
      if (current()) { operation.current = null; setPending(null); callbacks.current.onPendingChange(false); }
    }
  }

  useEffect(() => {
    alive.current = true;
    void readPreview();
    return () => { alive.current = false; sequence.current++; approval.current = null; operation.current = null; callbacks.current.onPendingChange(false); };
  }, []);
  useEffect(() => {
    if (!enabled && operation.current !== 'delete') {
      sequence.current++; approval.current = null; operation.current = null; setPending(null); callbacks.current.onPendingChange(false); callbacks.current.onClose();
    }
  }, [enabled]);

  const preserved = preview ? [...new Set(preview.preservedPaths)] : [];
  const source = preview && preserved.includes(preview.sourcePath) ? preview.sourcePath : null;
  const otherPreserved = preserved.filter(path => path !== source);
  const base = preview ? commonDir([...preview.deletePaths, ...otherPreserved]) : '/';
  const recheck = <button type="button" className="text-button node-delete-recheck" disabled={blocked} onClick={() => void readPreview()}>重新核对删除范围</button>;

  return <Modal title="删除项目" titleIcon={<Trash2 size={22} aria-hidden="true" />} className="project-delete-modal node-delete-modal" closeDisabled={pending !== null} onClose={close}>
    <section className="node-delete-confirmation" aria-label="永久删除 Node 项目确认" aria-busy={pending !== null}>
      <div className="node-delete-body">
        <h3>永久删除「{preview?.name ?? app.name}」？此操作无法撤销。</h3>
        <p className="project-delete-warning"><TriangleAlert size={18} />不进入平台或系统回收站，平台也不会为本次删除保存可恢复副本。</p>
        {pending === 'preview' && <p role="status">正在核对本项目的删除范围…</p>}
        {error && <p className="task-error" role="alert">{error}</p>}
        {preview && <>
          <p>{preview.ownership === 'managed' ? '这是平台安装副本：将永久删除本实例的程序、业务数据、应用配置与密钥。原始来源副本和安装证据继续保留。' : '这是外部目录应用：只删除平台为本实例保存的数据、配置与密钥，并取消登记。原始应用目录及其中的全部文件保持原样。'}</p>
          <div className="node-delete-scope">
            <p className="node-delete-base">平台目录 <code title={base}>{base === '/' ? '/' : `${base}/`}</code></p>
            <section className="node-delete-removed">
              <div className="node-delete-scope-heading"><h4>将删除</h4>{recheck}</div>
              <PathGroups paths={preview.deletePaths} base={base} />
            </section>
            <section className="node-delete-kept">
              <div className="node-delete-scope-heading"><h4>继续保留</h4></div>
              {source && <p className="node-delete-source"><span>{preview.ownership === 'managed' ? '来源副本' : '原始应用目录'}</span><code data-path={source} title={source}>{source}</code></p>}
              {otherPreserved.length > 0 && <PathGroups paths={otherPreserved} base={base} />}
              <p className="node-delete-footnote">历史备份、旧版保留项目、迁移旧目录及操作与恢复证据也不会自动清理。</p>
            </section>
          </div>
          <p className="node-delete-id">项目 ID：<code>{preview.appId}</code></p>
        </>}
      </div>
      <div className="node-delete-footer">
        <label className="checkbox-label"><input type="checkbox" checked={acknowledged} disabled={blocked || !verified} onChange={event => setAcknowledged(event.target.checked)} />我已关闭项目窗口、核对上述范围，并理解永久删除无法撤销</label>
        <div className="confirmation-actions">{!preview && recheck}<button className="refresh-button" disabled={pending !== null} onClick={close}>取消，保留项目</button><button className="danger-button" disabled={blocked || !verified || !acknowledged} onClick={() => void deleteApp()}>{pending === 'delete' ? '正在永久删除…' : '确认永久删除项目'}</button></div>
      </div>
    </section>
  </Modal>;
}
