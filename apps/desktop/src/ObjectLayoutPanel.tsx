import { t } from './i18n';
import type { ObjectLayout } from './object-layout-types';

export function ObjectLayoutPanel({ layout }: { layout?: ObjectLayout | null }) {
  return <details className="object-layout-panel">
    <summary>{t("文件位置")}</summary>
    {!layout ? <p>{t("尚未读取文件位置。")}</p> : <>
      <dl className="object-layout-paths">
        <div><dt>{t("源码目录")}</dt><dd>{layout.source.path ? <code>{layout.source.path}</code> : t("路径不可用")}<p>{({ available: t("源码目录存在"), missing: t("源码缺失"), unverified: t("源码未核实") })[layout.source.state]}{t("。")}{t(layout.source.reason)}</p></dd></div>
        <div><dt>{t("当前网页运行目录")}</dt><dd>{layout.runtime.webRoot ? <code>{layout.runtime.webRoot}</code> : t("未声明或不可用")}</dd></div>
        <div><dt>{t("当前后端运行目录")}</dt><dd>{layout.runtime.backendRoot ? <code>{layout.runtime.backendRoot}</code> : t("未声明或不可用")}</dd></div>
        <div><dt>{t("业务数据目录")}</dt><dd>{layout.data.root ? <code>{layout.data.root}</code> : t("路径未能核验")}</dd></div>
        <div><dt>{t("当前数据库")}</dt><dd>{layout.data.database ? <code>{layout.data.database}</code> : t("未声明或路径未能核验")}</dd></div>
        <div><dt>{t("附件目录")}</dt><dd>{layout.data.attachments ? <code>{layout.data.attachments}</code> : t("未声明或路径未能核验")}</dd></div>
        <div><dt>{t("应用配置文件")}</dt><dd>{layout.configurationPath ? <code>{layout.configurationPath}</code> : t("未提供或路径未能核验")}</dd></div>
        <div><dt>{t("平台控制目录")}</dt><dd>{layout.controlPath ? <code>{layout.controlPath}</code> : t("路径未能核验")}</dd></div>
      </dl>
      {layout.runtime.external && <p>{t("当前运行文件来自登记的外部目录，仍依赖该目录；不代表已安装自包含成品包。")}</p>}
      <h4>{t("构建范围")}</h4>
      <p>{layout.build.state === 'platform-build' ? t("可使用平台构建入口") : t("平台构建不可用")}{t("。")}{t(layout.build.reason)}</p>
      <h4>{t("Git 与数据库范围")}</h4>
      <p>{t("本报告只检查目录标记，未验证源码仓库。")}{t(layout.versioning.reason)}</p>
      {layout.versioning.markers.length ? <><p>{t("以下仅为检测到的 .git 标记，尚未验证仓库、版本历史或工作区状态。")}</p><ul className="object-layout-markers">{layout.versioning.markers.map((marker, index) => <li key={`${marker.scope}:${marker.path}:${index}`}><code>{marker.path}</code><span>{({ project: t("项目目录"), source: t("源码目录"), ancestor: t("上级目录") })[marker.scope]} · {({ directory: t("目录标记"), file: t("文件标记"), unsafe: t("无法安全核实的标记") })[marker.kind]}</span></li>)}</ul></> : <p>{t("本次未发现可报告的 .git 标记；这不证明其他位置没有仓库或版本历史。")}</p>}
      {layout.versioning.inspectionIssues.map((issue, index) => <p className="environment-issue" key={index}>{t(issue)}</p>)}
      <p>{t(layout.versioning.database.reason)}</p>
      <p>{t("代码保存与回退由你和 AI 使用项目自己的 Git 完成；平台不备份业务数据。")}</p>
    </>}
  </details>;
}
