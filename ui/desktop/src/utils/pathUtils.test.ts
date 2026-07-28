import os from 'node:os';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  isAbsolutePonduinPath,
  resolvePonduinPathRoot,
  sanitizePonduinPathRoot,
} from './pathUtils';

describe('resolvePonduinPathRoot', () => {
  it('rejects empty and relative values', () => {
    expect(resolvePonduinPathRoot(undefined)).toBeUndefined();
    expect(resolvePonduinPathRoot('   ')).toBeUndefined();
    expect(resolvePonduinPathRoot('relative/root')).toBeUndefined();
  });

  it('retains absolute paths without requiring them to exist', () => {
    const absolute = path.resolve('nonexistent-ponduin-root');
    expect(resolvePonduinPathRoot(`  ${absolute}  `)).toBe(absolute);
  });

  it('expands a home-relative root before validation', () => {
    expect(resolvePonduinPathRoot('~')).toBe(os.homedir());
  });

  it('removes a rejected value from the child-process environment', () => {
    const env = { PONDUIN_PATH_ROOT: 'relative/root' };
    expect(sanitizePonduinPathRoot(env)).toBeUndefined();
    expect(env).not.toHaveProperty('PONDUIN_PATH_ROOT');
  });

  it('matches Rust absolute-path handling on Windows', () => {
    expect(isAbsolutePonduinPath('C:\\ponduin\\root', 'win32')).toBe(true);
    expect(isAbsolutePonduinPath('\\\\server\\share\\ponduin', 'win32')).toBe(true);
    expect(isAbsolutePonduinPath('C:ponduin\\root', 'win32')).toBe(false);
    expect(isAbsolutePonduinPath('\\ponduin\\root', 'win32')).toBe(false);
    expect(isAbsolutePonduinPath('/ponduin/root', 'win32')).toBe(false);
  });
});
