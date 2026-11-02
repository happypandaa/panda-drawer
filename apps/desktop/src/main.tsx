import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import { initializeLocale } from './i18n';
import './styles.css';

void initializeLocale().then(() => ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
));
