import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';
import { beforeEach } from 'vitest';
import { useLocaleStore } from '@/stores/locale';

afterEach(cleanup);

beforeEach(() => useLocaleStore.setState({ locale: 'zh-CN' }));

Object.defineProperty(window, 'matchMedia', {
  configurable: true,
  value: () => ({ matches: false, addListener: () => {}, removeListener: () => {}, addEventListener: () => {}, removeEventListener: () => {} }),
});

class ResizeObserverMock {
  observe() {}
  unobserve() {}
  disconnect() {}
}

vi.stubGlobal('ResizeObserver', ResizeObserverMock);

const nativeGetComputedStyle = window.getComputedStyle;
window.getComputedStyle = (element: Element) => nativeGetComputedStyle(element);
