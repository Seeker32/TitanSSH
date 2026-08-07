import { useEffect } from 'react';
import { ConfigProvider, theme as antTheme } from 'antd';
import HomePage from '@/pages/HomePage';
import { useThemeStore } from '@/stores/theme';

/** 配置全局主题并渲染唯一主页面。 */
export default function App() {
  const theme = useThemeStore((state) => state.theme);
  const initTheme = useThemeStore((state) => state.initTheme);

  useEffect(() => {
    initTheme();
  }, [initTheme]);

  return (
    <ConfigProvider
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
