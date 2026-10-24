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

  return <Modal title="删除项目" titleIcon={<Trash2 size={22} aria-hidden="true" />} description={`核对「${app.name}」的永久删除范围`} className="project-delete-modal node-delete-modal" closeDisabled={pending !== null} onClose={close}>
    <section className="node-delete-confirmation" aria-label="永久删除 Node 项目确认" aria-busy={pending !== null}>
      <h3>永久删除「{preview?.name ?? app.name}」？此操作无法撤销。</h3>
      <p className="project-delete-warning"><TriangleAlert size={18} />本次删除不进入平台或系统回收站，无法从回收站恢复。</p>
      <p>请先正常关闭项目窗口，并保存需要的内容。平台不会为本次删除保存可恢复副本，也不会移入历史保留项目。</p>
      {pending === 'preview' && <p role="status">正在核对本项目的删除范围…</p>}
      {error && <p className="task-error" role="alert">{error}</p>}
      {preview && <>
        <p>{preview.ownership === 'managed' ? '这是平台安装副本：将永久删除本实例的程序、业务数据、应用配置与密钥。原始来源副本和安装证据继续保留。' : '这是外部目录应用：只永久删除平台为本实例保存的数据、配置与密钥，并取消登记。原始应用目录及其中的全部文件保持原样。'}</p>
        <p>项目 ID：<code>{preview.appId}</code></p>
        <div className="project-delete-scope"><h4>本次删除</h4><ul className="node-delete-paths">{preview.deletePaths.map(path => <li key={path}><code>{path}</code></li>)}</ul></div>
        <div className="project-delete-scope"><h4>继续保留</h4><ul className="node-delete-paths">{[...new Set(preview.preservedPaths)].map(path => <li key={path}><code>{path}</code></li>)}</ul></div>
        <p>历史备份、旧版保留项目、迁移旧目录及操作与恢复证据不会自动清理。</p>
      </>}
      <label className="checkbox-label"><input type="checkbox" checked={acknowledged} disabled={blocked || !verified} onChange={event => setAcknowledged(event.target.checked)} />我已关闭项目窗口，核对删除与保留目录，并理解永久删除无法撤销</label>
      <div className="confirmation-actions"><button className="refresh-button" disabled={pending !== null} onClick={close}>取消，保留项目</button><button className="refresh-button" disabled={blocked} onClick={() => void readPreview()}>重新核对删除范围</button><button className="danger-button" disabled={blocked || !verified || !acknowledged} onClick={() => void deleteApp()}>{pending === 'delete' ? '正在永久删除…' : '确认永久删除项目'}</button></div>
    </section>
  </Modal>;
}
