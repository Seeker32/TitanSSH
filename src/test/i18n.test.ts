import { beforeEach, describe, expect, it, vi } from 'vitest';
import { formatAppError, translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';

describe('i18n', () => {
  beforeEach(() => {
    localStorage.clear();
    useLocaleStore.setState({ locale: 'zh-CN' });
  });

  it('优先使用已保存语言，否则按系统语言选择', () => {
    localStorage.setItem('locale', 'en-US');
    expect(useLocaleStore.getState().initLocale(['zh-CN'])).toBe('en-US');

    localStorage.removeItem('locale');
    expect(useLocaleStore.getState().initLocale(['en-GB'])).toBe('en-US');
    expect(useLocaleStore.getState().initLocale(['zh-TW'])).toBe('zh-CN');
  });

  it('保存用户语言选择，并翻译错误摘要同时保留原始详情', () => {
    useLocaleStore.getState().setLocale('en-US');
    expect(localStorage.getItem('locale')).toBe('en-US');
    expect(translate('en-US', 'settings.language')).toBe('Language');
    expect(formatAppError('en-US', { code: 'AuthenticationError', detail: 'Permission denied' }))
      .toBe('Authentication failed: Permission denied');
  });

  it('本地存储不可用时仍保留内存语言偏好', () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementationOnce(() => { throw new Error('unavailable'); });
    useLocaleStore.getState().setLocale('en-US');
    expect(useLocaleStore.getState().locale).toBe('en-US');
  });
});
