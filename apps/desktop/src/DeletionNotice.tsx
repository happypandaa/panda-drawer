export function DeletionNotice() {
  return <div className="project-delete-git-warning" role="note" aria-label="删除前的版本与同步提醒">
    <strong>请先另存需要保留的内容，尤其是没有 Git 版本或未同步的源码。</strong>
    <p>平台已有本机源码版本功能，尚未提供远程推送、拉取或同步。本次没有核实远端备份；未提交或未推送的修改可能只有本机这一份。</p>
    <p>本机 Git 仓库若在下方目录内，也会一起移入系统废纸篓。保存过本机版本不等于已上传备份；数据库、附件和本机配置不能靠现有源码版本恢复。</p>
    <p>文件找回和永久清空交给 macOS 的 Finder。应用内不提供此次删除的恢复；找回文件后请重新核对登记状态，平台不会自动启动应用。</p>
  </div>;
}
