import { enUS } from './i18n/en-US';
import { zhCN } from './i18n/zh-CN';

/** 应用支持的界面语言。 */
export type Locale = 'zh-CN' | 'en-US';

/** 后端跨 Tauri 边界返回的稳定错误。 */
export interface AppErrorInfo {
  code: string;
  detail?: string | null;
}

export type TranslationKey = keyof typeof zhCN;
const dictionaries: Record<Locale, Record<TranslationKey, string>> = { 'zh-CN': zhCN, 'en-US': enUS };

/** 按语言读取文案并替换简单命名参数。 */
export function translate(locale: Locale, key: TranslationKey, params: Record<string, string | number> = {}): string {
  return dictionaries[locale][key].replace(/\{(\w+)\}/g, (_, name: string) => String(params[name] ?? `{${name}}`));
}

/** 格式化后端错误，保留诊断详情用于排障。 */
export function formatAppError(locale: Locale, error: AppErrorInfo | null | undefined): string {
  if (!error) return translate(locale, 'error.Unknown');
  const key = `error.${error.code}` as TranslationKey;
  const summary = key in dictionaries[locale] ? translate(locale, key) : translate(locale, 'error.Unknown');
  return error.detail?.trim() ? `${summary}: ${error.detail.trim()}` : summary;
}

/** 将 Tauri command rejection 规范为结构化错误。 */
export function toAppError(error: unknown): AppErrorInfo {
  if (error && typeof error === 'object' && 'code' in error) {
    const value = error as { code: unknown; detail?: unknown };
    return { code: typeof value.code === 'string' ? value.code : 'Unknown', detail: typeof value.detail === 'string' ? value.detail : null };
  }
  return { code: 'Unknown', detail: error instanceof Error ? error.message : String(error) };
}
