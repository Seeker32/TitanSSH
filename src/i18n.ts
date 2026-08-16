import { enUS } from './i18n/en-US';
import { zhCN } from './i18n/zh-CN';
import { errorDetailPhrases } from './i18n/error-details';

/** 应用支持的界面语言。 */
export type Locale = 'zh-CN' | 'en-US';

/** 后端跨 Tauri 边界返回的稳定错误。
 *
 * detail 为纯机器诊断（结构化详情时为 null）；detailKey 为中文固定文案模板
 * （gettext msgid 风格，{0}/{1} 占位），由前端按当前语言翻译；detailParams 为
 * 与占位对应的语言无关参数（底层错误、路径、endpoint 等）。 */
export interface AppErrorInfo {
  code: string;
  detail?: string | null;
  detailKey?: string | null;
  detailParams?: string[] | null;
}

export type TranslationKey = keyof typeof zhCN;
const dictionaries: Record<Locale, Record<TranslationKey, string>> = { 'zh-CN': zhCN, 'en-US': enUS };

/** 按语言读取文案并替换简单命名参数。 */
export function translate(locale: Locale, key: TranslationKey, params: Record<string, string | number> = {}): string {
  return dictionaries[locale][key].replace(/\{(\w+)\}/g, (_, name: string) => String(params[name] ?? `{${name}}`));
}

/** 用参数按占位替换详情模板；模板占位之外的额外参数（后端追加详情）以「；」连接。 */
function renderErrorDetailTemplate(template: string, params: string[]): string {
  let rendered = template;
  for (let index = 0; index < params.length; index += 1) {
    const placeholder = `{${index}}`;
    if (rendered.includes(placeholder)) {
      rendered = rendered.replace(placeholder, params[index]);
    } else {
      rendered += `；${params[index]}`;
    }
  }
  return rendered;
}

/** 渲染错误详情：结构化详情（detailKey）按语言翻译，无翻译时回退中文模板；
 *  纯文本详情（detail）原样保留。 */
function renderErrorDetail(locale: Locale, error: AppErrorInfo): string {
  if (error.detailKey) {
    const template =
      locale === 'zh-CN' ? error.detailKey : (errorDetailPhrases[error.detailKey] ?? error.detailKey);
    return renderErrorDetailTemplate(template, error.detailParams ?? []);
  }
  return error.detail?.trim() ?? '';
}

/** 格式化后端错误，保留诊断详情用于排障。 */
export function formatAppError(locale: Locale, error: AppErrorInfo | null | undefined): string {
  if (!error) return translate(locale, 'error.Unknown');
  const key = `error.${error.code}` as TranslationKey;
  const summary = key in dictionaries[locale] ? translate(locale, key) : translate(locale, 'error.Unknown');
  const detailText = renderErrorDetail(locale, error);
  return detailText ? `${summary}: ${detailText}` : summary;
}

/** 将 Tauri command rejection 规范为结构化错误。 */
export function toAppError(error: unknown): AppErrorInfo {
  if (error && typeof error === 'object' && 'code' in error) {
    const value = error as { code: unknown; detail?: unknown; detailKey?: unknown; detailParams?: unknown };
    const result: AppErrorInfo = {
      code: typeof value.code === 'string' ? value.code : 'Unknown',
      detail: typeof value.detail === 'string' ? value.detail : null,
    };
    if (typeof value.detailKey === 'string') result.detailKey = value.detailKey;
    if (
      Array.isArray(value.detailParams) &&
      value.detailParams.every((param) => typeof param === 'string')
    ) {
      result.detailParams = value.detailParams;
    }
    return result;
  }
  return { code: 'Unknown', detail: error instanceof Error ? error.message : String(error) };
}
