import { chmodSync, constants, accessSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

describe('macOS ARM packaging script', () => {
  const scriptPath = resolve(process.cwd(), 'scripts/build-macos-arm.sh');

  it('builds the native ARM DMG from the project root', () => {
    accessSync(scriptPath, constants.X_OK);
    const script = readFileSync(scriptPath, 'utf8');

    expect(script).toContain('uname -m');
    expect(script).toContain('aarch64-apple-darwin');
    expect(script).toContain('--bundles dmg');
    expect(script).toContain('cd "$PROJECT_ROOT"');
  });

  it('uses an ad-hoc signing identity', () => {
    const config = JSON.parse(readFileSync(resolve(process.cwd(), 'src-tauri/tauri.conf.json'), 'utf8'));

    expect(config.bundle.macOS.signingIdentity).toBe('-');
  });

  it('rejects non-macOS hosts', () => {
    const fakeBin = mkdtempSync(resolve(tmpdir(), 'titanssh-build-script-'));
    const fakeUname = resolve(fakeBin, 'uname');

    try {
      writeFileSync(fakeUname, '#!/bin/sh\necho Linux\n');
      chmodSync(fakeUname, 0o755);
      const result = spawnSync('/bin/bash', [scriptPath], {
        encoding: 'utf8',
        env: { ...process.env, PATH: fakeBin },
      });

      expect(result.status).toBe(1);
      expect(result.stderr).toContain('macOS');
    } finally {
      rmSync(fakeBin, { recursive: true, force: true });
    }
  });
});
