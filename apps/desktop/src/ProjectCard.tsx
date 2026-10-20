import { ArrowUpRight, FolderOpen, Globe, Info, Server, Trash2 } from 'lucide-react';
import { bytesText } from './storage-utils';
import type { MaintenanceStatus, ProjectInfo, TaskRecord } from './types';
import type { NodeAppDetail } from './node-app-types';

const labels: Record<ProjectInfo['state'], string> = {
  ready: '可进入', unbuilt: '等待构建', missing: '文件缺失', error: '需要检查', maintenance: '维护中', recoveryRequired: '恢复待处理',
};

export function ProjectCard({ project, status, task, blocked, opening, deletingDisabled, detailsDisabled = false, onEnter, onDetails, onDelete }: {
  project: ProjectInfo;
  status?: MaintenanceStatus;
  task?: TaskRecord;
  blocked: boolean;
  opening: boolean;
  deletingDisabled: boolean;
  detailsDisabled?: boolean;
  onEnter: () => void;
  onDetails: () => void;
  onDelete: () => void;
}) {
  const state = status?.recoveryRequired ? 'recoveryRequired' : status?.mode === 'maintenance' ? 'maintenance' : project.state;
  const busy = task && !task.final;
  const size = project.storage?.currentProject;
  return <article className="project-card" aria-label={project.name}>
    <div className="project-card-top">
      <span className="project-icon" aria-hidden="true">{project.iconDataUrl ? <img src={project.iconDataUrl} alt="" /> : <FolderOpen size={27} strokeWidth={1.6} />}</span>
      <span className={`project-state state-${state}`}><span />{busy ? '任务进行中' : labels[state]}</span>
    </div>
    <div className="project-card-identity"><h2 title={project.name}>{project.name}</h2><p className="project-description">{project.description || '为这个项目添加一段简介，让它更容易找到。'}</p></div>
    <div className="project-card-meta"><span className="node-runtime-label"><Globe size={13} />网页项目</span><span>{size?.totalBytes != null ? `${bytesText(size.totalBytes)}${size.stale ? ' · 待扫描' : size.complete === false ? ' · 部分统计' : ''}` : '空间尚未统计'}</span><button className="project-delete" type="button" disabled={deletingDisabled} onClick={onDelete} aria-haspopup="dialog" aria-label={`删除项目「${project.name}」`}><Trash2 size={14} />删除项目</button></div>
    <div className="project-actions">
      <button className="primary-button" disabled={blocked || project.state !== 'ready' || opening} onClick={onEnter} aria-label={`进入项目「${project.name}」`} title={project.state !== 'ready' ? '请先在详情中检查状态或构建项目' : blocked ? '请先处理项目维护或等待工作区任务完成' : undefined}>{opening ? '进入中…' : '进入项目'}<ArrowUpRight size={16} /></button>
      <button className="refresh-button" disabled={detailsDisabled} onClick={onDetails} aria-haspopup="dialog" aria-label={`查看项目「${project.name}」的详情`}><Info size={16} />详情</button>
    </div>
  </article>;
}

export function NodeProjectCard({ app, blocked, opening, onEnter, onDetails }: {
  app: NodeAppDetail; blocked: boolean; opening: boolean; onEnter: () => void; onDetails: () => void;
}) {
  const needsConfiguration = !app.configuration.canStart;
  const label = app.state === 'running' ? '运行中' : app.state === 'starting' ? '打开中' : app.state === 'stopping' ? '关闭中' : app.state === 'failed' ? '需要检查' : needsConfiguration ? '需要配置' : '可进入';
  return <article className="project-card" aria-label={app.name}>
    <div className="project-card-top"><span className="project-icon" aria-hidden="true"><Server size={27} strokeWidth={1.6} /></span><span className={`project-state state-${app.state === 'failed' ? 'error' : needsConfiguration ? 'unbuilt' : 'ready'}`}><span />{label}</span></div>
    <div className="project-card-identity"><h2 title={app.name}>{app.name}</h2><p className="project-description">{app.lastError ?? (needsConfiguration ? '进入前请先完成应用配置。密钥与服务地址可在详情中填写。' : '在独立应用窗口中使用，关闭应用窗口会停止相关服务。')}</p></div>
    <div className="project-card-meta"><span className="node-runtime-label"><Server size={13} />Node 服务项目</span><span>{app.restartRequired ? '已保存配置 · 待重启' : needsConfiguration ? '需要配置' : '配置完整'}</span></div>
    <div className="project-actions"><button className="primary-button" disabled={blocked || opening} onClick={onEnter} aria-label={`进入项目「${app.name}」`}>{opening ? '进入中…' : '进入项目'}<ArrowUpRight size={16} /></button><button className="refresh-button" disabled={opening} onClick={onDetails} aria-haspopup="dialog" aria-label={`查看项目「${app.name}」的详情`}><Info size={16} />详情</button></div>
  </article>;
}
