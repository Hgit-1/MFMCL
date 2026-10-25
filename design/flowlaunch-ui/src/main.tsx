import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import { applyAccent } from '../utils/accent';
import { I18nProvider } from '../utils/i18n/I18nProvider';
import { readLanguagePreference, resolveLanguage, LOCALES } from '../utils/i18n';

// Apply the saved accent before the first paint so opening Settings never
// causes a visible theme/accent jump (the Settings panel previews the same value).
try {
  const saved = JSON.parse(localStorage.getItem('flowlaunch-settings') || '{}') as { customAccent?: string };
  if (saved.customAccent) applyAccent(saved.customAccent);
} catch {
  // Ignore invalid persisted settings; defaults remain in place.
}

// 语言要在首帧前定好，否则中文文案会先闪一下再被替换。
document.documentElement.lang = LOCALES[resolveLanguage(readLanguagePreference())];

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <I18nProvider>
      <App />
    </I18nProvider>
  </React.StrictMode>
);
