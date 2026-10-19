import React from 'react';
import ReactDOM from 'react-dom/client';
import { WindowsHost } from './WindowsHost';
import './styles.css';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <WindowsHost />
  </React.StrictMode>,
);
