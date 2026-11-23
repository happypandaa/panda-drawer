import { t } from './i18n';
import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { bytesText, partBytes, timeText } from './storage-utils';
import { messageFrom } from './ui-utils';
import { PlatformUpdateSettings } from './PlatformUpdates';
import type { PlatformUpdates } from './PlatformUpdates';
import type { StoragePart } from './types';
import { Check, FolderOpen, Info, ShieldCheck, X } from 'lucide-react';
import type { BootstrapInfo } from './types';
import { useFrontendPlatform } from './frontend-platform';
export function AboutPlatform({ info, updates }: { info: BootstrapInfo | null; updates: PlatformUpdates }) {
  const [program, setProgram] = useState<StoragePart | null>(null); const [busy, setBusy] = useState(false); const [error, setError] = useState('');
  async function scanProgram() { if (busy) return; setBusy(true); setError(''); try { const value = await invoke<{ platformProgram: StoragePart }>('workspace_storage'); setProgram(value.platformProgram); } catch (cause) { setError(messageFrom(cause)); } finally { setBusy(false); } }
  const platform = info?.platform;
  return <section className="diagnostics-card about-platform"><div className="section-heading"><div><h2><Info size={18} />{t("关于平台")}</h2><p>{t("以下信息来自当前运行的程序。平台程序的大小与应用数据分开统计。")}</p></div></div><dl className="about-grid">{[[t("名称"), platform?.name ?? info?.displayName], [t("版本"), platform?.version ?? info?.version], [t("构建"), platform?.buildNumber ?? platform?.build], [t("架构"), platform?.architecture], [t("应用标识"), platform?.identifier ?? info?.identifier], [t("安装位置"), platform?.installationPath ?? t("未从应用包启动")], [t("可执行文件"), platform?.executablePath], [t("发行来源"), platform?.distribution]].map(([label, value]) => <div key={String(label)}><dt>{String(label)}</dt><dd><code>{typeof value === 'string' || typeof value === 'number' ? value : t("尚未读取")}</code></dd></div>)}</dl><div className="platform-update"><strong>{t("当前安装程序体积")}</strong><p>{bytesText(partBytes(program ?? undefined))}{program ? program.complete ? t(" · 扫描完整") : t(" · 部分统计 / 未知") : ''} {t("· 扫描时间：")}{timeText(program?.completedAt as string | number | undefined)}</p><button className="refresh-button" disabled={!info || busy} onClick={() => void scanProgram()}>{busy ? t("统计中…") : t("统计当前程序体积")}</button>{error && <p role="alert">{t(error)}</p>}</div><PlatformUpdateSettings updates={updates} /></section>;
}
const GUIDE_KEY = 'local-projects:dev:onboarding:v1';
export function FirstStartGuide({ enabled, onCreate, onEnvironment, onStorage }: { enabled: boolean; onCreate: () => void; onEnvironment: () => void; onStorage: () => void }) {
  const frontendPlatform = useFrontendPlatform();
  const [show, setShow] = useState(() => { try { return localStorage.getItem(GUIDE_KEY) !== 'complete'; } catch { return true; } });
  const complete = () => { try { localStorage.setItem(GUIDE_KEY, 'complete'); } catch { /* The guide can still be dismissed for this session. */ } setShow(false); };
  if (!show) return <button className="text-button guide-reopen" onClick={() => setShow(true)}><Info size={13} />{t("查看开始使用指南")}</button>;
  return <section className="first-start-guide" aria-label={t("开始使用指南")}><div className="storage-heading"><div><span className="eyebrow">{t("开始使用")}</span><h2>{t("从一个自己的应用开始")}</h2><p>{t("应用在独立窗口中打开，数据保存到当前存放位置。")}</p></div><button className="icon-button" aria-label={t("暂时收起指南")} onClick={() => setShow(false)}><X size={17} /></button></div><div className="guide-steps"><div><span>01</span><h3>{t("创建与打开")}</h3><p>{t("新应用可以直接打开。每个应用拥有独立数据库和素材目录。")}</p><button className="text-button" disabled={!enabled} onClick={onCreate}><FolderOpen size={13} />{t("创建第一个应用")}</button></div><div><span>02</span><h3>{t("告诉 AI 怎么改")}</h3><p>{t("在应用详情点击“复制提示词”，粘贴给其他 AI 并补充你的需求。修改前按说明保存 Git 版本。改动源码后通过平台构建验证，再处理草稿并明确重新加载。")}</p><button className="text-button" onClick={onEnvironment}>{t("检查 Node 构建环境")}</button></div><div><span>03</span><h3>{t("保留与管理数据")}</h3><p>{t("应用概览中可修改名称和图标、扫描空间及浏览素材。删除应用会永久清除这个应用的文件夹，请先核对并保存需要的内容。")}</p><button className="text-button" onClick={onStorage}>{t("存储与清理")}</button></div></div><div className="guide-footer"><span><ShieldCheck size={14} />{frontendPlatform.developmentBuild ? t("当前为独立开发环境") : t("应用数据保存在本机。")}</span><button className="refresh-button" onClick={complete}><Check size={14} />{t("已了解，完成引导")}</button></div></section>;
}
