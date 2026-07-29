import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { buildLocalServeUrls, findPonduinBinaryPath, startPonduinServe } from './ponduinServe';

const binaryName = process.platform === 'win32' ? 'ponduin.exe' : 'ponduin';
const tempDirs: string[] = [];
const originalCwd = process.cwd();
type ReadinessFetchInit = Parameters<typeof globalThis.fetch>[1];

function makeTempDir(): string {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'ponduin-serve-test-'));
  tempDirs.push(tempDir);
  return tempDir;
}

function makeFile(filePath: string): string {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, '');
  fs.chmodSync(filePath, 0o755);
  return filePath;
}

function makeExecutable(filePath: string, contents: string): string {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, contents);
  fs.chmodSync(filePath, 0o755);
  return filePath;
}

async function waitForFileLines(filePath: string): Promise<string[]> {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    if (fs.existsSync(filePath)) {
      return fs.readFileSync(filePath, 'utf8').trim().split('\n');
    }
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  throw new Error(`Timed out waiting for ${filePath}`);
}

describe('findPonduinBinaryPath', () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    process.chdir(originalCwd);

    while (tempDirs.length > 0) {
      const tempDir = tempDirs.pop();
      if (tempDir) {
        fs.rmSync(tempDir, { recursive: true, force: true });
      }
    }
  });

  it('uses PONDUIN_BINARY in development builds', () => {
    const tempDir = makeTempDir();
    const overridePath = makeFile(path.join(tempDir, 'override-ponduin'));
    vi.stubEnv('PONDUIN_BINARY', overridePath);

    expect(findPonduinBinaryPath({ isPackaged: false })).toBe(overridePath);
  });

  it('rejects PONDUIN_BINARY in packaged builds', () => {
    const tempDir = makeTempDir();
    const resourcesPath = path.join(tempDir, 'resources');
    const overridePath = makeFile(path.join(tempDir, 'override-ponduin'));
    makeFile(path.join(resourcesPath, 'bin', binaryName));
    vi.stubEnv('PONDUIN_BINARY', overridePath);

    expect(() => findPonduinBinaryPath({ isPackaged: true, resourcesPath })).toThrow(
      'PONDUIN_BINARY is only supported in development builds'
    );
  });

  it('prefers the staged binary over target builds in development builds', () => {
    const tempDir = makeTempDir();
    const desktopDir = path.join(tempDir, 'ui', 'desktop');
    const stagedPath = makeFile(path.join(desktopDir, 'src', 'bin', binaryName));
    const debugPath = makeFile(path.join(tempDir, 'target', 'debug', binaryName));
    const releasePath = makeFile(path.join(tempDir, 'target', 'release', binaryName));
    process.chdir(desktopDir);

    const resolvedPath = findPonduinBinaryPath({ isPackaged: false });
    expect(fs.realpathSync(resolvedPath)).toBe(fs.realpathSync(stagedPath));
    expect(fs.realpathSync(resolvedPath)).not.toBe(fs.realpathSync(releasePath));
    expect(fs.realpathSync(resolvedPath)).not.toBe(fs.realpathSync(debugPath));
  });

  it('uses the bundled ponduin binary in packaged builds', () => {
    const tempDir = makeTempDir();
    const resourcesPath = path.join(tempDir, 'resources');
    const bundledPath = makeFile(path.join(resourcesPath, 'bin', binaryName));

    expect(findPonduinBinaryPath({ isPackaged: true, resourcesPath })).toBe(bundledPath);
  });
});

describe('buildLocalServeUrls', () => {
  it('builds HTTP and WS URLs', () => {
    expect(buildLocalServeUrls(1234, 'secret', 'http')).toEqual({
      httpBaseUrl: 'http://127.0.0.1:1234',
      statusUrl: 'http://127.0.0.1:1234/status',
      healthUrl: 'http://127.0.0.1:1234/health',
      acpUrl: 'ws://127.0.0.1:1234/acp?token=secret',
      redactedAcpUrl: 'ws://127.0.0.1:1234/acp?token=REDACTED',
    });
  });

  it('builds HTTPS and WSS URLs', () => {
    expect(buildLocalServeUrls(1234, 'secret', 'https')).toEqual({
      httpBaseUrl: 'https://127.0.0.1:1234',
      statusUrl: 'https://127.0.0.1:1234/status',
      healthUrl: 'https://127.0.0.1:1234/health',
      acpUrl: 'wss://127.0.0.1:1234/acp?token=secret',
      redactedAcpUrl: 'wss://127.0.0.1:1234/acp?token=REDACTED',
    });
  });
});

describe('startPonduinServe', () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    process.chdir(originalCwd);

    while (tempDirs.length > 0) {
      const tempDir = tempDirs.pop();
      if (tempDir) {
        fs.rmSync(tempDir, { recursive: true, force: true });
      }
    }
  });

  it.skipIf(process.platform === 'win32')('uses the injected readiness fetch', async () => {
    const tempDir = makeTempDir();
    const ponduinPath = makeExecutable(
      path.join(tempDir, 'ponduin'),
      '#!/usr/bin/env sh\nwhile true; do sleep 1; done\n'
    );
    vi.stubEnv('PONDUIN_BINARY', ponduinPath);

    const readinessUrls: string[] = [];
    const readinessFetch = vi.fn(async (input: string, _init?: ReadinessFetchInit) => {
      readinessUrls.push(input);
      return new Response(null, { status: 200 });
    });

    const result = await startPonduinServe({
      serverSecret: 'test-secret',
      dir: tempDir,
      readinessFetch,
    });

    try {
      expect(readinessFetch).toHaveBeenCalledTimes(1);
      expect(readinessUrls[0]).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/status$/);
    } finally {
      await result.cleanup();
    }
  });

  it.skipIf(process.platform === 'win32')('captures the TLS fingerprint from stdout', async () => {
    const tempDir = makeTempDir();
    const ponduinPath = makeExecutable(
      path.join(tempDir, 'ponduin'),
      [
        '#!/usr/bin/env sh',
        'printf "PONDUIND_CERT_FINGERPRINT=AA:BB:CC\\n"',
        'while true; do sleep 1; done',
        '',
      ].join('\n')
    );
    vi.stubEnv('PONDUIN_BINARY', ponduinPath);

    let fingerprintLogged!: () => void;
    const fingerprintSeen = new Promise<void>((resolve) => {
      fingerprintLogged = resolve;
    });
    const logger = {
      info: vi.fn((message: unknown) => {
        if (String(message).includes('Pinned cert fingerprint')) {
          fingerprintLogged();
        }
      }),
      error: vi.fn(),
    };
    const readinessFetch = vi.fn(async () => {
      await fingerprintSeen;
      return new Response(null, { status: 200 });
    });

    const result = await startPonduinServe({
      serverSecret: 'test-secret',
      dir: tempDir,
      logger,
      readinessFetch,
    });

    try {
      expect(result.certFingerprint).toBe('AA:BB:CC');
    } finally {
      await result.cleanup();
    }
  });

  it.skipIf(process.platform === 'win32')(
    'uses TLS URLs and args when TLS is enabled',
    async () => {
      const tempDir = makeTempDir();
      const argsPath = path.join(tempDir, 'args.txt');
      const ponduinPath = makeExecutable(
        path.join(tempDir, 'ponduin'),
        [
          '#!/usr/bin/env sh',
          'printf "%s\\n" "$@" > "$TEST_ARGS_PATH"',
          'printf "PONDUIND_CERT_FINGERPRINT=DD:EE:FF\\n"',
          'while true; do sleep 1; done',
          '',
        ].join('\n')
      );
      vi.stubEnv('PONDUIN_BINARY', ponduinPath);

      const readinessUrls: string[] = [];
      let fingerprintRegistered = false;
      const logger = {
        info: vi.fn(),
        error: vi.fn(),
      };
      const readinessFetch = vi.fn(async (input: string, _init?: ReadinessFetchInit) => {
        expect(fingerprintRegistered).toBe(true);
        readinessUrls.push(input);
        return new Response(null, { status: 200 });
      });

      const result = await startPonduinServe({
        serverSecret: 'test-secret',
        dir: tempDir,
        tls: true,
        env: {
          TEST_ARGS_PATH: argsPath,
        },
        logger,
        readinessFetch,
        onCertFingerprint: (fingerprint) => {
          expect(fingerprint).toBe('DD:EE:FF');
          fingerprintRegistered = true;
        },
      });

      try {
        expect(readinessUrls[0]).toMatch(/^https:\/\/127\.0\.0\.1:\d+\/status$/);
        expect(result.acpUrl).toMatch(/^wss:\/\/127\.0\.0\.1:\d+\/acp\?token=test-secret$/);
        expect(result.certFingerprint).toBe('DD:EE:FF');
        expect(fingerprintRegistered).toBe(true);
        await expect(waitForFileLines(argsPath)).resolves.toContain('--tls');
      } finally {
        await result.cleanup();
      }
    }
  );

  it.skipIf(process.platform === 'win32')(
    'waits for and registers the TLS fingerprint before readiness',
    async () => {
      const tempDir = makeTempDir();
      const ponduinPath = makeExecutable(
        path.join(tempDir, 'ponduin'),
        [
          '#!/usr/bin/env sh',
          'sleep 0.1',
          'printf "PONDUIND_CERT_FINGERPRINT=11:22:33\\n"',
          'while true; do sleep 1; done',
          '',
        ].join('\n')
      );
      vi.stubEnv('PONDUIN_BINARY', ponduinPath);

      const events: string[] = [];
      const readinessFetch = vi.fn(async () => {
        events.push('readiness');
        return new Response(null, { status: 200 });
      });

      const result = await startPonduinServe({
        serverSecret: 'test-secret',
        dir: tempDir,
        tls: true,
        readinessFetch,
        onCertFingerprint: () => {
          events.push('fingerprint');
        },
      });

      try {
        expect(readinessFetch).toHaveBeenCalled();
        expect(result.certFingerprint).toBe('11:22:33');
        expect(events).toEqual(['fingerprint', 'readiness']);
      } finally {
        await result.cleanup();
      }
    }
  );
});
