export const AI_TOOLS = [
  { id: 'workbuddy', name: 'WorkBuddy' },
  { id: 'codex', name: 'ChatGPT（Codex）' },
  { id: 'claude', name: 'Claude 桌面版' },
  { id: 'cursor', name: 'Cursor' },
  { id: 'qoder', name: 'Qoder' },
  { id: 'vscode', name: 'VS Code' },
  { id: 'trae', name: 'Trae' },
  { id: 'trae-cn', name: 'Trae CN' },
  { id: 'windsurf', name: 'Windsurf' },
  { id: 'kiro', name: 'Kiro' },
  { id: 'codebuddy', name: 'CodeBuddy' },
  { id: 'codebuddy-cn', name: 'CodeBuddy CN' },
  { id: 'lingma', name: '通义灵码' },
] as const;

export type AiToolId = typeof AI_TOOLS[number]['id'] | 'other';
/**
 * What one click achieves, as reported by native detection:
 * full = folder + new conversation + filled prompt; folder-prefill = folder,
 * short prompts filled; folder = folder, paste; launch = start the tool, the
 * user chooses the folder and pastes; manual = file manager and clipboard.
 */
export type AiToolCapability = 'full' | 'folder-prefill' | 'folder' | 'launch' | 'manual';
export interface AiTool {
  id: AiToolId;
  name: string;
  installed: boolean;
  detectedBy: string[];
  oneClick: boolean;
  capability: AiToolCapability;
  /** A native setup-hint code (none are issued at present). */
  setupHint?: string;
}
/**
 * `purpose` and `text` are set only for platform-originated requests: create,
 * repair explanations built by the manager, and app pages. `entry` names the
 * button for the native handoff log.
 */
export interface AiHandoffTarget { projectId?: string; appId?: string; sourcePath?: string; appName: string; purpose?: 'modify' | 'create' | 'repair'; text?: string; expectedWindowGeneration?: number; entry?: string }
export interface AiHandoffResult {
  toolId: AiToolId;
  toolName: string;
  oneClick: boolean;
  opened: boolean;
  capability: AiToolCapability;
  prefilled: boolean;
  text: string;
  projectRoot: string;
  instanceId: string;
  appName: string;
  /** Native warning code, translated by `aiHandoffWarning`. */
  warningCode?: string;
  /** Already translated manager text, used only for manager-side failures. */
  warning?: string;
}
export interface AiToolPreferences {
  acknowledged: boolean;
  selectedToolId: AiToolId | null;
  hiddenGuides: AiToolId[];
}
export const AI_TOOL_PREFERENCES_KEY = 'panda-drawer:ai-tools:v1';
export const isAiToolId = (value: unknown): value is AiToolId => value === 'other' || AI_TOOLS.some(tool => tool.id === value);
/** Chinese source name; display it through t(). */
export const aiToolName = (id: AiToolId) => id === 'other' ? '其他 / 我还不确定' : AI_TOOLS.find(tool => tool.id === id)!.name;
const capabilities: readonly AiToolCapability[] = ['full', 'folder-prefill', 'folder', 'launch', 'manual'];
export const isAiToolCapability = (value: unknown): value is AiToolCapability => capabilities.includes(value as AiToolCapability);
/** What each tool calls an app folder, and a shorter name for buttons where the full name is long. */
const toolTerms: Partial<Record<AiToolId, { short?: string; folder: string }>> = {
  workbuddy: { folder: '分组' },
  codex: { short: 'Codex', folder: '项目' },
  claude: { short: 'Claude', folder: '项目' },
  trae: { folder: '项目' },
  'trae-cn': { folder: '项目' },
  qoder: { folder: '工作区' },
};
/** Name on buttons such as 「告诉 Codex 怎么改」; "AI" when no specific tool applies. Display it through t(). */
export const aiToolShortName = (id: AiToolId) => id === 'other' ? 'AI' : toolTerms[id]?.short ?? aiToolName(id);
/** The tool's own word for an app folder (Chinese source text); 「文件夹」 until confirmed for that tool. */
export const aiToolFolderTerm = (id: AiToolId) => toolTerms[id]?.folder ?? '文件夹';
/** Chinese source template ({tool}) for native warning codes. */
export function aiHandoffWarning(code: string | undefined): string | undefined {
  switch (code) {
    case 'TOOL_NOT_FOUND': return '没找到 {tool}，可以换一个工具。先按下面的步骤手动做。';
    case 'TOOL_OPEN_FAILED': return '没能打开 {tool}，按下面的步骤手动做。';
    case 'FOLDER_OPEN_FAILED': return '应用文件夹没能打开，请按下面的路径手动打开。';
    case 'PROMPT_NOT_FILLED': return '提示词太长，{tool} 不能自动填入。已经复制好了，粘贴即可。';
    default: return undefined;
  }
}

export function readAiToolPreferences(): AiToolPreferences {
  const fallback: AiToolPreferences = { acknowledged: false, selectedToolId: null, hiddenGuides: [] };
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(AI_TOOL_PREFERENCES_KEY) ?? 'null');
    if (!value || typeof value !== 'object') return fallback;
    const saved = value as Record<string, unknown>;
    const selectedToolId = isAiToolId(saved.selectedToolId) ? saved.selectedToolId : null;
    return {
      acknowledged: saved.acknowledged === true || selectedToolId !== null,
      selectedToolId,
      hiddenGuides: Array.isArray(saved.hiddenGuides) ? saved.hiddenGuides.filter(isAiToolId) : [],
    };
  } catch { return fallback; }
}

export function checkedAiTools(value: unknown): AiTool[] {
  if (!Array.isArray(value)) throw new Error('无法读取 AI 工具检测结果。');
  return AI_TOOLS.map(tool => {
    const item = value.find(candidate => candidate && typeof candidate === 'object' && candidate.id === tool.id);
    return {
      ...tool,
      installed: item?.installed === true,
      detectedBy: Array.isArray(item?.detectedBy) ? item.detectedBy.filter((entry: unknown): entry is string => typeof entry === 'string') : [],
      oneClick: item?.oneClick === true,
      capability: isAiToolCapability(item?.capability) ? item.capability : item?.oneClick === true ? 'full' : 'folder',
      setupHint: typeof item?.setupHint === 'string' ? item.setupHint : undefined,
    };
  });
}

/** `fromEvent`: native window events (titlebar, app pages) never carry manager purposes. */
export function checkedAiHandoffTarget(value: unknown, fromEvent = false): AiHandoffTarget | null {
  if (!value || typeof value !== 'object') return null;
  const target = value as Record<string, unknown>;
  if (typeof target.appName !== 'string' || !target.appName) return null;
  const projectId = typeof target.projectId === 'string' && target.projectId ? target.projectId : undefined;
  const appId = typeof target.appId === 'string' && target.appId ? target.appId : undefined;
  const sourcePath = typeof target.sourcePath === 'string' && target.sourcePath && !target.sourcePath.includes('\0') ? target.sourcePath : undefined;
  if ([projectId, appId, sourcePath].filter(Boolean).length !== 1) return null;
  const generation = target.expectedWindowGeneration ?? target.windowGeneration;
  if (generation !== undefined && (typeof generation !== 'number' || !Number.isSafeInteger(generation) || generation < 0 || sourcePath
    || target.expectedWindowGeneration !== undefined && target.windowGeneration !== undefined && target.expectedWindowGeneration !== target.windowGeneration)) return null;
  if (fromEvent && (sourcePath || target.purpose !== undefined)) return null;
  const purpose = target.purpose === 'create' ? 'create' as const : target.purpose === 'repair' ? 'repair' as const : undefined;
  if (purpose === 'repair' && (sourcePath || typeof target.text !== 'string')) return null;
  const limit = sourcePath || purpose === 'repair' ? 20000 : 2000;
  const text = typeof target.text === 'string' && target.text.trim() && [...target.text].length <= limit ? target.text : undefined;
  if (target.text !== undefined && !text) return null;
  const entry = typeof target.entry === 'string' && /^[a-z-]{1,32}$/.test(target.entry) ? target.entry : undefined;
  return { projectId, appId, ...(generation === undefined ? {} : {expectedWindowGeneration: generation as number}), ...(sourcePath ? {sourcePath} : {}), appName: target.appName, ...(purpose ? { purpose } : {}), ...(text ? { text } : {}), ...(entry ? { entry } : {}) };
}

export function checkedAiHandoffResult(value: unknown): AiHandoffResult {
  if (!value || typeof value !== 'object') throw new Error('提示词暂时无法读取。');
  const result = value as AiHandoffResult;
  if (!isAiToolId(result.toolId) || typeof result.toolName !== 'string' || typeof result.oneClick !== 'boolean'
    || typeof result.opened !== 'boolean' || typeof result.text !== 'string' || !result.text
    || typeof result.projectRoot !== 'string' || !result.projectRoot || typeof result.instanceId !== 'string' || !result.instanceId
    || typeof result.appName !== 'string') throw new Error('提示词暂时无法读取。');
  return {
    ...result,
    capability: isAiToolCapability(result.capability) ? result.capability : result.oneClick ? 'full' : 'manual',
    prefilled: result.prefilled === true,
    warningCode: typeof result.warningCode === 'string' ? result.warningCode : undefined,
  };
}
