import { invoke } from '@tauri-apps/api/core';
import { Copy, FolderOpen, RefreshCw, Server } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import { NodeDataContractInfo } from './NodeDataContractInfo';
import type { NodeAppDetail, NodeDirectoryReview } from './node-app-types';
import { messageFrom } from './ui-utils';

type Operation = 'choose' | 'inspect' | 'initialize' | 'register' | 'copy' | null;

function checkedReview(value: NodeDirectoryReview, sourcePath: string): NodeDirectoryReview {
  if (!value || value.sourcePath !== sourcePath || !['empty', 'ready', 'needs-review'].includes(value.state)
    || typeof value.expectedRevision !== 'string' || !value.expectedRevision
    || !Array.isArray(value.issues) || !value.issues.every(issue => typeof issue === 'string')
    || typeof value.reviewPrompt !== 'string') throw new Error('目录检查结果不完整或路径不符，请重新检查。');
  if (value.state === 'ready' && (!value.inspection || value.inspection.runtime !== 'node-service'
    || !value.inspection.app || typeof value.inspection.app.name !== 'string' || typeof value.inspection.app.version !== 'string'
    || !Array.isArray(value.inspection.configuration?.fields)
    || !value.inspection.configuration.fields.every(field => field && typeof field.label === 'string' && typeof field.required === 'boolean'))) throw new Error('项目预览不完整，请重新检查目录。');
  if (value.state === 'needs-review' && !value.reviewPrompt) throw new Error('缺少给 AI 的检查说明，请重新检查目录。');
  return value;
}

export function LocalDirectoryPanel({ enabled, instanceId, onPendingChange, onAdded }: {
  enabled: boolean;
  instanceId?: string;
  onPendingChange: (pending: boolean) => void;
  onAdded: (app: NodeAppDetail) => void;
}) {
  const [sourcePath, setSourcePath] = useState('');
  const [name, setName] = useState('');
  const [review, setReview] = useState<NodeDirectoryReview | null>(null);
  const [reviewPrompt, setReviewPrompt] = useState('');
  const [initializeConfirm, setInitializeConfirm] = useState(false);
  const [initializeAcknowledged, setInitializeAcknowledged] = useState(false);
  const [pending, setPending] = useState<Operation>(null);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [copyNotice, setCopyNotice] = useState('');
  const promptField = useRef<HTMLTextAreaElement>(null);
  const alive = useRef(true);
  const operation = useRef(false);
  const sequence = useRef(0);
  const context = useRef({ enabled, instanceId }); context.current = { enabled, instanceId };
  const callbacks = useRef({ onPendingChange, onAdded }); callbacks.current = { onPendingChange, onAdded };
  const id = useId();
  const blocked = !enabled || pending !== null;

  function revokeReview() {
    setReview(null); setInitializeConfirm(false); setInitializeAcknowledged(false);
  }
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; sequence.current++; operation.current = false; callbacks.current.onPendingChange(false); };
  }, []);
  useEffect(() => {
    sequence.current++; operation.current = false; setPending(null); callbacks.current.onPendingChange(false);
    revokeReview(); setCopyNotice('');
    if (sourcePath) setNotice('接入状态已变化，请重新检查目录后继续。');
  }, [instanceId, enabled]);

  async function run(kind: Exclude<Operation, null>, perform: (current: () => boolean) => Promise<void>) {
    if (operation.current || !context.current.enabled) return;
    operation.current = true; setPending(kind); callbacks.current.onPendingChange(true);
    setError(''); setNotice(''); setCopyNotice('');
    const request = ++sequence.current; const service = context.current.instanceId;
    const current = () => alive.current && sequence.current === request && context.current.enabled && context.current.instanceId === service;
    try { await perform(current); }
    catch (cause) { if (current()) { if (kind !== 'copy') revokeReview(); setError(messageFrom(cause)); } }
    finally { if (current()) { operation.current = false; setPending(null); callbacks.current.onPendingChange(false); } }
  }
  async function inspect(source: string, current: () => boolean) {
    const value = await invoke<NodeDirectoryReview>('node_app_directory_inspect', { sourcePath: source });
    if (!current()) return;
    const checked = checkedReview(value, source);
    setReview(checked); setReviewPrompt(checked.reviewPrompt);
  }
  function choose() {
    if (blocked || operation.current) return;
    revokeReview();
    void run('choose', async current => {
      const source = await invoke<string | null>('choose_path', { kind: 'nodeapp' });
      if (!current()) return;
      if (!source) { setNotice('已取消选择。已选目录仍保留，重新检查后才能继续。'); return; }
      if (!source.startsWith('/')) throw new Error('选择器未返回有效的本地目录，请重新选择。');
      setSourcePath(source); setName(source.split('/').filter(Boolean).at(-1) ?? '新项目'); setReviewPrompt('');
      await inspect(source, current);
    });
  }
  function recheck() {
    if (blocked || !sourcePath || operation.current) return;
    revokeReview();
    void run('inspect', current => inspect(sourcePath, current));
  }
  function initialize() {
    if (blocked || !review || review.state !== 'empty' || !initializeConfirm || !initializeAcknowledged || !name.trim() || operation.current) return;
    const selected = review; revokeReview();
    void run('initialize', async current => {
      const value = await invoke<NodeDirectoryReview>('node_app_directory_initialize', { sourcePath: selected.sourcePath, input: { expectedRevision: selected.expectedRevision, name: name.trim() } });
      if (!current()) return;
      const checked = checkedReview(value, selected.sourcePath);
      if (checked.state !== 'ready') throw new Error('生成结果尚不能确认。请重新检查目录，不要重复生成。');
      setReview(checked); setReviewPrompt(checked.reviewPrompt);
      setNotice('起始项目已生成，尚未添加或运行。请核对下方项目预览，再明确确认添加。');
    });
  }
  function register() {
    if (blocked || !review || review.state !== 'ready' || operation.current) return;
    const selected = review; revokeReview();
    void run('register', async current => {
      const value = await invoke<NodeAppDetail>('node_app_directory_register', { sourcePath: selected.sourcePath, expectedRevision: selected.expectedRevision });
      if (!current()) return;
      if (!value || !value.id || value.sourcePath !== selected.sourcePath || value.runtime !== 'node-service' || !Array.isArray(value.fields) || !value.configuration) throw new Error('添加结果尚不能确认。请重新检查登记状态，不要重复添加。');
      callbacks.current.onAdded(value);
    });
  }
  function copy(value: string, prompt: boolean) {
    if (blocked || !value || operation.current) return;
    void run('copy', async current => {
      try {
        if (!navigator.clipboard?.writeText) throw new Error('剪贴板不可用');
        await navigator.clipboard.writeText(value);
        if (current()) setCopyNotice(prompt ? '完整检查说明已复制，请自行粘贴给能读取本机目录的 AI。' : '目录路径已复制。');
      } catch {
        if (current()) setCopyNotice(prompt ? '复制未完成。完整说明仍在下方，可选中后手动复制。' : '复制未完成，可选中上方完整目录路径后手动复制。');
      }
    });
  }

  const inspection = review?.state === 'ready' ? review.inspection : null;
  return <div className="node-add-dialog local-directory-panel" aria-label="本地项目文件夹接入" aria-busy={pending !== null}>
    <p>选择本机项目文件夹。平台先只读检查目录，生成起始项目和添加项目都需要你分别明确确认。</p>
    <div className="local-directory-actions"><button className="refresh-button" disabled={blocked} onClick={choose}><FolderOpen size={16} />{pending === 'choose' ? '正在选择与检查…' : sourcePath ? '重新选择文件夹' : '选择本地项目文件夹'}</button><button className="refresh-button" disabled={blocked || !sourcePath} onClick={recheck}><RefreshCw size={14} />{pending === 'inspect' ? '正在检查…' : '重新检查'}</button></div>
    {sourcePath && <div className="local-directory-path"><label htmlFor={`${id}-path`}>本地项目目录</label><input id={`${id}-path`} className="node-selected-source" readOnly value={sourcePath} /><button className="text-button" disabled={blocked} onClick={() => copy(sourcePath, false)}><Copy size={13} />复制目录路径</button></div>}
    {error && <p className="task-error" role="alert">{error} 请重新检查或重新选择目录后继续。</p>}
    {notice && <p className="workspace-inline-notice" role="status">{notice}</p>}
    {review && review.issues.length > 0 && <ul aria-label="目录检查说明">{review.issues.map((issue, index) => <li key={index}>{issue}</li>)}</ul>}
    {review?.state === 'empty' && <section className="local-directory-empty" aria-label="空目录初始化">
      <h3>这是一个空目录</h3><p>可在此生成 Node 起始项目。生成会写入所选目录，不会添加项目、安装依赖或启动应用。以后从平台删除此类外部目录应用时，原始目录仍保留。</p>
      <label htmlFor={`${id}-name`}>项目名称<input id={`${id}-name`} value={name} maxLength={200} disabled={blocked} onChange={event => { setName(event.target.value); setInitializeConfirm(false); setInitializeAcknowledged(false); }} /></label>
      {!initializeConfirm && <button className="primary-button" disabled={blocked || !name.trim()} onClick={() => { setInitializeConfirm(true); setInitializeAcknowledged(false); }}>在空目录生成起始项目…</button>}
      {initializeConfirm && <div className="operation-confirmation" aria-label="生成起始项目确认"><strong>为「{name.trim()}」生成 Node 起始项目？</strong><code>{sourcePath}</code><p>这会向上方空目录写入起始文件。生成完成后仍需核对并明确确认添加，不会自动运行。</p><label className="checkbox-label"><input type="checkbox" checked={initializeAcknowledged} disabled={blocked} onChange={event => setInitializeAcknowledged(event.target.checked)} />我已核对目录与项目名称，同意写入起始文件</label><div className="confirmation-actions"><button className="primary-button" disabled={blocked || !initializeAcknowledged || !name.trim()} onClick={initialize}>确认生成起始项目</button><button className="refresh-button" disabled={blocked} onClick={() => { setInitializeConfirm(false); setInitializeAcknowledged(false); }}>取消，保留空目录</button></div></div>}
    </section>}
    {inspection && <section className="node-inspection-preview" aria-label="可添加项目预览"><h3>{inspection.app.name}</h3><p>{inspection.app.description}</p><span className="node-runtime-label"><Server size={14} />{inspection.runtimeLabel} · {inspection.app.version}</span><NodeDataContractInfo value={inspection.dataContract} legacyNotice="平台未验证实际数据格式。" /><p>应用声明了 {inspection.configuration.fields.length} 项配置，其中 {inspection.configuration.fields.filter(field => field.required).length} 项必填。添加不读取旧的 .env，也不会自动运行应用。</p><ul>{inspection.configuration.fields.map((field, index) => <li key={index}>{field.label}{field.required ? '（必填）' : '（选填）'}{field.secret ? ' · 密钥' : ''}</li>)}</ul><p>这是外部目录接入，运行依赖上方目录；从平台删除应用时，原始目录及其中的文件仍保留。</p><button className="primary-button" disabled={blocked} onClick={register}>确认添加此项目</button></section>}
    {review?.state === 'needs-review' && <section className="local-directory-review"><h3>需要进一步检查</h3><p>平台尚未判断这个目录是否适配。可把下方完整说明交给能读取本机目录的 AI，请它先检查再建议下一步。</p><p>复制说明不会发送给 AI，也不会初始化、修改或运行这个目录。处理后请回到这里重新检查。</p></section>}
    {reviewPrompt && (review?.state === 'needs-review' || !review) && <section className="local-directory-prompt"><label htmlFor={`${id}-prompt`}>{review ? '给 AI 的完整检查说明' : '上次保留的检查说明（重新检查后更新）'}</label><textarea ref={promptField} id={`${id}-prompt`} aria-label="给 AI 的检查说明" readOnly rows={9} value={reviewPrompt} /><div className="local-directory-actions"><button className="refresh-button" disabled={blocked} onClick={() => copy(reviewPrompt, true)}><Copy size={14} />复制给 AI 检查</button><button className="text-button" onClick={() => { promptField.current?.focus(); promptField.current?.select(); }}>选择完整说明</button></div></section>}
    {copyNotice && <p className="workspace-inline-notice" role="status">{copyNotice}</p>}
  </div>;
}
