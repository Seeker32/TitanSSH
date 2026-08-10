import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

describe('application name', () => {
  it('uses TitanSSH for package output and window branding', () => {
    const packageJson = JSON.parse(readFileSync(resolve(process.cwd(), 'package.json'), 'utf8'));
    const tauriConfig = JSON.parse(readFileSync(resolve(process.cwd(), 'src-tauri/tauri.conf.json'), 'utf8'));
    const cargoToml = readFileSync(resolve(process.cwd(), 'src-tauri/Cargo.toml'), 'utf8');
    const main = readFileSync(resolve(process.cwd(), 'src-tauri/src/main.rs'), 'utf8');

    expect(packageJson.name).toBe('titanssh');
    expect(tauriConfig.productName).toBe('TitanSSH');
    expect(tauriConfig.identifier).toBe('com.titanssh.desktop');
    expect(tauriConfig.app.windows[0].title).toBe('TitanSSH');
    expect(cargoToml).toContain('name = "titanssh"');
    expect(main).toContain('titanssh::run()');
  });
});
