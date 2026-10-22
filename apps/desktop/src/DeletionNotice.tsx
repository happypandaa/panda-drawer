export function DeletionNotice() {
  return <div className="project-delete-git-warning" role="note" aria-label="删除前的版本与同步提醒">
    <strong>未同步的内容请先另存。</strong>
    <p>平台尚不支持远程 Git 同步，目录内的本机 Git 记录也会一起移走。</p>
  </div>;
}
