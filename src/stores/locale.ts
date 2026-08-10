import { create } from 'zustand';
import type { Locale } from '@/i18n';

const LOCALE_KEY = 'locale';

/** 根据浏览器语言选择受支持语言。 */
export function detectLocale(languages: readonly string[] = navigator.languages): Locale {
  return languages.some((language) => language.toLowerCase().startsWith('zh')) ? 'zh-CN' : 'en-US';
}

interface LocaleState {
  locale: Locale;
  initLocale: (languages?: readonly string[]) => Locale;
  setLocale: (locale: Locale) => void;
}

/** 保存应用语言偏好；无偏好时跟随系统语言。 */
export const useLocaleStore = create<LocaleState>((set) => ({
  locale: detectLocale(),
  initLocale(languages) {
    let locale: Locale;
    try {
      const saved = localStorage.getItem(LOCALE_KEY);
      locale = saved === 'zh-CN' || saved === 'en-US' ? saved : detectLocale(languages);
    } catch {
      locale = detectLocale(languages);
    }
    set({ locale });
    return locale;
  },
  setLocale(locale) {
    set({ locale });
    try { localStorage.setItem(LOCALE_KEY, locale); } catch { /* 存储不可用时保留内存偏好。 */ }
  },
}));
