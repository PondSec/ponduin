import { spawn, type ChildProcess } from 'child_process';
import fs from 'node:fs';
import https from 'node:https';
import { createServer } from 'node:net';
import os from 'node:os';
import path from 'node:path';
import type { TLSSocket } from 'node:tls';
import { normalizeCertificateFingerprint } from './certificateFingerprint';
import {
  appendTail as appendStartupTail,
  createPonduinServeStartupDiagnostics,
  type PonduinServeStartupDiagnostics,
} from './startupDiagnostics';

export interface Logger {
  info: (...args: unknown[]) => void;
  error: (...args: unknown[]) => void;
}

export const defaultLogger: Logger = {
  info: (...args) => console.log('[ponduin-serve]', ...args),
  error: (...args) => console.error('[ponduin-serve]', ...args),
};

export interface FindPonduinBinaryOptions {
  isPackaged?: boolean;
  resourcesPath?: string;
}

type ReadinessFetchInit = Parameters<typeof globalThis.fetch>[1];
export type PonduinServeExitSignal = ChildProcess['signalCode'];
type ReadinessFetch = (input: string, init?: ReadinessFetchInit) => Promise<Response>;

export interface StartPonduinServeOptions extends FindPonduinBinaryOptions {
  dir?: string;
  serverSecret: string;
  tls?: boolean;
  env?: Record<string, string | undefined>;
  logger?: Logger;
  diagnosticsDir?: string;
  readinessFetch?: ReadinessFetch;
  onCertFingerprint?: (fingerprint: string) => void;
}

export interface PonduinServeResult {
  acpUrl: string;
  workingDir: string;
  process: ChildProcess;
  errorLog: string[];
  certFingerprint: string | null;
  cleanup: () => Promise<void>;
  hasExited: () => boolean;
  getExitDetails: () => { code: number | null; signal: PonduinServeExitSignal };
  startupDiagnosticsPath: string | null;
  getStartupDiagnostics: () => PonduinServeStartupDiagnostics | null;
  recordStartupEvent: (name: string, details?: Record<string, unknown>) => void;
}

const existingFile = (candidate: string): boolean => {
  try {
    return fs.existsSync(candidate) && fs.statSync(candidate).isFile();
  } catch {
    return false;
  }
};

export const findPonduinBinaryPath = (options: FindPonduinBinaryOptions = {}): string => {
  const { isPackaged = false, resourcesPath } = options;
  const pathFromEnv = process.env.PONDUIN_BINARY;
  if (pathFromEnv) {
    if (isPackaged) {
      throw new Error('PONDUIN_BINARY is only supported in development builds');
    }

    const resolvedPath = path.resolve(pathFromEnv);
    if (existingFile(resolvedPath)) {
      return resolvedPath;
    }
    throw new Error(`Invalid PONDUIN_BINARY path: ${pathFromEnv} (pwd is ${process.cwd()})`);
  }

  const binaryName = process.platform === 'win32' ? 'ponduin.exe' : 'ponduin';
  const possiblePaths: string[] = [];

  if (isPackaged && resourcesPath) {
    possiblePaths.push(path.join(resourcesPath, 'bin', binaryName));
    possiblePaths.push(path.join(resourcesPath, binaryName));
  } else {
    possiblePaths.push(
      path.join(process.cwd(), 'src', 'bin', binaryName),
      path.join(process.cwd(), '..', '..', 'target', 'release', binaryName),
      path.join(process.cwd(), '..', '..', 'target', 'debug', binaryName)
    );
  }

  for (const candidate of possiblePaths) {
    if (existingFile(candidate)) {
      return candidate;
    }
  }

  throw new Error(
    `Ponduin binary not found in any of the possible paths: ${possiblePaths.join(', ')}`
  );
};

const findAvailablePort = (): Promise<number> => {
  return new Promise((resolve, reject) => {
    const server = createServer();

    server.on('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address() as { port: number };
      server.close(() => {
        resolve(port);
      });
    });
  });
};

const delay = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

const isFatalError = (line: string): boolean => {
  const fatalPatterns = [/panicked at/, /RUST_BACKTRACE/, /fatal error/i];
  return fatalPatterns.some((pattern) => pattern.test(line));
};

const appendErrorTail = (target: string[], lines: string[], maxLines = 100): void => {
  for (const line of lines) {
    if (line.trim()) {
      target.push(line);
    }
  }
  if (target.length > maxLines) {
    target.splice(0, target.length - maxLines);
  }
};

const CERT_FINGERPRINT_PREFIX = 'PONDUIND_CERT_FINGERPRINT=';
const TLS_FINGERPRINT_TIMEOUT_MS = 15_000;

interface ReadinessProbeResult {
  ok: boolean;
  status?: number;
  error?: string;
}

export const createPinnedHttpsReadinessFetch = (expectedFingerprint: string): ReadinessFetch => {
  const normalizedExpectedFingerprint = normalizeCertificateFingerprint(expectedFingerprint);

  return async (input, init) =>
    new Promise<Response>((resolve, reject) => {
      const request = https.request(
        input,
        {
          method: 'GET',
          rejectUnauthorized: false,
          signal: init?.signal ?? undefined,
        },
        (response) => {
          const certificate = (response.socket as TLSSocket).getPeerCertificate();
          const actualFingerprint = certificate.fingerprint256
            ? normalizeCertificateFingerprint(certificate.fingerprint256)
            : null;

          if (!actualFingerprint || actualFingerprint !== normalizedExpectedFingerprint) {
            response.resume();
            reject(new Error('TLS certificate fingerprint mismatch during readiness check'));
            return;
          }

          response.resume();
          resolve(new Response(null, { status: response.statusCode ?? 500 }));
        }
      );

      request.once('error', reject);
      request.end();
    });
};

const fetchStatus = async (
  statusUrl: string,
  readinessFetch: ReadinessFetch
): Promise<ReadinessProbeResult> => {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 1000);

  try {
    const response = await readinessFetch(statusUrl, { signal: controller.signal });
    return {
      ok: response.ok,
      status: response.status,
      ...(!response.ok ? { error: `HTTP ${response.status}` } : {}),
    };
  } catch (error) {
    return {
      ok: false,
      error: error instanceof Error ? `${error.name}: ${error.message}` : String(error),
    };
  } finally {
    clearTimeout(timeout);
  }
};

const waitForFingerprint = async (
  fingerprintReady: Promise<string | null>,
  timeoutMs: number
): Promise<string | null> => {
  let timeout: ReturnType<typeof setTimeout> | undefined;
  const timeoutPromise = new Promise<null>((resolve) => {
    timeout = setTimeout(() => resolve(null), timeoutMs);
  });

  try {
    return await Promise.race([fingerprintReady, timeoutPromise]);
  } finally {
    if (timeout) {
      clearTimeout(timeout);
    }
  }
};

const waitForPonduinServeReady = async (
  statusUrl: string,
  errorLog: string[],
  shouldStopWaiting: () => boolean,
  options: {
    healthUrl: string;
    readinessFetch: ReadinessFetch;
    onEvent?: (name: string, details?: Record<string, unknown>) => void;
  }
): Promise<boolean> => {
  const timeout = 30000;
  const interval = 100;
  const deadline = Date.now() + timeout;
  const probeDetails = {
    transport: statusUrl.startsWith('https:') ? 'https' : 'plain-http',
    method: 'GET',
    path: '/status',
    url: statusUrl,
    statusUrl,
    healthUrl: options.healthUrl,
  };
  options.onEvent?.('healthcheck_start', {
    ...probeDetails,
    timeoutMs: timeout,
    intervalMs: interval,
  });

  let attempt = 1;
  let lastProbe: ReadinessProbeResult | null = null;
  while (Date.now() < deadline) {
    if (shouldStopWaiting()) {
      options.onEvent?.('healthcheck_fatal_error', {
        ...probeDetails,
        attempt,
        reason: 'process_unavailable',
      });
      return false;
    }

    if (errorLog.some(isFatalError)) {
      options.onEvent?.('healthcheck_fatal_error', {
        ...probeDetails,
        attempt,
        reason: 'fatal_stderr',
      });
      return false;
    }

    lastProbe = await fetchStatus(statusUrl, options.readinessFetch);
    if (lastProbe.ok) {
      options.onEvent?.('healthcheck_success', {
        ...probeDetails,
        attempt,
        status: lastProbe.status,
      });
      return true;
    }

    await delay(interval);
    attempt += 1;
  }

  options.onEvent?.('healthcheck_timeout', {
    ...probeDetails,
    timeoutMs: timeout,
    attempts: attempt - 1,
    ...(lastProbe?.status !== undefined ? { lastStatus: lastProbe.status } : {}),
    ...(lastProbe?.error ? { lastError: lastProbe.error } : {}),
  });
  return false;
};

export type LocalServeScheme = 'http' | 'https';

export interface LocalServeUrls {
  httpBaseUrl: string;
  statusUrl: string;
  healthUrl: string;
  acpUrl: string;
  redactedAcpUrl: string;
}

export const buildLocalServeUrls = (
  port: number,
  token: string,
  scheme: LocalServeScheme
): LocalServeUrls => {
  const httpBaseUrl = `${scheme}://127.0.0.1:${port}`;
  const websocketProtocol = scheme === 'https' ? 'wss:' : 'ws:';

  const acpUrl = new URL(`${httpBaseUrl}/acp`);
  acpUrl.protocol = websocketProtocol;
  acpUrl.searchParams.set('token', token);

  const redactedAcpUrl = new URL(`${httpBaseUrl}/acp`);
  redactedAcpUrl.protocol = websocketProtocol;
  redactedAcpUrl.searchParams.set('token', 'REDACTED');

  return {
    httpBaseUrl,
    statusUrl: `${httpBaseUrl}/status`,
    healthUrl: `${httpBaseUrl}/health`,
    acpUrl: acpUrl.toString(),
    redactedAcpUrl: redactedAcpUrl.toString(),
  };
};

const errorMessage = (error: unknown): string => {
  if (error instanceof Error) {
    return error.message;
  }
  return String(error);
};

const withStartupDiagnosticsPath = (
  message: string,
  startupDiagnosticsPath: string | null
): string => {
  if (!startupDiagnosticsPath) {
    return message;
  }
  return `${message} Startup diagnostics: ${startupDiagnosticsPath}`;
};

const buildPonduinServeEnv = (
  serverSecret: string,
  binaryPath: string,
  additionalEnv: Record<string, string | undefined>
): Record<string, string | undefined> => {
  const homeDir = process.env.HOME || os.homedir();
  const pathKey = process.platform === 'win32' ? 'Path' : 'PATH';
  const currentPath = process.env[pathKey] || '';

  const env: Record<string, string | undefined> = {
    ...process.env,
    HOME: homeDir,
    [pathKey]: `${path.dirname(binaryPath)}${path.delimiter}${currentPath}`,
  };

  if (process.platform === 'win32') {
    env.USERPROFILE = homeDir;
    env.APPDATA = process.env.APPDATA || path.join(homeDir, 'AppData', 'Roaming');
    env.LOCALAPPDATA = process.env.LOCALAPPDATA || path.join(homeDir, 'AppData', 'Local');
  }

  for (const [key, value] of Object.entries(additionalEnv)) {
    if (value !== undefined) {
      env[key] = value;
    }
  }

  env.PONDUIN_SERVER__SECRET_KEY = serverSecret;

  return env;
};

export const startPonduinServe = async ({
  dir,
  serverSecret,
  tls = false,
  env: additionalEnv = {},
  isPackaged,
  resourcesPath,
  logger = defaultLogger,
  diagnosticsDir,
  readinessFetch,
  onCertFingerprint,
}: StartPonduinServeOptions): Promise<PonduinServeResult> => {
  const workingDir = dir || process.cwd();
  const startupTrace = createPonduinServeStartupDiagnostics(diagnosticsDir, workingDir);
  const startupDiagnosticsPath = startupTrace?.diagnosticsPath ?? null;
  const secretKey = serverSecret.trim();
  if (!secretKey) {
    const message = 'PONDUIN_SERVER__SECRET_KEY is required for ponduin serve';
    startupTrace?.record('configuration_error', { message });
    throw new Error(withStartupDiagnosticsPath(message, startupDiagnosticsPath));
  }

  let ponduinPath: string;
  try {
    ponduinPath = findPonduinBinaryPath({ isPackaged, resourcesPath });
  } catch (error) {
    const message = errorMessage(error);
    startupTrace?.record('binary_resolve_error', { message });
    throw new Error(withStartupDiagnosticsPath(message, startupDiagnosticsPath));
  }

  const port = await findAvailablePort();
  const localServeScheme: LocalServeScheme = tls ? 'https' : 'http';
  const { httpBaseUrl, statusUrl, healthUrl, acpUrl, redactedAcpUrl } = buildLocalServeUrls(
    port,
    secretKey,
    localServeScheme
  );
  const errorLog: string[] = [];
  const args = [
    'serve',
    ...(tls ? ['--tls'] : []),
    '--platform',
    'desktop',
    '--host',
    '127.0.0.1',
    '--port',
    String(port),
  ];

  logger.info(`Starting ponduin serve from: ${ponduinPath} on port ${port} in dir ${workingDir}`);
  if (startupTrace) {
    startupTrace.diagnostics.binaryPath = ponduinPath;
    startupTrace.diagnostics.httpBaseUrl = httpBaseUrl;
    startupTrace.diagnostics.readinessUrl = statusUrl;
    startupTrace.diagnostics.statusUrl = statusUrl;
    startupTrace.diagnostics.healthUrl = healthUrl;
    startupTrace.diagnostics.acpUrl = redactedAcpUrl;
    startupTrace.record('spawn_start', {
      binaryPath: ponduinPath,
      port,
      tls,
      workingDir,
      args,
    });
  }

  const spawnOptions = {
    env: buildPonduinServeEnv(secretKey, ponduinPath, additionalEnv),
    cwd: workingDir,
    windowsHide: true,
    shell: false as const,
    stdio: ['ignore', 'pipe', 'pipe'] as ['ignore', 'pipe', 'pipe'],
  };

  const ponduinProcess = spawn(ponduinPath, args, spawnOptions);
  if (startupTrace) {
    startupTrace.diagnostics.pid = ponduinProcess.pid ?? null;
    startupTrace.record('spawn_success', { pid: ponduinProcess.pid ?? null });
  }

  let exited = false;
  let spawnFailed = false;
  let exitCode: number | null = null;
  let exitSignal: PonduinServeExitSignal = null;
  let certFingerprint: string | null = null;
  let stdoutBuffer = '';
  let stdoutCollectionStopped = false;
  let fingerprintReadyResolved = false;
  let resolveFingerprintReady: (fingerprint: string | null) => void = () => {};
  const fingerprintReady = new Promise<string | null>((resolve) => {
    resolveFingerprintReady = resolve;
  });

  const resolveFingerprint = (fingerprint: string | null) => {
    if (fingerprintReadyResolved) {
      return;
    }
    fingerprintReadyResolved = true;
    resolveFingerprintReady(fingerprint);
  };

  const stopStdoutCollection = () => {
    if (stdoutCollectionStopped) {
      return;
    }
    stdoutCollectionStopped = true;
    ponduinProcess.stdout?.off('data', onStdoutData);
    ponduinProcess.stdout?.resume();
  };

  const recordCertFingerprint = (fingerprint: string) => {
    if (!fingerprint) {
      return;
    }
    certFingerprint = fingerprint;
    logger.info(`Pinned cert fingerprint: ${certFingerprint}`);
    startupTrace?.record('fingerprint_received', { certFingerprint });
    try {
      onCertFingerprint?.(certFingerprint);
    } catch (error) {
      const message = `Failed to register ponduin serve TLS fingerprint: ${errorMessage(error)}`;
      appendErrorTail(errorLog, [message]);
      startupTrace?.record('fingerprint_registration_error', { message });
      spawnFailed = true;
    }
    resolveFingerprint(certFingerprint);
    stopStdoutCollection();
  };

  const onStdoutData = (data: Buffer) => {
    stdoutBuffer += data.toString();
    const lines = stdoutBuffer.split(/\r?\n/);
    stdoutBuffer = lines.pop() ?? '';

    for (const line of lines) {
      if (line.startsWith(CERT_FINGERPRINT_PREFIX)) {
        recordCertFingerprint(line.slice(CERT_FINGERPRINT_PREFIX.length).trim());
        return;
      }
    }
  };

  ponduinProcess.stdout?.on('data', onStdoutData);

  const onStderrData = (data: Buffer) => {
    const lines = data.toString().split('\n');
    appendErrorTail(errorLog, lines);
    if (startupTrace) {
      appendStartupTail(startupTrace.diagnostics.stderrTail, lines);
    }
    for (const line of lines) {
      if (line.trim() && isFatalError(line)) {
        logger.error(`ponduin serve stderr for port ${port} and dir ${workingDir}: ${line}`);
      }
    }
  };

  ponduinProcess.stderr?.on('data', onStderrData);

  ponduinProcess.on('exit', (code, signal) => {
    exited = true;
    exitCode = code;
    exitSignal = signal;
    logger.info(
      `ponduin serve process exited with code ${code} and signal ${signal} for port ${port} and dir ${workingDir}`
    );
    if (startupTrace) {
      startupTrace.diagnostics.childExitCode = code;
      startupTrace.diagnostics.childExitSignal = signal;
      startupTrace.record('child_exit', { code, signal });
    }
    resolveFingerprint(null);
  });

  ponduinProcess.on('error', (error) => {
    spawnFailed = true;
    errorLog.push(error.message);
    logger.error(`Failed to start ponduin serve on port ${port} and dir ${workingDir}`, error);
    startupTrace?.record('spawn_error', { message: error.message, name: error.name });
  });

  const cleanup = async (): Promise<void> => {
    return new Promise<void>((resolve) => {
      if (exited || ponduinProcess.killed) {
        resolve();
        return;
      }

      let resolved = false;
      const finish = () => {
        if (!resolved) {
          resolved = true;
          resolve();
        }
      };

      ponduinProcess.once('close', finish);

      logger.info('Terminating ponduin serve');
      try {
        if (process.platform === 'win32') {
          if (ponduinProcess.pid) {
            spawn('taskkill', ['/pid', ponduinProcess.pid.toString(), '/f', '/t']);
          }
        } else {
          ponduinProcess.kill('SIGTERM');
        }
      } catch (error) {
        logger.error('Error while terminating ponduin serve process:', error);
      }

      setTimeout(() => {
        if (!exited && !ponduinProcess.killed && process.platform !== 'win32') {
          ponduinProcess.kill('SIGKILL');
        }
        finish();
      }, 5000);
    });
  };

  const stopOutputCollection = () => {
    stopStdoutCollection();
    ponduinProcess.stderr?.off('data', onStderrData);
    ponduinProcess.stderr?.resume();
  };

  if (tls) {
    startupTrace?.record('fingerprint_wait_start', { timeoutMs: TLS_FINGERPRINT_TIMEOUT_MS });
    const fingerprint = await waitForFingerprint(fingerprintReady, TLS_FINGERPRINT_TIMEOUT_MS);
    if (!fingerprint) {
      stopOutputCollection();
      await cleanup();
      const exitDetails = exited
        ? ` Process exited with code ${exitCode} and signal ${exitSignal}.`
        : '';
      const stderrDetails = errorLog.length ? ` Stderr: ${errorLog.join('\n')}` : '';
      startupTrace?.record('fingerprint_missing', {
        timeoutMs: TLS_FINGERPRINT_TIMEOUT_MS,
        exited,
        exitCode,
        exitSignal,
      });
      throw new Error(
        withStartupDiagnosticsPath(
          `ponduin serve did not emit TLS certificate fingerprint on ${statusUrl}.${exitDetails}${stderrDetails}`,
          startupDiagnosticsPath
        )
      );
    }
  }

  const effectiveReadinessFetch =
    readinessFetch ??
    (tls && certFingerprint ? createPinnedHttpsReadinessFetch(certFingerprint) : fetch);
  const ready = await waitForPonduinServeReady(statusUrl, errorLog, () => exited || spawnFailed, {
    healthUrl,
    readinessFetch: effectiveReadinessFetch,
    onEvent: startupTrace?.record,
  });

  if (!ready) {
    stopOutputCollection();
    await cleanup();
    const exitDetails = exited
      ? ` Process exited with code ${exitCode} and signal ${exitSignal}.`
      : '';
    const stderrDetails = errorLog.length ? ` Stderr: ${errorLog.join('\n')}` : '';
    throw new Error(
      withStartupDiagnosticsPath(
        `ponduin serve did not become ready on ${statusUrl}.${exitDetails}${stderrDetails}`,
        startupDiagnosticsPath
      )
    );
  }

  stopOutputCollection();

  return {
    acpUrl,
    workingDir,
    process: ponduinProcess,
    errorLog,
    certFingerprint,
    cleanup,
    hasExited: () => exited,
    getExitDetails: () => ({ code: exitCode, signal: exitSignal }),
    startupDiagnosticsPath,
    getStartupDiagnostics: () => startupTrace?.diagnostics ?? null,
    recordStartupEvent: (name, details) => startupTrace?.record(name, details),
  };
};
