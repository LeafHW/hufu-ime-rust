import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';

if (new URLSearchParams(location.search).get('shell') === '1') {
  document.body.classList.add('webview');
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
