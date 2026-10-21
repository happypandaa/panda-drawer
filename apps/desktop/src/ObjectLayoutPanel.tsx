import type { ObjectLayout } from './object-layout-types';

export function ObjectLayoutPanel({ layout }: { layout?: ObjectLayout | null }) {
  return <details className="object-layout-panel">
    <summary>文件与版本范围</summary>
    {!layout ? <p>尚未读取或未能核验目录与版本范围，暂不能判断源码、构建和数据库纳管情况。</p> : <>
      <dl className="object-layout-paths">
        <div><dt>{layout.kind === 'local-node-source' ? '登记的外部应用目录' : '源码目录'}</dt><dd>{layout.source.path ? <code>{layout.source.path}</code> : '路径不可用'}<p>{({ available: '源码目录存在', missing: '源码缺失', unverified: '源码未核实' })[layout.source.state]}。{layout.source.reason}</p></dd></div>
        <div><dt>{layout.kind === 'project-v1' ? '声明的网页产物目录' : '当前网页运行目录'}</dt><dd>{layout.runtime.webRoot ? <code>{layout.runtime.webRoot}</code> : '未声明或不可用'}</dd></div>
        <div><dt>{layout.kind === 'project-v1' ? '声明的后端产物目录' : '当前后端运行目录'}</dt><dd>{layout.runtime.backendRoot ? <code>{layout.runtime.backendRoot}</code> : '未声明或不可用'}</dd></div>
        <div><dt>业务数据目录</dt><dd>{layout.data.root ? <code>{layout.data.root}</code> : '路径未能核验'}</dd></div>
        <div><dt>当前数据库</dt><dd>{layout.data.database ? <code>{layout.data.database}</code> : '未声明或路径未能核验'}</dd></div>
        <div><dt>附件目录</dt><dd>{layout.data.attachments ? <code>{layout.data.attachments}</code> : '未声明或路径未能核验'}</dd></div>
        <div><dt>应用配置文件</dt><dd>{layout.configurationPath ? <code>{layout.configurationPath}</code> : '未提供或路径未能核验'}</dd></div>
        <div><dt>平台控制目录</dt><dd>{layout.controlPath ? <code>{layout.controlPath}</code> : '路径未能核验'}</dd></div>
      </dl>
      {layout.kind === 'project-v1' && <p>平台管理实际发布代次；已打开窗口可能仍使用原代次。</p>}
      {layout.runtime.external && <p>当前运行文件来自登记的外部目录，仍依赖该目录；不代表已安装自包含成品包。</p>}
      <h4>构建范围</h4>
      <p>{layout.build.state === 'platform-build' ? '可使用平台构建入口' : '平台构建不可用'}。{layout.build.reason}</p>
      <h4>Git 与数据库范围</h4>
      <p>{layout.kind === 'project-v1' ? '源码版本操作请查看“源码版本”页；以下仅为目录标记检查。' : '此类应用尚未接入 Git 版本操作。'}{layout.versioning.reason}</p>
      {layout.versioning.markers.length ? <><p>以下仅为检测到的 .git 标记，尚未验证仓库、版本历史或工作区状态。</p><ul className="object-layout-markers">{layout.versioning.markers.map((marker, index) => <li key={`${marker.scope}:${marker.path}:${index}`}><code>{marker.path}</code><span>{({ project: '项目目录', source: '源码目录', ancestor: '上级目录' })[marker.scope]} · {({ directory: '目录标记', file: '文件标记', unsafe: '无法安全核实的标记' })[marker.kind]}</span></li>)}</ul></> : <p>本次未发现可报告的 .git 标记；这不证明其他位置没有仓库或版本历史。</p>}
      {layout.versioning.inspectionIssues.map((issue, index) => <p className="environment-issue" key={index}>{issue}</p>)}
      <p>数据库尚未纳入平台 Git 版本管理。{layout.versioning.database.reason}</p>
      <p>版本保存采用项目标准 Git，不另建独立备份体系。旧备份与未完成现场继续保留。</p>
    </>}
  </details>;
}
