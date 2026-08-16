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

  it('日志导出路径解析失败：摘要按语言显示，detail 只保留底层诊断', () => {
    const error = { code: 'LogExportPathResolveFailed', detail: 'URL is not a valid path' };
    expect(formatAppError('zh-CN', error)).toBe('无法解析保存路径: URL is not a valid path');
    expect(formatAppError('en-US', error)).toBe('Could not resolve save path: URL is not a valid path');
  });

  it('无效日志等级：专用 code 本地化摘要，详情携带输入值供诊断', () => {
    const error = { code: 'InvalidLogLevel', detail: 'verbose' };
    expect(formatAppError('zh-CN', error)).toBe('无效的日志等级: verbose');
    expect(formatAppError('en-US', error)).toBe('Invalid log level: verbose');
  });

  it('结构化详情按语言渲染模板，参数按占位顺序替换', () => {
    const error = {
      code: 'StorageError',
      detailKey: '读取主机配置文件失败: {0}',
      detailParams: ['permission denied'],
    };
    expect(formatAppError('zh-CN', error)).toBe('存储错误: 读取主机配置文件失败: permission denied');
    expect(formatAppError('en-US', error)).toBe('Storage error: Could not read host config file: permission denied');
  });

  it('结构化详情无英文翻译时回退为中文模板，不静默丢失诊断', () => {
    const error = {
      code: 'StorageError',
      detailKey: '未知短语: {0}',
      detailParams: ['x'],
    };
    expect(formatAppError('en-US', error)).toBe('Storage error: 未知短语: x');
  });

  it('模板占位之外的额外参数（追加详情）以「；」连接在末尾', () => {
    const error = {
      code: 'SftpReadError',
      detailKey: '远端读取失败: {0}',
      detailParams: ['connection reset', 'cleanup failed'],
    };
    expect(formatAppError('zh-CN', error)).toBe(
      'SFTP 读取失败: 远端读取失败: connection reset；cleanup failed',
    );
  });

  it('本地存储不可用时仍保留内存语言偏好', () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementationOnce(() => { throw new Error('unavailable'); });
    useLocaleStore.getState().setLocale('en-US');
    expect(useLocaleStore.getState().locale).toBe('en-US');
  });
});
