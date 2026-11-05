import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Copy, RefreshCw, Sparkles, X } from 'lucide-react';
import { createContext, useCallback, useContext, useEffect, useId, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { Modal } from './Modal';
import { t } from './i18n';
import { useFrontendPlatform } from './frontend-platform';
import { onProjectDeleted, projectWasDeleted } from './project-deletion';
import { messageFrom } from './ui-utils';
import { AI_TOOL_PREFERENCES_KEY, aiToolName, checkedAiHandoffResult, checkedAiHandoffTarget, checkedAiTools, readAiToolPreferences } from './ai-tools';
import type { AiHandoffResult, AiHandoffTarget, AiTool, AiToolId, AiToolPreferences } from './ai-tools';

interface AiToolsContextValue {
  enabled: boolean;
  selectedTool: AiTool | null;
  selectedToolName: string | null;
  tools: AiTool[];
  detecting: boolean;
  busy: boolean;
  notice: string;
  openChooser: () => void;
  detectTools: () => Promise<void>;
  handoff: (target: AiHandoffTarget) => Promise<void>;
}
const AiToolsContext = createContext<AiToolsContextValue>({
  enabled: false, selectedTool: null, selectedToolName: null, tools: [], detecting: false, busy: false, notice: '',
  openChooser: () => {}, detectTools: async () => {}, handoff: async () => {},
});
export const useAiTools = () => useContext(AiToolsContext);
let clipboardQueue = Promise.resolve();
interface Guide { result: AiHandoffResult; copied: boolean; manual: boolean; epoch: number; target: AiHandoffTarget }
const targetId = (target: AiHandoffTarget) => target.projectId ?? target.appId;

export function AiToolsProvider({ children, instanceId, scopeKey, enabled: allowed = true }: {
  children: ReactNode;
  instanceId?: string;
  scopeKey?: string;
  enabled?: boolean;
}) {
  const platform = useFrontendPlatform();
  const enabled = platform.aiHandoffAvailable && isTauri() && allowed && Boolean(instanceId);
  const [preferences, setPreferences] = useState(readAiToolPreferences);
  const preferencesRef = useRef(preferences); preferencesRef.current = preferences;
  const [tools, setTools] = useState<AiTool[]>(() => checkedAiTools([]));
  const [detecting, setDetecting] = useState(false);
  const [detectionError, setDetectionError] = useState('');
  const [chooserOpen, setChooserOpen] = useState(false);
  const [choice, setChoice] = useState<AiToolId>('other');
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState('');
  const [guide, setGuide] = useState<Guide | null>(null);
  const [hideGuide, setHideGuide] = useState(false);
  const [copying, setCopying] = useState(false);
  const [showText, setShowText] = useState(false);
  const pendingTarget = useRef<AiHandoffTarget | null>(null);
  const activeTarget = useRef<AiHandoffTarget | null>(null);
  const alive = useRef(true);
  const epoch = useRef(0);
  const identity = `${scopeKey ?? ''}|${instanceId ?? ''}|${enabled}`;
  const currentIdentity = useRef(identity);
  // Revoke async results during render, before effects or clipboard continuations.
  if (currentIdentity.current !== identity) { currentIdentity.current = identity; epoch.current++; }
  const latest = useRef({ enabled, instanceId }); latest.current = { enabled, instanceId };
  const operation = useRef<number | null>(null);
  const operationSequence = useRef(0);
  const detectionSequence = useRef(0);
  const id = useId();
  const selectedTool = preferences.selectedToolId === null ? null : preferences.selectedToolId === 'other'
    ? { id: 'other' as const, name: aiToolName('other'), installed: true, oneClick: false, detectedBy: [] }
    : tools.find(tool => tool.id === preferences.selectedToolId) ?? null;

  const savePreferences = useCallback((next: AiToolPreferences) => {
    preferencesRef.current = next; setPreferences(next);
    try { window.localStorage.setItem(AI_TOOL_PREFERENCES_KEY, JSON.stringify(next)); }
    catch { setNotice(t('当前选择已生效，但没能保存；下次打开可能需要重新选择。')); }
  }, []);
  const detectTools = useCallback(async () => {
    if (!latest.current.enabled) return;
    const request = ++detectionSequence.current;
    const capturedEpoch = epoch.current;
    setDetecting(true); setDetectionError('');
    try {
      const value = checkedAiTools(await invoke<unknown>('detect_ai_tools'));
      if (alive.current && request === detectionSequence.current && capturedEpoch === epoch.current) setTools(value);
    } catch (cause) {
      if (alive.current && request === detectionSequence.current && capturedEpoch === epoch.current) setDetectionError(messageFrom(cause));
    } finally { if (alive.current && request === detectionSequence.current) setDetecting(false); }
  }, []);
  const openChooser = useCallback(() => {
    if (!latest.current.enabled) return;
    pendingTarget.current = null;
    setChoice(preferencesRef.current.selectedToolId ?? 'other'); setChooserOpen(true);
    void detectTools();
  }, [detectTools]);

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; epoch.current++; detectionSequence.current++; };
  }, []);
  useEffect(() => {
    pendingTarget.current = null; activeTarget.current = null;
    setChooserOpen(false); setGuide(null); setNotice(''); setCopying(false);
    if (!enabled) return;
    void detectTools();
    if (!preferencesRef.current.acknowledged) { setChoice('other'); setChooserOpen(true); }
  }, [identity, enabled, detectTools]);
  useEffect(() => onProjectDeleted(deletion => {
    if (deletion.instanceId !== latest.current.instanceId) return;
    const active = activeTarget.current;
    const waiting = pendingTarget.current;
    if (active && targetId(active) === deletion.id || waiting && targetId(waiting) === deletion.id) {
      epoch.current++; activeTarget.current = null; pendingTarget.current = null;
      setGuide(null); setNotice(''); setChooserOpen(false); setCopying(false);
    }
  }), []);

  const showManager = async () => { try { await invoke('ai_handoff_show_manager'); } catch { /* The main window retains the manual handoff. */ } };
  async function handoff(target: AiHandoffTarget, selectedId = preferencesRef.current.selectedToolId) {
    if (!latest.current.enabled || !checkedAiHandoffTarget(target) || projectWasDeleted(targetId(target)) || operation.current !== null) return;
    if (!selectedId) {
      const chooserEpoch = epoch.current;
      await showManager();
      if (!alive.current || !latest.current.enabled || chooserEpoch !== epoch.current || projectWasDeleted(targetId(target))) return;
      pendingTarget.current = target; setChoice('other'); setChooserOpen(true); void detectTools();
      return;
    }
    const sequence = ++operationSequence.current;
    const capturedEpoch = epoch.current;
    const capturedInstance = latest.current.instanceId;
    operation.current = sequence; activeTarget.current = target;
    setBusy(true); setGuide(null); setNotice('');
    const current = () => alive.current && latest.current.enabled && capturedEpoch === epoch.current
      && capturedInstance === latest.current.instanceId && !projectWasDeleted(targetId(target));
    try {
      let result: AiHandoffResult;
      try {
        result = checkedAiHandoffResult(await invoke<unknown>('open_ai_handoff', {
          toolId: selectedId, projectId: target.projectId ?? null, appId: target.appId ?? null,
        }));
      } catch {
        if (!current()) return;
        // A failed native open is never repeated. Only prepare the existing text.
        const prepared = await invoke<{ text: string; projectRoot: string; instanceId: string }>(
          target.projectId ? 'project_handoff' : 'node_app_handoff', target.projectId ? { projectId: target.projectId } : { appId: target.appId });
        result = checkedAiHandoffResult({ ...prepared, toolId: 'other', toolName: aiToolName('other'), oneClick: false,
          opened: false, appName: target.appName, warning: t('未能自动打开。你可以手动打开 AI 工具，选择下方项目文件夹并复制提示词。') });
      }
      if (!current() || result.instanceId !== capturedInstance) return;
      let copied = false;
      const write = clipboardQueue.then(async () => { if (current()) { await navigator.clipboard.writeText(result.text); copied = true; } });
      clipboardQueue = write.catch(() => {});
      await write.catch(() => {});
      if (!current()) return;
      const manual = !copied || !result.opened;
      if (result.oneClick && result.opened && copied && !result.warning) {
        setNotice(t('已请求在 {tool} 打开，请检查项目文件夹和提示词后发送。', { tool: result.toolName }));
      } else if (!manual && !result.warning && preferencesRef.current.hiddenGuides.includes(result.toolId)) {
        setNotice(result.warning || t('已请求打开，提示词已复制。请在 AI 工具中检查项目文件夹并粘贴。'));
      } else {
        setGuide({ result, copied, manual, epoch: capturedEpoch, target });
        setHideGuide(false); setShowText(manual); setCopying(false);
        await showManager();
      }
    } catch (cause) { if (current()) { setNotice(messageFrom(cause)); await showManager(); } }
    finally {
      if (operation.current === sequence) { operation.current = null; if (alive.current) setBusy(false); }
    }
  }
  const handoffRef = useRef(handoff); handoffRef.current = handoff;
  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<unknown>('ai-handoff-request', event => {
      const target = checkedAiHandoffTarget(event.payload);
      if (target) void handoffRef.current(target);
    }).then(stop => { if (disposed) stop(); else unlisten = stop; }).catch(() => {});
    return () => { disposed = true; unlisten?.(); };
  }, [enabled]);

  function closeChooser() {
    pendingTarget.current = null; setChooserOpen(false);
    savePreferences({ ...preferencesRef.current, acknowledged: true });
  }
  function confirmChoice() {
    const target = pendingTarget.current; pendingTarget.current = null;
    savePreferences({ ...preferencesRef.current, acknowledged: true, selectedToolId: choice });
    setChooserOpen(false);
    if (target) void handoff(target, choice);
  }
  function closeGuide() {
    if (guide && hideGuide && !guide.manual) savePreferences({ ...preferencesRef.current,
      hiddenGuides: [...new Set([...preferencesRef.current.hiddenGuides, guide.result.toolId])] });
    setGuide(null); setCopying(false);
  }
  async function copyAgain() {
    const captured = guide;
    if (!captured || copying) return;
    const current = () => alive.current && captured.epoch === epoch.current && !projectWasDeleted(targetId(captured.target));
    if (!current()) return;
    setCopying(true);
    let copied = false;
    const write = clipboardQueue.then(async () => { if (current()) { await navigator.clipboard.writeText(captured.result.text); copied = true; } });
    clipboardQueue = write.catch(() => {});
    await write.catch(() => {});
    if (!current()) return;
    setCopying(false);
    setGuide(value => value === captured ? { ...captured, copied, manual: !copied || captured.manual } : value);
    if (!copied) setShowText(true);
  }
  const detected = tools.filter(tool => tool.installed);
  const missing = tools.filter(tool => !tool.installed);
  const validChoice = choice === 'other' || detected.some(tool => tool.id === choice);
  const guideResult = guide?.result;
  const value: AiToolsContextValue = { enabled, selectedTool, selectedToolName: selectedTool?.name ?? null, tools,
    detecting, busy, notice, openChooser, detectTools, handoff };
  return <AiToolsContext.Provider value={value}>
    {children}
    {enabled && notice && <div className="ai-tool-notice" role="status"><span>{t(notice)}</span><button type="button" className="text-button" aria-label={t('关闭提示')} onClick={() => setNotice('')}><X size={15} /></button></div>}
    {enabled && chooserOpen && <Modal title={t('你用哪个 AI 工具改应用？')} className="ai-tool-chooser" onClose={closeChooser}>
      <p className="ai-tool-lead">{t('熊猫小抽屉不自带 AI。选一个能修改电脑上文件的 AI 工具，之后应用详情里的按钮会帮你打开它。随时可以在设置里更换。')}</p>
      <fieldset className="ai-tool-options" disabled={busy}>
        <legend>{detecting ? t('正在检测这台电脑上的 AI 工具…') : t('检测到 {count} 个 AI 工具', { count: detected.length })}</legend>
        {detected.map(tool => <label key={tool.id} className={`ai-tool-option${choice === tool.id ? ' is-selected' : ''}`}>
          <input type="radio" name={`${id}-tool`} value={tool.id} checked={choice === tool.id} onChange={() => setChoice(tool.id)} />
          <span className="ai-tool-option-copy"><strong>{tool.name}</strong><span>{t(tool.id === 'qoder' ? '打开 Qoder，选择或确认项目文件夹，再粘贴提示词' : tool.oneClick ? '尝试打开应用文件夹并填入提示词' : '打开应用文件夹，提示词复制后粘贴到 AI 对话')}{tool.setupHint && <span className="ai-tool-setup-hint">{t(tool.setupHint)}</span>}</span></span>
          <span className={`ai-tool-tier${tool.oneClick ? ' is-one-click' : ''}`}>{t(tool.oneClick ? '一键打开' : '打开后粘贴')}</span>
        </label>)}
        <label className={`ai-tool-option${choice === 'other' ? ' is-selected' : ''}`}><input type="radio" name={`${id}-tool`} checked={choice === 'other'} onChange={() => setChoice('other')} /><span className="ai-tool-option-copy"><strong>{t(aiToolName('other'))}</strong><span>{t('复制提示词，并在访达中打开项目文件夹')}</span></span></label>
      </fieldset>
      <div className="ai-tool-detection"><p>{missing.length > 0 && t('没检测到：{tools}', { tools: missing.map(tool => tool.name).join('、') })}</p><button type="button" className="refresh-button" disabled={detecting || busy} onClick={() => void detectTools()}><RefreshCw size={14} />{t('重新检测')}</button></div>
      {detectionError && <p className="project-help" role="status">{t('暂时无法完成检测，可以重试，或先选“其他 / 我还不确定”。')}</p>}
      <p className="ai-tool-browser-hint">{t('网页版 AI（豆包、ChatGPT 网页版等）改不了电脑上的文件，不能用来改应用。')}</p>
      <div className="ai-tool-footer"><button type="button" className="refresh-button" onClick={closeChooser}>{t('以后再说')}</button><button type="button" className="primary-button" disabled={!validChoice || busy} onClick={confirmChoice}>{choice === 'other' ? t('保存选择') : t('保存，以后都用 {tool}', { tool: aiToolName(choice) })}</button></div>
    </Modal>}
    {enabled && guide && guideResult && <Modal title={guide.manual ? t('把 {app} 交给你的 AI', { app: guideResult.appName }) : guideResult.toolId === 'other' ? t('在你的 AI 工具中打开 {app}', { app: guideResult.appName }) : t('已请求在 {tool} 中打开 {app}', { tool: guideResult.toolName, app: guideResult.appName })} className="ai-tool-guide" onClose={closeGuide}>
      <p className="ai-tool-lead" role="status">{guide.copied ? t('提示词已复制。') : t('无法自动复制，请选中下方完整提示词手动复制。')}{guideResult.warning && ` ${t(guideResult.warning)}`}</p>
      <ol className="ai-tool-guide-steps"><li><strong>{guideResult.toolId === 'other' || !guideResult.opened ? t('手动打开能修改本机文件的 AI 工具') : guideResult.toolId === 'qoder' ? t('在 Qoder 中选择或确认项目文件夹') : t('切到 {tool}，打开 AI 对话', { tool: guideResult.toolName })}</strong><p>{t('确认文件列表是 {app} 的项目文件夹，选择能修改文件的智能体模式。', { app: guideResult.appName })}</p></li><li><strong>{t('粘贴提示词，发送')}</strong><p>{t('AI 会先读懂应用，再问你想怎么改。告诉它你的需求，让它说明计划，确认后再修改。')}</p></li></ol>
      <dl className="ai-tool-path"><dt>{t('项目文件夹')}</dt><dd><code>{guideResult.projectRoot}</code></dd></dl>
      <p className="project-help">{t('修改完成后，按 AI 的说明操作，再回到熊猫小抽屉使用应用。')}</p>
      {!showText && <button className="text-button" type="button" onClick={() => setShowText(true)}>{t('查看完整提示词')}</button>}
      {showText && <div className="ai-tool-manual-copy"><label htmlFor={`${id}-manual`}>{t('完整提示词 · 可手动复制')}</label><textarea id={`${id}-manual`} value={guideResult.text} readOnly rows={9} onFocus={event => event.currentTarget.select()} /></div>}
      <div className="ai-tool-footer">{!guide.manual && <label className="ai-tool-hide-guide"><input type="checkbox" checked={hideGuide} onChange={event => setHideGuide(event.target.checked)} />{t('下次不再显示')}</label>}<button type="button" className="refresh-button" disabled={copying} onClick={() => void copyAgain()}><Copy size={14} />{copying ? t('正在复制…') : t('再复制一次')}</button><button type="button" className="primary-button" onClick={closeGuide}>{t('知道了')}</button></div>
    </Modal>}
  </AiToolsContext.Provider>;
}

export function AiToolSettings() {
  const ai = useAiTools();
  if (!ai.enabled) return null;
  return <div className="settings-row ai-tool-settings"><div><strong>{t('我的 AI 工具')}</strong><p>{ai.selectedTool ? `${ai.selectedTool.name} · ${t(ai.selectedTool.oneClick ? '一键打开' : '打开后粘贴')}` : t('还没选择 AI 工具')}</p></div><div className="ai-tool-settings-actions"><button type="button" className="refresh-button" disabled={ai.busy} onClick={ai.openChooser}><Sparkles size={14} />{t(ai.selectedTool ? '更换' : '选择 AI 工具')}</button><button type="button" className="refresh-button" disabled={ai.detecting || ai.busy} onClick={() => void ai.detectTools()}><RefreshCw size={14} />{t(ai.detecting ? '检测中…' : '重新检测')}</button></div></div>;
}
