export const AI_TOOLS = [
  { id: 'workbuddy', name: 'WorkBuddy' },
  { id: 'codex', name: 'Codex' },
  { id: 'claude-code', name: 'Claude Code' },
  { id: 'claude', name: 'Claude' },
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
export interface AiTool {
  id: AiToolId;
  name: string;
  installed: boolean;
  detectedBy: string[];
  oneClick: boolean;
  setupHint?: string;
}
export interface AiHandoffTarget { projectId?: string; appId?: string; appName: string }
export interface AiHandoffResult {
  toolId: AiToolId;
  toolName: string;
  oneClick: boolean;
  opened: boolean;
  text: string;
  projectRoot: string;
  instanceId: string;
  appName: string;
  warning?: string;
}
export interface AiToolPreferences {
  acknowledged: boolean;
  selectedToolId: AiToolId | null;
  hiddenGuides: AiToolId[];
}
export const AI_TOOL_PREFERENCES_KEY = 'panda-drawer:ai-tools:v1';
export const isAiToolId = (value: unknown): value is AiToolId => value === 'other' || AI_TOOLS.some(tool => tool.id === value);
export const aiToolName = (id: AiToolId) => id === 'other' ? '其他 / 我还不确定' : AI_TOOLS.find(tool => tool.id === id)!.name;

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
      setupHint: typeof item?.setupHint === 'string' ? item.setupHint : undefined,
    };
  });
}

export function checkedAiHandoffTarget(value: unknown): AiHandoffTarget | null {
  if (!value || typeof value !== 'object') return null;
  const target = value as Record<string, unknown>;
  if (typeof target.appName !== 'string' || !target.appName) return null;
  const projectId = typeof target.projectId === 'string' && target.projectId ? target.projectId : undefined;
  const appId = typeof target.appId === 'string' && target.appId ? target.appId : undefined;
  return Boolean(projectId) !== Boolean(appId) ? { projectId, appId, appName: target.appName } : null;
}

export function checkedAiHandoffResult(value: unknown): AiHandoffResult {
  if (!value || typeof value !== 'object') throw new Error('交接说明暂时无法读取。');
  const result = value as AiHandoffResult;
  if (!isAiToolId(result.toolId) || typeof result.toolName !== 'string' || typeof result.oneClick !== 'boolean'
    || typeof result.opened !== 'boolean' || typeof result.text !== 'string' || !result.text
    || typeof result.projectRoot !== 'string' || !result.projectRoot || typeof result.instanceId !== 'string' || !result.instanceId
    || typeof result.appName !== 'string') throw new Error('交接说明暂时无法读取。');
  return result;
}
