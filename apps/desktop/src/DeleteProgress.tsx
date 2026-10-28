import { LoaderCircle } from 'lucide-react';
import { useLayoutEffect, useRef } from 'react';

export function DeleteProgress({ name, checking = false, detail }: { name: string; checking?: boolean; detail?: string }) {
  const heading = useRef<HTMLHeadingElement>(null);
  useLayoutEffect(() => {
    const element = heading.current;
    const dialogHeading = element?.closest('[role="dialog"]')?.querySelector<HTMLElement>('.modal-header h2');
    element?.focus();
    return () => { if (element === document.activeElement) dialogHeading?.focus(); };
  }, []);
  return <div className="delete-progress" role="status" aria-live="polite" aria-atomic="true">
    <LoaderCircle className="delete-progress-spinner" size={32} aria-hidden="true" />
    <h3 ref={heading} tabIndex={-1}>{checking ? '正在核对本项目的删除范围…' : '正在永久删除…'}</h3>
    <p className="delete-progress-name">{name}</p>
    {detail && <p className="delete-progress-stage">{detail}</p>}
    <p className="delete-progress-help">{checking ? '正在读取需要删除和保留的内容，请稍候。' : '无需再次点击。文件较多时可能需要一点时间，请保持软件开启；完成后会自动更新项目列表。'}</p>
  </div>;
}
