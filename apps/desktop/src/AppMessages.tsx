import { useState } from 'react';
import { Bell, CircleAlert, Info, X } from 'lucide-react';
import { Modal } from './Modal';

export interface AppMessage {
  id: string;
  level: 'info' | 'warning' | 'error';
  title: string;
  message: string;
}

type AppMessagesProps = {
  messages: AppMessage[];
  onDismiss: (id: string) => void;
  onClear: () => void;
  disabled?: boolean;
};

const levelLabels = { info: '提示', warning: '提醒', error: '错误' };

export function AppMessages({ messages, onDismiss, onClear, disabled = false }: AppMessagesProps) {
  const [open, setOpen] = useState(false);

  return <>
    <button className="radio-messages-button" type="button" disabled={disabled} aria-label={`消息，${messages.length} 条`} aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen(true)}>
      <Bell size={17} aria-hidden="true" />
      <span className="radio-messages-label">消息</span>
      <span className="radio-messages-count" aria-hidden="true">{messages.length}</span>
    </button>
    <span className="sr-only" role="status">当前有 {messages.length} 条消息</span>
    {open && <Modal title="消息" className="radio-messages-modal" onClose={() => setOpen(false)}>
      {messages.length > 0 ? <>
        <div className="app-messages-toolbar">
          <span>{messages.length} 条消息</span>
          <button className="text-button" type="button" onClick={onClear}>全部清除</button>
        </div>
        <ul className="app-messages-list" aria-label="操作消息">
          {messages.map((message) => <li key={message.id} className={`app-message is-${message.level}`}>
            <span className="app-message-icon" aria-hidden="true">{message.level === 'info' ? <Info size={18} /> : <CircleAlert size={18} />}</span>
            <div className="app-message-content">
              <h3><span className="sr-only">{levelLabels[message.level]}：</span>{message.title}</h3>
              <p>{message.message}</p>
            </div>
            <button className="icon-button app-message-dismiss" type="button" aria-label={`清除消息：${message.title}`} onClick={() => onDismiss(message.id)}><X size={15} aria-hidden="true" /></button>
          </li>)}
        </ul>
      </> : <div className="app-messages-empty"><Bell size={26} aria-hidden="true" /><p>暂无消息</p></div>}
    </Modal>}
  </>;
}
