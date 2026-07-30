import { app } from 'electron';
import { execFile } from 'child_process';
import { createHash, randomUUID } from 'crypto';
import { constants as fsConstants, createReadStream } from 'fs';
import * as fs from 'fs/promises';
import * as os from 'os';
import * as path from 'path';
import log from './logger';
import { errorMessage } from './conversionUtils';
import {
  isAllowedGitHubUrl,
  isNewerVersion,
  parseUpdateManifest,
  releaseApiUrl,
  releaseTag,
  resolveUpdateChannel,
  selectManifestFile,
  updateAssetName,
  updateManifestName,
  type UpdateChannel,
} from './updateSource';

const CHECK_TIMEOUT_MS = 30_000;
const DOWNLOAD_TIMEOUT_MS = 15 * 60_000;
const MAX_MANIFEST_BYTES = 1024 * 1024;
const MAX_RELEASE_METADATA_BYTES = 4 * 1024 * 1024;
const MAX_UPDATE_BYTES = 2 * 1024 * 1024 * 1024;

interface GitHubReleaseAsset {
  name: string;
  browser_download_url: string;
  size: number;
}

interface GitHubRelease {
  tag_name: string;
  name: string;
  published_at: string;
  html_url: string;
  assets: GitHubReleaseAsset[];
}

interface GhReleaseAsset {
  name: string;
  size: number;
  url: string;
}

interface GhRelease {
  tagName: string;
  name: string;
  publishedAt: string;
  url: string;
  assets: GhReleaseAsset[];
}

interface ReleaseDescriptor {
  tag: string;
  name: string;
  publishedAt: string;
  releaseUrl: string;
  assets: GitHubReleaseAsset[];
  transport: 'https' | 'gh-cli';
  ghPath?: string;
}

export interface GitHubUpdateArtifact {
  version: string;
  assetName: string;
  size: number;
  sha512: string;
  releaseUrl: string;
  releaseTag: string;
  transport: 'https' | 'gh-cli';
  downloadUrl?: string;
  ghPath?: string;
}

export interface UpdateCheckResult {
  updateAvailable: boolean;
  latestVersion?: string;
  releaseUrl?: string;
  artifact?: GitHubUpdateArtifact;
  error?: string;
}

interface DownloadResult {
  success: boolean;
  downloadPath?: string;
  error?: string;
}

export class GitHubUpdater {
  private readonly owner = 'PondSec';
  private readonly repo = 'ponduin';
  private readonly bundleName = process.env.PONDUIN_BUNDLE_NAME || 'Ponduin';
  private readonly channel: UpdateChannel = resolveUpdateChannel(
    process.env.PONDUIN_UPDATE_CHANNEL
  );

  async checkForUpdates(): Promise<UpdateCheckResult> {
    const started = Date.now();
    try {
      const currentVersion = app.getVersion();
      log.info(`Checking ${this.channel} updates from ${this.owner}/${this.repo}`);
      log.info(`Current app version: ${currentVersion}`);

      const release = await this.loadRelease();
      const artifact = await this.resolveArtifact(release);
      const updateAvailable = isNewerVersion(artifact.version, currentVersion);

      log.info(
        `Update check completed in ${Date.now() - started}ms: current=${currentVersion}, latest=${artifact.version}, available=${updateAvailable}, transport=${artifact.transport}`
      );
      if (!updateAvailable) {
        return {
          updateAvailable: false,
          latestVersion: artifact.version,
          releaseUrl: artifact.releaseUrl,
        };
      }
      return {
        updateAvailable: true,
        latestVersion: artifact.version,
        releaseUrl: artifact.releaseUrl,
        artifact,
      };
    } catch (error) {
      const message = errorMessage(error, 'Unknown update error');
      log.error(`GitHub update check failed after ${Date.now() - started}ms: ${message}`);
      return { updateAvailable: false, error: message };
    }
  }

  async downloadUpdate(
    artifact: GitHubUpdateArtifact,
    onProgress?: (percent: number) => void
  ): Promise<DownloadResult> {
    const started = Date.now();
    const tempDir = await fs.mkdtemp(path.join(os.tmpdir(), 'ponduin-update-'));
    try {
      log.info(
        `Downloading verified ${this.channel} update ${artifact.version} via ${artifact.transport}`
      );
      onProgress?.(1);
      const temporaryArchive =
        artifact.transport === 'gh-cli'
          ? await this.downloadWithGitHubCli(artifact, tempDir, onProgress)
          : await this.downloadWithHttps(artifact, tempDir, onProgress);

      await verifyFileIntegrity(temporaryArchive, artifact);
      onProgress?.(95);
      const downloadPath = await copyVerifiedArchive(temporaryArchive, artifact, this.bundleName);
      onProgress?.(100);
      log.info(
        `Verified update ${artifact.version} saved in ${Date.now() - started}ms at ${downloadPath}`
      );
      return { success: true, downloadPath };
    } catch (error) {
      const message = errorMessage(error, 'Unknown download error');
      log.error(`Verified update download failed after ${Date.now() - started}ms: ${message}`);
      return { success: false, error: message };
    } finally {
      await fs.rm(tempDir, { recursive: true, force: true }).catch((error) => {
        log.warn(`Could not remove updater temporary directory: ${errorMessage(error, 'unknown')}`);
      });
    }
  }

  private async loadRelease(): Promise<ReleaseDescriptor> {
    if (this.channel === 'main') {
      return this.loadReleaseWithGitHubCli();
    }
    try {
      return await this.loadReleaseWithHttps();
    } catch (httpsError) {
      log.info(
        `Unauthenticated GitHub release lookup unavailable; trying authenticated GitHub CLI: ${errorMessage(httpsError, 'unknown')}`
      );
      return this.loadReleaseWithGitHubCli();
    }
  }

  private async loadReleaseWithHttps(): Promise<ReleaseDescriptor> {
    const apiUrl = releaseApiUrl(this.owner, this.repo, this.channel);
    const source = await fetchBoundedText(apiUrl, MAX_RELEASE_METADATA_BYTES);
    const release = JSON.parse(source) as GitHubRelease;
    if (
      !release ||
      typeof release.tag_name !== 'string' ||
      typeof release.html_url !== 'string' ||
      !Array.isArray(release.assets)
    ) {
      throw new Error('GitHub release API returned invalid release information');
    }
    return {
      tag: release.tag_name,
      name: release.name,
      publishedAt: release.published_at,
      releaseUrl: release.html_url,
      assets: release.assets,
      transport: 'https',
    };
  }

  private async loadReleaseWithGitHubCli(): Promise<ReleaseDescriptor> {
    const ghPath = await findGitHubCli();
    const tag = releaseTag(this.channel);
    const args = ['release', 'view'];
    if (tag) {
      args.push(tag);
    }
    args.push(
      '--repo',
      `${this.owner}/${this.repo}`,
      '--json',
      'tagName,name,publishedAt,url,assets'
    );
    const stdout = await runGitHubCli(ghPath, args, CHECK_TIMEOUT_MS);
    const release = JSON.parse(stdout) as GhRelease;
    if (
      !release ||
      typeof release.tagName !== 'string' ||
      !Array.isArray(release.assets) ||
      typeof release.url !== 'string'
    ) {
      throw new Error('GitHub CLI returned invalid release information');
    }
    return {
      tag: release.tagName,
      name: release.name,
      publishedAt: release.publishedAt,
      releaseUrl: release.url,
      assets: release.assets.map((asset) => ({
        name: asset.name,
        browser_download_url: asset.url,
        size: asset.size,
      })),
      transport: 'gh-cli',
      ghPath,
    };
  }

  private async resolveArtifact(release: ReleaseDescriptor): Promise<GitHubUpdateArtifact> {
    log.info(
      `Found ${this.channel} release ${release.tag} (${release.name}), published ${release.publishedAt}`
    );
    const manifestName = updateManifestName(process.platform);
    const manifestAsset = release.assets.find(
      (asset) => asset.name.toLowerCase() === manifestName.toLowerCase()
    );
    if (!manifestAsset) {
      throw new Error(
        `${this.channel} release does not contain the required ${manifestName} integrity manifest`
      );
    }

    const manifestSource =
      release.transport === 'gh-cli'
        ? await this.downloadAndVerifyManifest(release, manifestName)
        : await fetchManifest(manifestAsset.browser_download_url);
    const manifest = parseUpdateManifest(manifestSource);
    const expectedAssetName = updateAssetName(process.platform, process.arch, this.bundleName);
    const manifestFile = selectManifestFile(manifest, expectedAssetName);
    const releaseAsset = release.assets.find(
      (asset) => asset.name.toLowerCase() === manifestFile.url.toLowerCase()
    );
    if (!releaseAsset) {
      throw new Error(`Release does not contain manifest asset ${manifestFile.url}`);
    }
    if (releaseAsset.size !== manifestFile.size) {
      throw new Error(`Release size for ${manifestFile.url} does not match its manifest`);
    }
    if (release.transport === 'https' && !isAllowedGitHubUrl(releaseAsset.browser_download_url)) {
      throw new Error('Release asset URL is outside the allowed GitHub hosts');
    }

    return {
      version: manifest.version,
      assetName: manifestFile.url,
      size: manifestFile.size,
      sha512: manifestFile.sha512,
      releaseUrl: release.releaseUrl,
      releaseTag: release.tag,
      transport: release.transport,
      downloadUrl: release.transport === 'https' ? releaseAsset.browser_download_url : undefined,
      ghPath: release.ghPath,
    };
  }

  private async downloadAndVerifyManifest(
    release: ReleaseDescriptor,
    manifestName: string
  ): Promise<string> {
    if (!release.ghPath) {
      throw new Error('Authenticated GitHub CLI path is unavailable');
    }
    const tempDir = await fs.mkdtemp(path.join(os.tmpdir(), 'ponduin-update-manifest-'));
    try {
      await downloadReleaseAsset(
        release.ghPath,
        `${this.owner}/${this.repo}`,
        release.tag,
        manifestName,
        tempDir
      );
      const manifestPath = path.join(tempDir, manifestName);
      await verifyGitHubAttestation(
        release.ghPath,
        manifestPath,
        this.owner,
        this.repo,
        this.channel
      );
      const stats = await fs.stat(manifestPath);
      if (!stats.isFile() || stats.size <= 0 || stats.size > MAX_MANIFEST_BYTES) {
        throw new Error('Downloaded update manifest has an invalid size');
      }
      return await fs.readFile(manifestPath, 'utf8');
    } finally {
      await fs.rm(tempDir, { recursive: true, force: true }).catch((error) => {
        log.warn(`Could not remove updater manifest directory: ${errorMessage(error, 'unknown')}`);
      });
    }
  }

  private async downloadWithGitHubCli(
    artifact: GitHubUpdateArtifact,
    tempDir: string,
    onProgress?: (percent: number) => void
  ): Promise<string> {
    if (!artifact.ghPath) {
      throw new Error('Authenticated GitHub CLI path is unavailable');
    }
    await downloadReleaseAsset(
      artifact.ghPath,
      `${this.owner}/${this.repo}`,
      artifact.releaseTag,
      artifact.assetName,
      tempDir
    );
    onProgress?.(75);
    const archivePath = path.join(tempDir, artifact.assetName);
    await verifyGitHubAttestation(
      artifact.ghPath,
      archivePath,
      this.owner,
      this.repo,
      this.channel
    );
    onProgress?.(90);
    return archivePath;
  }

  private async downloadWithHttps(
    artifact: GitHubUpdateArtifact,
    tempDir: string,
    onProgress?: (percent: number) => void
  ): Promise<string> {
    if (!artifact.downloadUrl || !isAllowedGitHubUrl(artifact.downloadUrl)) {
      throw new Error('Verified HTTPS update URL is unavailable');
    }
    const archivePath = path.join(tempDir, artifact.assetName);
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), DOWNLOAD_TIMEOUT_MS);
    let handle: fs.FileHandle | undefined;
    try {
      const response = await fetch(artifact.downloadUrl, {
        headers: githubHeaders(app.getVersion()),
        signal: controller.signal,
      });
      if (!response.ok || !response.body) {
        throw new Error(`Update download returned HTTP ${response.status}`);
      }
      ensureAllowedResponseUrl(response);
      const declaredLength = Number(response.headers.get('content-length') || '0');
      if (
        declaredLength > MAX_UPDATE_BYTES ||
        (declaredLength > 0 && declaredLength !== artifact.size)
      ) {
        throw new Error('Update download Content-Length does not match the verified manifest');
      }

      handle = await fs.open(archivePath, 'wx', 0o600);
      const reader = response.body.getReader();
      let downloaded = 0;
      while (true) {
        const { done, value } = await reader.read();
        if (done) {
          break;
        }
        downloaded += value.byteLength;
        if (downloaded > MAX_UPDATE_BYTES || downloaded > artifact.size) {
          throw new Error('Update download exceeded its verified size');
        }
        await handle.write(value);
        onProgress?.(Math.max(2, Math.min(85, Math.floor((downloaded / artifact.size) * 85))));
      }
      if (downloaded !== artifact.size) {
        throw new Error('Update download size does not match the verified manifest');
      }
      await handle.sync();
      await handle.close();
      handle = undefined;
      return archivePath;
    } finally {
      clearTimeout(timeout);
      await handle?.close().catch(() => undefined);
    }
  }
}

async function fetchManifest(url: string): Promise<string> {
  return fetchBoundedText(url, MAX_MANIFEST_BYTES);
}

async function fetchBoundedText(url: string, maxBytes: number): Promise<string> {
  if (!isAllowedGitHubUrl(url)) {
    throw new Error('Updater request URL is outside the allowed GitHub hosts');
  }
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), CHECK_TIMEOUT_MS);
  try {
    const response = await fetch(url, {
      headers: githubHeaders(app.getVersion()),
      signal: controller.signal,
    });
    if (!response.ok || !response.body) {
      throw new Error(`Updater request returned HTTP ${response.status}`);
    }
    ensureAllowedResponseUrl(response);
    const declaredLength = Number(response.headers.get('content-length') || '0');
    if (declaredLength > maxBytes) {
      throw new Error('Updater response exceeds its size limit');
    }
    const chunks: Uint8Array[] = [];
    const reader = response.body.getReader();
    let received = 0;
    while (true) {
      const { done, value } = await reader.read();
      if (done) {
        break;
      }
      received += value.byteLength;
      if (received > maxBytes) {
        throw new Error('Updater response exceeds its size limit');
      }
      chunks.push(value);
    }
    return Buffer.concat(chunks.map((chunk) => Buffer.from(chunk))).toString('utf8');
  } finally {
    clearTimeout(timeout);
  }
}

function ensureAllowedResponseUrl(response: Response): void {
  if (!isAllowedGitHubUrl(response.url)) {
    throw new Error('Updater response redirected outside the allowed GitHub hosts');
  }
}

function githubHeaders(version: string): Record<string, string> {
  return {
    Accept: 'application/vnd.github+json',
    'User-Agent': `Ponduin-Desktop/${version}`,
    'X-GitHub-Api-Version': '2022-11-28',
  };
}

async function findGitHubCli(): Promise<string> {
  const executableName = process.platform === 'win32' ? 'gh.exe' : 'gh';
  const candidates = [
    '/opt/homebrew/bin/gh',
    '/usr/local/bin/gh',
    '/usr/bin/gh',
    ...(process.env.PATH || '')
      .split(path.delimiter)
      .filter((directory) => path.isAbsolute(directory))
      .map((directory) => path.join(directory, executableName)),
  ];
  if (process.platform === 'win32' && process.env.LOCALAPPDATA) {
    candidates.unshift(path.join(process.env.LOCALAPPDATA, 'Programs', 'GitHub CLI', 'gh.exe'));
  }
  for (const candidate of [...new Set(candidates)]) {
    try {
      await fs.access(candidate, fsConstants.X_OK);
      return await fs.realpath(candidate);
    } catch {
      // Try the next absolute candidate.
    }
  }
  throw new Error(
    'This private main update requires an authenticated GitHub CLI (`gh auth login`)'
  );
}

async function runGitHubCli(
  executable: string,
  args: string[],
  timeoutMs: number
): Promise<string> {
  return await new Promise((resolve, reject) => {
    execFile(
      executable,
      args,
      {
        encoding: 'utf8',
        timeout: timeoutMs,
        maxBuffer: 4 * 1024 * 1024,
        windowsHide: true,
        env: githubCliEnvironment(),
      },
      (error, stdout) => {
        if (error) {
          reject(new Error('Authenticated GitHub CLI operation failed'));
        } else {
          resolve(stdout);
        }
      }
    );
  });
}

function githubCliEnvironment(): Record<string, string | undefined> {
  const allowed = [
    'PATH',
    'HOME',
    'USERPROFILE',
    'LOCALAPPDATA',
    'APPDATA',
    'XDG_CONFIG_HOME',
    'GH_CONFIG_DIR',
    'GH_HOST',
    'GH_TOKEN',
    'GITHUB_TOKEN',
    'SystemRoot',
    'WINDIR',
  ];
  const environment: Record<string, string | undefined> = {
    GH_PROMPT_DISABLED: '1',
    GH_PAGER: 'cat',
    NO_COLOR: '1',
  };
  for (const name of allowed) {
    if (process.env[name]) {
      environment[name] = process.env[name];
    }
  }
  return environment;
}

async function downloadReleaseAsset(
  ghPath: string,
  repository: string,
  tag: string,
  assetName: string,
  destination: string
): Promise<void> {
  await runGitHubCli(
    ghPath,
    [
      'release',
      'download',
      tag,
      '--repo',
      repository,
      '--pattern',
      assetName,
      '--dir',
      destination,
      '--clobber',
    ],
    DOWNLOAD_TIMEOUT_MS
  );
  const downloaded = path.join(destination, assetName);
  const stats = await fs.stat(downloaded);
  if (!stats.isFile() || stats.size <= 0 || stats.size > MAX_UPDATE_BYTES) {
    throw new Error('GitHub CLI downloaded an invalid update artifact');
  }
}

async function verifyGitHubAttestation(
  ghPath: string,
  artifactPath: string,
  owner: string,
  repo: string,
  channel: UpdateChannel
): Promise<void> {
  const workflow = channel === 'main' ? 'canary.yml' : 'release.yml';
  const args = [
    'attestation',
    'verify',
    artifactPath,
    '--repo',
    `${owner}/${repo}`,
    '--signer-workflow',
    `${owner}/${repo}/.github/workflows/${workflow}`,
    '--deny-self-hosted-runners',
  ];
  if (channel === 'main') {
    args.push('--source-ref', 'refs/heads/main');
  }
  await runGitHubCli(ghPath, args, CHECK_TIMEOUT_MS);
}

async function verifyFileIntegrity(
  filePath: string,
  artifact: Pick<GitHubUpdateArtifact, 'size' | 'sha512'>
): Promise<void> {
  const stats = await fs.stat(filePath);
  if (!stats.isFile() || stats.size !== artifact.size || stats.size > MAX_UPDATE_BYTES) {
    throw new Error('Downloaded update size does not match the verified manifest');
  }
  const hash = createHash('sha512');
  for await (const chunk of createReadStream(filePath)) {
    hash.update(chunk as Buffer);
  }
  if (hash.digest('base64') !== artifact.sha512) {
    throw new Error('Downloaded update SHA-512 does not match the verified manifest');
  }
}

async function copyVerifiedArchive(
  source: string,
  artifact: GitHubUpdateArtifact,
  bundleName: string
): Promise<string> {
  const downloadsDir = path.join(os.homedir(), 'Downloads');
  await fs.mkdir(downloadsDir, { recursive: true });
  const extension = path.extname(artifact.assetName) || '.zip';
  const version = artifact.version.replace(/[^A-Za-z0-9._+-]/g, '_');
  const digestPrefix = Buffer.from(artifact.sha512, 'base64').toString('hex').slice(0, 12);
  const baseName = `${bundleName}-${version}-${process.platform}-${process.arch}-${digestPrefix}${extension}`;
  let destination = path.join(downloadsDir, baseName);
  if (await pathExists(destination)) {
    try {
      await verifyFileIntegrity(destination, artifact);
      return destination;
    } catch {
      destination = path.join(
        downloadsDir,
        `${path.basename(baseName, extension)}-${randomUUID()}${extension}`
      );
    }
  }

  const partial = `${destination}.part-${randomUUID()}`;
  try {
    await fs.copyFile(source, partial, fsConstants.COPYFILE_EXCL);
    await verifyFileIntegrity(partial, artifact);
    await fs.rename(partial, destination);
    return destination;
  } catch (error) {
    await fs.rm(partial, { force: true });
    throw error;
  }
}

async function pathExists(filePath: string): Promise<boolean> {
  try {
    await fs.access(filePath);
    return true;
  } catch {
    return false;
  }
}

export const githubUpdater = new GitHubUpdater();
