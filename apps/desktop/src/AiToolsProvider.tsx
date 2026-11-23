import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Check, Copy, FolderOpen, RefreshCw, X } from 'lucide-react';
import { createContext, useCallback, useContext, useEffect, useId, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { Modal } from './Modal';
import { t } from './i18n';
import { useFrontendPlatform } from './frontend-platform';
import { onProjectDeleted, projectWasDeleted } from './project-deletion';
import { messageFrom } from './ui-utils';
import { localizePlatformPrompt } from './platform-prompt';
import { AI_TOOL_PREFERENCES_KEY, aiHandoffWarning, aiToolName, aiToolShortName, checkedAiHandoffResult, checkedAiHandoffTarget, checkedAiTools, readAiToolPreferences } from './ai-tools';
import type { AiHandoffResult, AiHandoffTarget, AiTool, AiToolCapability, AiToolId, AiToolPreferences } from './ai-tools';

interface AiToolsContextValue {
  enabled: boolean;
  selectedTool: AiTool | null;
  selectedToolName: string | null;
  /** Name for buttons such as 「告诉 Codex 怎么改」: the chosen tool's short name, or "AI" when none can be opened. */
  buttonTool: string;
  /** The chosen tool's name when it is no longer found on this computer. */
  lostToolName: string | null;
  tools: AiTool[];
  detecting: boolean;
  busy: boolean;
  notice: string;
  openChooser: () => void;
  selectTool: (toolId: AiToolId) => void;
  detectTools: () => Promise<void>;
  handoff: (target: AiHandoffTarget) => Promise<void>;
}
const AiToolsContext = createContext<AiToolsContextValue>({
  enabled: false, selectedTool: null, selectedToolName: null, buttonTool: 'AI', lostToolName: null, tools: [], detecting: false, busy: false, notice: '',
  openChooser: () => {}, selectTool: () => {}, detectTools: async () => {}, handoff: async () => {},
});
export const useAiTools = () => useContext(AiToolsContext);
let clipboardQueue: Promise<unknown> = Promise.resolve();
function copyHandoffText(text: string, nativeFallback: boolean, current: () => boolean): Promise<boolean> {
  const write = clipboardQueue.then(async () => {
    if (!current()) return false;
    try { await navigator.clipboard.writeText(text); }
    catch {
      // A native menu or external tool can hold focus after the handoff.
      if (!nativeFallback || !current()) return false;
      try { await invoke<void>('copy_ai_handoff_text', { text }); }
      catch { return false; }
    }
    return true;
  });
  clipboardQueue = write.catch(() => {});
  return write;
}
/** `result` is null while the tool is still being started. */
interface Guide { result: AiHandoffResult | null; copied: boolean; epoch: number; target: AiHandoffTarget; selectedId: AiToolId }
function needsManualCopyOrOpen(result: AiHandoffResult, copied: boolean): boolean {
  return !result.opened || (!result.prefilled && !copied);
}
const targetId = (target: AiHandoffTarget) => target.projectId ?? target.appId ?? target.sourcePath;
/** Chooser line: what one click does with this tool (Chinese source text). */
export function aiToolCapabilityText(capability: AiToolCapability): string {
  switch (capability) {
    case 'full': return '选好应用文件夹、新建对话、填好提示词，你只要发送';
    case 'folder-prefill': return '打开应用文件夹，尽量填好提示词，没填上就粘贴';
    case 'folder': return '打开应用文件夹，你新建对话再粘贴提示词';
    case 'launch': return '打开工具，你新建对话再粘贴提示词';
    case 'manual': return '不能自动打开，复制提示词后你自己操作';
  }
}
/** Short tier label for a capability (Chinese source text). */
export function aiToolTierText(capability: AiToolCapability): string {
  return capability === 'full' ? '全自动' : capability === 'manual' ? '手动' : '要粘贴';
}
const capabilityRank: Record<AiToolCapability, number> = { full: 0, 'folder-prefill': 1, folder: 2, launch: 3, manual: 4 };
/** Detected tools, the ones that do the most with one click first. */
const byCapability = (tools: AiTool[]) => tools.filter(tool => tool.installed).sort((a, b) => capabilityRank[a.capability] - capabilityRank[b.capability]);
const defaultEntry = (target: AiHandoffTarget) => target.entry ?? (target.purpose === 'create' ? 'create' : target.purpose === 'repair' ? 'repair' : 'detail');

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
  const guideRef = useRef(guide); guideRef.current = guide;
  const [hideGuide, setHideGuide] = useState(false);
  const [copying, setCopying] = useState(false);
  const [showText, setShowText] = useState(false);
  const [pathNote, setPathNote] = useState<'' | 'copied' | 'copy-failed' | 'open-failed'>('');
  const choiceTouched = useRef(false);
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
    ? { id: 'other' as const, name: aiToolName('other'), installed: true, oneClick: false, capability: 'manual' as const, detectedBy: [] }
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
  const selectTool = useCallback((toolId: AiToolId) => {
    if (!latest.current.enabled) return;
    savePreferences({ ...preferencesRef.current, acknowledged: true, selectedToolId: toolId });
  }, [savePreferences]);
  const openChooser = useCallback(() => {
    if (!latest.current.enabled) return;
    pendingTarget.current = null;
    choiceTouched.current = false;
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
    // The chooser appears when the user first hands an app to AI, not at startup.
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
  /** `early`: show the panel before the tool has started. Requests from app windows wait for the native answer, so a stale window never brings the manager forward. */
  async function handoff(target: AiHandoffTarget, selectedId = preferencesRef.current.selectedToolId, early = true) {
    if (!latest.current.enabled || !checkedAiHandoffTarget(target) || projectWasDeleted(targetId(target)) || operation.current !== null) return;
    if (!selectedId) {
      const chooserEpoch = epoch.current;
      await showManager();
      if (!alive.current || !latest.current.enabled || chooserEpoch !== epoch.current || projectWasDeleted(targetId(target))) return;
      // After the choice is saved, confirmChoice continues with this target.
      pendingTarget.current = target; choiceTouched.current = false; setChoice('other'); setChooserOpen(true); void detectTools();
      return;
    }
    const sequence = ++operationSequence.current;
    const capturedEpoch = epoch.current;
    const capturedInstance = latest.current.instanceId;
    operation.current = sequence; activeTarget.current = target;
    setBusy(true); setNotice(''); setPathNote('');
    // A tool whose guide was dismissed for good only gets a short notice.
    const quiet = preferencesRef.current.hiddenGuides.includes(selectedId);
    const canLaunch = tools.some(tool => tool.id === selectedId && tool.installed && tool.capability !== 'manual');
    if (quiet || !early || !canLaunch) setGuide(null);
    else {
      // Shown at once: starting the tool can take a while, and the user should not type into it yet.
      setGuide({ result: null, copied: false, epoch: capturedEpoch, target, selectedId });
      setHideGuide(false); setShowText(false); setCopying(false);
      void showManager();
    }
    let settled = false;
    const current = () => alive.current && latest.current.enabled && capturedEpoch === epoch.current
      && capturedInstance === latest.current.instanceId && !projectWasDeleted(targetId(target));
    try {
      let result: AiHandoffResult;
      try {
        result = checkedAiHandoffResult(await invoke<unknown>('open_ai_handoff', {
          toolId: selectedId, projectId: target.projectId ?? null, appId: target.appId ?? null,
          purpose: target.purpose ?? null, text: target.text ?? null, sourcePath: target.sourcePath ?? null, entry: defaultEntry(target),
          ...(target.expectedWindowGeneration === undefined ? {} : {expectedWindowGeneration: target.expectedWindowGeneration}),
        }));
      } catch (cause) {
        if (cause && typeof cause === 'object' && 'code' in cause && cause.code === 'APP_WINDOW_ACTION_EXPIRED') return;
        if (!current()) return;
        // A failed native open is never repeated. Only prepare the existing text.
        const prepared = target.sourcePath && target.text
          ? {text: target.text, projectRoot: target.sourcePath, instanceId: capturedInstance ?? ''}
          : await invoke<Pick<AiHandoffResult, 'text' | 'projectRoot' | 'displayProjectRoot' | 'instanceId'>>('app_handoff', {appId: target.projectId ?? target.appId, purpose: target.purpose === 'create' ? 'create' : null});
        result = checkedAiHandoffResult({ ...prepared, ...(target.text ? { text: target.text } : {}), toolId: 'other', toolName: aiToolName('other'), oneClick: false,
          opened: false, capability: 'manual', prefilled: false, appName: target.appName, warningCode: 'TOOL_OPEN_FAILED' });
      }
      if (!current() || result.instanceId !== capturedInstance) return;
      if (target.text === undefined) result = { ...result, text: localizePlatformPrompt(result.text) };
      const copied = await copyHandoffText(result.text, platform.nativeAiHandoffClipboard, current);
      if (!current()) return;
      const manual = needsManualCopyOrOpen(result, copied);
      const warned = Boolean(result.warning || result.warningCode);
      const toolName = t(aiToolShortName(selectedId));
      settled = true;
      if (quiet && !manual && !warned) {
        setGuide(null);
        setNotice(result.prefilled ? t('已请求 {tool} 填入提示词，请检查草稿后再发送。', { tool: toolName }) : t('提示词已复制，在 {tool} 里新建对话后粘贴发送。', { tool: toolName }));
      } else {
        setGuide({ result, copied, epoch: capturedEpoch, target, selectedId });
        setHideGuide(false); setShowText(manual); setCopying(false);
        await showManager();
      }
    } catch (cause) { if (current()) { settled = true; setGuide(null); setNotice(messageFrom(cause)); await showManager(); } }
    finally {
      // Never leave the "starting" panel behind an abandoned request.
      if (!settled && alive.current) setGuide(value => value && value.result === null && value.target === target ? null : value);
      if (operation.current === sequence) { operation.current = null; if (alive.current) setBusy(false); }
    }
  }
  const handoffRef = useRef(handoff); handoffRef.current = handoff;
  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<unknown>('ai-handoff-request', event => {
      // Titlebar/menu requests carry a window generation; app pages carry text.
      const target = checkedAiHandoffTarget(event.payload, true);
      if (target) void handoffRef.current({ ...target, entry: target.expectedWindowGeneration !== undefined ? 'titlebar' : target.text ? 'app-page' : 'detail' }, undefined, false);
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
    if (guide?.result && hideGuide && !needsManualCopyOrOpen(guide.result, guide.copied)) savePreferences({ ...preferencesRef.current,
      hiddenGuides: [...new Set([...preferencesRef.current.hiddenGuides, guide.selectedId])] });
    guideRef.current = null;
    setGuide(null); setCopying(false); setPathNote('');
  }
  async function copyAgain() {
    const captured = guide;
    if (!captured?.result || copying) return;
    const text = captured.result.text;
    const current = () => alive.current && guideRef.current === captured && captured.epoch === epoch.current && !projectWasDeleted(targetId(captured.target));
    if (!current()) return;
    setCopying(true);
    const copied = await copyHandoffText(text, platform.nativeAiHandoffClipboard, current);
    if (!current()) return;
    setCopying(false); setPathNote('');
    setGuide(value => value === captured ? { ...captured, copied } : value);
    if (!copied) setShowText(true);
  }
  async function copyPath(path: string) {
    // The path replaces the prompt on the clipboard, so the note points at "copy the prompt again".
    try { await navigator.clipboard.writeText(path); setPathNote('copied'); } catch { setPathNote('copy-failed'); }
  }
  const ranked = byCapability(tools);
  const recommendedId: AiToolId = ranked.find(tool => tool.capability !== 'manual')?.id ?? 'other';
  // Until the user picks one, the chooser preselects the tool that does the most with one click.
  useEffect(() => {
    if (chooserOpen && !choiceTouched.current && preferencesRef.current.selectedToolId === null) setChoice(recommendedId);
  }, [chooserOpen, recommendedId]);
  const missing = tools.filter(tool => !tool.installed);
  const validChoice = choice === 'other' || ranked.some(tool => tool.id === choice);
  const lostTool = selectedTool && selectedTool.id !== 'other' && !selectedTool.installed ? selectedTool : null;
  const buttonTool = selectedTool && !lostTool ? t(aiToolShortName(selectedTool.id)) : 'AI';
  // The app windows' native action names the same tool.
  const titlebarTool = enabled && selectedTool && !lostTool && selectedTool.id !== 'other' ? buttonTool : null;
  const reportedTitlebarTool = useRef<string | null>(null);
  useEffect(() => {
    if (!enabled || reportedTitlebarTool.current === titlebarTool) return;
    reportedTitlebarTool.current = titlebarTool;
    void invoke('set_ai_titlebar_tool', { name: titlebarTool }).catch(() => { /* The action keeps its previous title. */ });
  }, [enabled, titlebarTool]);
  const guideResult = guide?.result ?? null;
  const guideSelected = guide ? t(aiToolShortName(guide.selectedId)) : '';
  const guideFailed = guideResult?.toolId === 'other' && guide?.selectedId !== 'other'
    && (guideResult.warningCode === 'TOOL_OPEN_FAILED' || guideResult.warningCode === 'TOOL_NOT_FOUND');
  const guideOther = guideResult?.toolId === 'other' && !guideFailed;
  const guideTool = guideResult && guideResult.toolId !== 'other' ? t(aiToolShortName(guideResult.toolId)) : '';
  const warningTemplate = aiHandoffWarning(guideResult?.warningCode);
  const guideWarning = !guideResult ? '' : guideResult.warning ? t(guideResult.warning)
    : warningTemplate ? t(warningTemplate, { tool: guideSelected }) : '';
  const guideTarget = guide?.target;
  const guideAfterSend = !guideTarget ? '' : guideTarget.purpose === 'repair' ? t('小抽屉不会替你发送。发送后，AI 会先查原因，再告诉你怎么改。')
    : guideTarget.sourcePath ? t('小抽屉不会替你发送。发送后，AI 会先检查，再告诉你结论。')
    : guideTarget.text !== undefined ? t('小抽屉不会替你发送。发送后，AI 会按应用给的提示词去做。')
    : guideTarget.purpose === 'create' ? t('小抽屉不会替你发送。发送后，AI 会先问你想做什么。')
    : t('小抽屉不会替你发送。发送后，AI 读懂应用，会问你想改哪里。');
  const guideOwnApp = Boolean(guideTarget && !guideTarget.sourcePath && (guideTarget.text === undefined || guideTarget.purpose === 'repair'));
  const guideDisplayPath = guideResult?.displayProjectRoot ?? guideResult?.projectRoot ?? '';
  const guideFolderName = guideDisplayPath.split(/[\\/]/).filter(Boolean).pop() ?? '';
  const value: AiToolsContextValue = { enabled, selectedTool, selectedToolName: selectedTool ? t(selectedTool.name) : null, buttonTool,
    lostToolName: lostTool ? t(lostTool.name) : null, tools, detecting, busy, notice, openChooser, selectTool, detectTools, handoff };
  return <AiToolsContext.Provider value={value}>
    {children}
    {enabled && notice && <div className="ai-tool-notice" role="status"><span>{t(notice)}</span><button type="button" className="text-button" aria-label={t('关闭提示')} onClick={() => setNotice('')}><X size={15} /></button></div>}
    {enabled && chooserOpen && <Modal title={t('你平时用哪个 AI 工具？')} className="ai-tool-chooser" onClose={closeChooser}>
      <p className="ai-tool-lead">{t('熊猫小抽屉不自带 AI。选一个你平时用来改应用的工具，以后点「告诉 ……」就会为它准备提示词。随时可以在设置里换。')}</p>
      <fieldset className="ai-tool-options" disabled={busy}>
        <legend>{detecting ? t('正在检测这台电脑上的 AI 工具…') : ranked.length ? t('检测到 {count} 个 AI 工具', { count: ranked.length }) : t('这台电脑上还没找到支持的 AI 工具。装好一个后点「重新检测」。')}</legend>
        {ranked.map(tool => <label key={tool.id} className={`ai-tool-option${choice === tool.id ? ' is-selected' : ''}`}>
          <input type="radio" name={`${id}-tool`} value={tool.id} checked={choice === tool.id} onChange={() => { choiceTouched.current = true; setChoice(tool.id); }} />
          <span className="ai-tool-option-copy"><strong>{t(tool.name)}{tool.id === recommendedId && <span className="ai-tool-recommended">{t('推荐')}</span>}</strong><span>{t(aiToolCapabilityText(tool.capability))}{tool.id === 'claude' && tool.capability === 'full' && <span className="ai-tool-setup-hint">{t('每次都会请你确认是否信任这个文件夹。')}</span>}</span></span>
          <span className={`ai-tool-tier${tool.capability === 'full' ? ' is-one-click' : ''}`}>{t(aiToolTierText(tool.capability))}</span>
        </label>)}
        <label className={`ai-tool-option${choice === 'other' ? ' is-selected' : ''}`}><input type="radio" name={`${id}-tool`} checked={choice === 'other'} onChange={() => { choiceTouched.current = true; setChoice('other'); }} /><span className="ai-tool-option-copy"><strong>{t(aiToolName('other'))}</strong><span>{t(platform.aiOtherToolDescription)}</span></span></label>
      </fieldset>
      <div className="ai-tool-detection"><p>{missing.length > 0 && t(ranked.length ? '没检测到：{tools}' : '支持的工具：{tools}', { tools: missing.map(tool => t(tool.name)).join(t('、')) })}</p><button type="button" className="refresh-button" disabled={detecting || busy} onClick={() => void detectTools()}><RefreshCw size={14} />{t('重新检测')}</button></div>
      {detectionError && <p className="project-help" role="status">{t('暂时无法完成检测，可以重试，或先选“其他 / 我还不确定”。')}</p>}
      <p className="ai-tool-browser-hint">{t('网页版 AI（豆包、ChatGPT 网页版等）改不了电脑上的文件，不能用来改应用。')}</p>
      <div className="ai-tool-footer modal-footer"><button type="button" className="refresh-button" onClick={closeChooser}>{t('以后再说')}</button><button type="button" className="primary-button" disabled={!validChoice || busy} onClick={confirmChoice}>{choice === 'other' ? t('保存选择') : t('保存，以后都用 {tool}', { tool: t(aiToolName(choice)) })}</button></div>
    </Modal>}
    {enabled && guide && !guideResult && <Modal title={t('正在启动 {tool}', { tool: guideSelected })} className="ai-tool-guide" onClose={closeGuide}>
      <p className="ai-tool-lead" role="status">{t('启动好之前先别在里面输入。启动好以后，这里会告诉你还剩哪几步。')}</p>
    </Modal>}
    {enabled && guide && guideResult && <Modal title={guideOther ? t('在你的 AI 工具里打开 {app}', { app: guideResult.appName }) : guideFailed ? t('没能打开 {tool}，按下面的步骤手动做', { tool: guideSelected }) : t('正在 {tool} 中打开 {app}', { tool: guideTool, app: guideResult.appName })} className="ai-tool-guide" onClose={closeGuide}>
      {(!guide.copied || guideWarning) && <p className="ai-tool-lead" role="status">{!guide.copied && (guideResult.prefilled
        ? t('提示词未复制到剪贴板。如果草稿是空的，可以重试复制或查看完整提示词。')
        : t('无法自动复制，请选中下方完整提示词手动复制。'))}{guideWarning && ` ${guideWarning}`}</p>}
      <section className="ai-tool-guide-group"><h3>{t('小抽屉已做')}</h3><ul>
        {guideResult.opened && <li>{guideTool ? t('打开了 {tool}', { tool: guideTool }) : t('打开了应用文件夹')}</li>}
        {guideResult.prefilled && guideTool && <li>{t('已请求 {tool} 填入提示词', { tool: guideTool })}</li>}
        {guide.copied && <li>{t('复制了提示词')}</li>}
      </ul></section>
      <section className="ai-tool-guide-group"><h3>{t('你来做')}</h3>
        <ol className="ai-tool-guide-steps">{remainingSteps(guideResult, guideTool).map(step => <li key={step}>{step}</li>)}</ol>
        <p>{guideAfterSend}</p>
        {(!guideResult.opened || guideResult.capability === 'launch' || guideResult.capability === 'manual') && <p>{t('AI 读写不了这个应用的文件夹时，会告诉你在它那里怎么打开。')}</p>}
        {guideResult.toolId === 'other' && <p>{t('网页版 AI 改不了电脑上的文件，要用装在电脑上的 AI 工具。')}</p>}
      </section>
      <dl className="ai-tool-path"><dt>{t('应用文件夹')}</dt><dd><code>{guideDisplayPath}</code>{guideFolderName && <span className="ai-tool-path-name">{t('文件夹名：{name}', { name: guideFolderName })}</span>}</dd></dl>
      <div className="ai-tool-path-actions"><button type="button" className="text-button" onClick={() => void copyPath(guideDisplayPath)}><Copy size={14} />{t('复制路径')}</button><button type="button" className="text-button" onClick={() => void invoke('open_project_folder', { path: guideResult.projectRoot }).catch(() => setPathNote('open-failed'))}><FolderOpen size={14} />{t(platform.openFolderLabel)}</button></div>
      {pathNote && <p className="project-help" role="status">{pathNote === 'copied' ? t('路径已复制。要粘贴提示词时，先点「再复制一次提示词」。') : pathNote === 'copy-failed' ? t('路径没能复制，请手动选中上面的路径。') : t('应用文件夹没能打开，请按上面的路径手动打开。')}</p>}
      {guideOwnApp && <p className="project-help">{t('AI 改完会告诉你。点应用窗口上的「重新加载」看效果；还想改或改坏了，就在同一个对话里接着说。')}</p>}
      {!showText && <button className="text-button" type="button" onClick={() => setShowText(true)}>{t('查看完整提示词')}</button>}
      {showText && <div className="ai-tool-manual-copy"><label htmlFor={`${id}-manual`}>{t('完整提示词 · 可手动复制')}</label><textarea id={`${id}-manual`} value={guideResult.text} readOnly rows={9} onFocus={event => event.currentTarget.select()} /></div>}
      <div className="ai-tool-footer modal-footer">{!needsManualCopyOrOpen(guideResult, guide.copied) && <label className="ai-tool-hide-guide"><input type="checkbox" checked={hideGuide} onChange={event => setHideGuide(event.target.checked)} />{t('下次不再显示')}</label>}<button type="button" className="refresh-button" disabled={copying} onClick={() => void copyAgain()}><Copy size={14} />{copying ? t('正在复制…') : t('再复制一次提示词')}</button><button type="button" className="primary-button" onClick={closeGuide}>{t('知道了')}</button></div>
    </Modal>}
  </AiToolsContext.Provider>;
}

/** What is left for the user after one click, in the tool's own words (already translated). */
function remainingSteps(result: AiHandoffResult, tool: string): string[] {
  if (result.toolId === 'other' || !result.opened) return [t('打开你的 AI 工具，新建对话。'), t('粘贴提示词，自己点发送。')];
  // Qoder asks what the AI may access; full access lets it edit the app folder without a workspace.
  const qoder = result.toolId === 'qoder' ? [t('Qoder 问给 AI 什么权限时，选「完全访问」。')] : [];
  switch (result.capability) {
    case 'full': return [...(result.toolId === 'claude' ? [t('Claude 会先问你是否信任这个文件夹，选信任。')] : []),
      result.prefilled ? t('看一眼提示词，自己点发送。') : t('粘贴提示词，自己点发送。')];
    case 'folder-prefill': return [...qoder, t('输入框是空的就粘贴；看一眼，自己点发送。')];
    default: return [...qoder, t('在 {tool} 里新建对话，粘贴提示词，自己点发送。', { tool })];
  }
}

/** How the tool was found, for the settings line (Chinese source text). */
export function aiToolDetectionText(detectedBy: string[]): string {
  const kinds = new Set(detectedBy.map(way => way.startsWith('command:') ? '命令行' : '桌面应用'));
  return [...kinds].map(kind => t(kind)).join(t('、'));
}

export function AiToolSettings() {
  const ai = useAiTools();
  const platform = useFrontendPlatform();
  const id = useId();
  if (!ai.enabled) return null;
  const selected = ai.selectedTool;
  // A saved tool that is no longer found stays visible so the choice is not silently lost.
  const lost = selected && selected.id !== 'other' && !selected.installed ? selected : null;
  const choices: AiTool[] = [...(lost ? [lost] : []), ...byCapability(ai.tools),
    { id: 'other', name: aiToolName('other'), installed: true, oneClick: false, capability: 'manual', detectedBy: [] }];
  const found = ai.tools.some(tool => tool.installed);
  const description = !selected ? t('选一个你平时用来修改电脑上文件的 AI 工具。')
    : selected.id === 'other' ? t(platform.aiOtherToolDescription)
    : lost ? t('这台电脑上没检测到 {tool}。装好后点「重新检测」，或换一个工具。', { tool: t(lost.name) })
    : t(aiToolCapabilityText(selected.capability));
  const basis = selected && !lost && selected.id !== 'other' && selected.detectedBy.length > 0
    ? t('检测依据：{kinds}', { kinds: aiToolDetectionText(selected.detectedBy) }) : '';
  return <div className="settings-row ai-tool-settings">
    <div className="ai-tool-settings-copy"><strong id={`${id}-label`}>{t('我的 AI 工具')}</strong>
      <p>{ai.detecting && !found ? t('正在检测这台电脑上的 AI 工具…') : !found && !lost ? t('没检测到支持的 AI 工具，装好后点「重新检测」。') : t('只列出这台电脑上检测到的工具。')}</p></div>
    <button type="button" className="refresh-button" disabled={ai.detecting || ai.busy} onClick={() => void ai.detectTools()}><RefreshCw size={14} />{t(ai.detecting ? '检测中…' : '重新检测')}</button>
    <div className="ai-tool-pills" role="radiogroup" aria-labelledby={`${id}-label`}>
      {choices.map(tool => <button key={tool.id} type="button" role="radio" aria-checked={selected?.id === tool.id} className={`ai-tool-pill${selected?.id === tool.id ? ' is-selected' : ''}${tool === lost ? ' is-lost' : ''}`} disabled={ai.busy} onClick={() => ai.selectTool(tool.id)}>
        {selected?.id === tool.id && <Check size={13} aria-hidden="true" />}{t(tool.name)}{tool.id !== 'other' && tool !== lost && <span className="ai-tool-pill-badge">{t(aiToolTierText(tool.capability))}</span>}
      </button>)}
    </div>
    <p className="ai-tool-settings-detail">{description}{basis && <span className="ai-tool-settings-basis">{basis}</span>}</p>
  </div>;
}
