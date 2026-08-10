import { useEffect } from 'react';
import { ConfigProvider, theme as antTheme } from 'antd';
import enUS from 'antd/locale/en_US';
import zhCN from 'antd/locale/zh_CN';
import HomePage from '@/pages/HomePage';
import { useLocaleStore } from '@/stores/locale';
import { useThemeStore } from '@/stores/theme';

/** 配置全局主题并渲染唯一主页面。 */
export default function App() {
  const theme = useThemeStore((state) => state.theme);
  const initTheme = useThemeStore((state) => state.initTheme);
  const locale = useLocaleStore((state) => state.locale);
  const initLocale = useLocaleStore((state) => state.initLocale);

  useEffect(() => {
    initTheme();
    initLocale();
  }, [initLocale, initTheme]);

  useEffect(() => { document.documentElement.lang = locale; }, [locale]);

  return (
    <ConfigProvider
      locale={locale === 'zh-CN' ? zhCN : enUS}
      theme={{
        algorithm: theme === 'dark' ? antTheme.darkAlgorithm : antTheme.defaultAlgorithm,
        token: {
          colorPrimary: theme === 'dark' ? '#10b981' : '#059669',
          borderRadius: 12,
          fontFamily: '"SF Pro Text", "PingFang SC", "Helvetica Neue", sans-serif',
        },
      }}
    >
      <HomePage />
    </ConfigProvider>
  );
}
