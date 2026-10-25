// 语言 Provider：持有当前语言、写入 <html lang>、通过 context 向下分发。
//
// 为什么把 `t` 固定成「当前语言 + 固定引用」的形式：组件里常把 t 放进
// useMemo/useEffect 的依赖数组。如果每次渲染都新建一个闭包，所有依赖 t 的
// 计算都会失效重算。所以 t 本身随 language 变化，但在同一语言下保持稳定。

import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import {
  I18nContext,
  LANGUAGE_STORAGE_KEY,
  LOCALES,
  readLanguagePreference,
  resolveLanguage,
  translate,
  type I18nValue,
  type LanguagePreference,
  type TranslateOptions,
} from './index';
import type { MessageKey } from './messages/zh-CN';

export function I18nProvider({
  children,
  initialPreference,
}: {
  children: ReactNode;
  /** 仅用于服务端渲染与测试：跳过 localStorage，直接指定初始偏好。 */
  initialPreference?: LanguagePreference;
}) {
  const [preference, setPreferenceState] = useState<LanguagePreference>(
    initialPreference ?? readLanguagePreference,
  );
  const [systemLanguage, setSystemLanguage] = useState(() => resolveLanguage('system'));

  const language = preference === 'system' ? systemLanguage : preference;

  // 跟随系统时，浏览器语言或系统主题变化都要重算。
  useEffect(() => {
    if (preference !== 'system') return;
    const update = () => setSystemLanguage(resolveLanguage('system'));
    window.addEventListener('languagechange', update);
    return () => window.removeEventListener('languagechange', update);
  }, [preference]);

  // <html lang> 影响字体回退、断词与读屏发音，必须跟着语言走。
  useEffect(() => {
    document.documentElement.lang = LOCALES[language];
    document.documentElement.setAttribute('data-language', language);
  }, [language]);

  const setLanguage = useCallback((next: LanguagePreference) => {
    setPreferenceState(next);
    try {
      if (next === 'system') localStorage.removeItem(LANGUAGE_STORAGE_KEY);
      else localStorage.setItem(LANGUAGE_STORAGE_KEY, next);
    } catch {
      // 存储被禁用时只在本次会话生效，不影响使用。
    }
  }, []);

  const t = useCallback(
    (key: MessageKey, options?: TranslateOptions) => translate(language, key, options),
    [language],
  );

  const value = useMemo<I18nValue>(
    () => ({ language, preference, t, setLanguage }),
    [language, preference, t, setLanguage],
  );

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}
