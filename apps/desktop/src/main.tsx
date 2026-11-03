import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import { initializeLocale } from './i18n';
import './styles.css';
import './AiTools.css';
import './app-library.css';

void initializeLocale().then(() => ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
));
