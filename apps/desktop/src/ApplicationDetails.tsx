import type { ReactNode } from 'react';

/** Shared content order for web and Node application details. */
export function ApplicationDetails({ actions, handoff, notices, information, more, deleteAction, children }: {
  actions?: ReactNode; handoff?: ReactNode; notices?: ReactNode; information?: ReactNode;
  more?: ReactNode; deleteAction?: ReactNode; children?: ReactNode;
}) {
  return <div className="application-details">
    {notices && <div className="application-detail-notices">{notices}</div>}
    {actions && <div className="detail-actions detail-action-row application-primary-actions">{actions}</div>}
    {handoff}
    {information && <dl className="application-detail-information">{information}</dl>}
    {children}
    {more}
    <footer className="application-detail-footer"><p>数据和密钥保存在本机，改代码不会动到它们</p>{deleteAction}</footer>
  </div>;
}

export function ApplicationDetailRow({ label, children, action }: { label: string; children: ReactNode; action?: ReactNode }) {
  return <div className="application-detail-row"><dt>{label}</dt><dd>{children}</dd>{action && <div className="application-detail-row-action">{action}</div>}</div>;
}
