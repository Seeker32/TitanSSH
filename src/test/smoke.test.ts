import { describe, it, expect } from 'vitest';

describe('Project Setup', () => {
  it('should have working test infrastructure', () => {
    expect(true).toBe(true);
  });

  it('should support TypeScript', () => {
    const message: string = 'TypeScript is working';
    expect(message).toContain('TypeScript');
  });

  it('should have Tauri API mocked', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    expect(invoke).toBeDefined();
  });
});
