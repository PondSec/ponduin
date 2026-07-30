import { compareVersions, validate } from 'compare-versions';
import * as yaml from 'yaml';

export type UpdateChannel = 'stable' | 'main';

export interface UpdateManifestFile {
  url: string;
  sha512: string;
  size: number;
}

export interface UpdateManifest {
  version: string;
  files: UpdateManifestFile[];
}

export const DEFAULT_UPDATE_CHANNEL: UpdateChannel = 'main';

export function resolveUpdateChannel(value: string | undefined): UpdateChannel {
  const normalized = value?.trim().toLowerCase();
  if (!normalized) {
    return DEFAULT_UPDATE_CHANNEL;
  }
  if (normalized === 'stable' || normalized === 'main') {
    return normalized;
  }
  return 'stable';
}

export function releaseTag(channel: UpdateChannel): string | null {
  return channel === 'main' ? 'canary' : null;
}

export function releaseApiUrl(owner: string, repo: string, channel: UpdateChannel): string {
  const tag = releaseTag(channel);
  return tag
    ? `https://api.github.com/repos/${owner}/${repo}/releases/tags/${tag}`
    : `https://api.github.com/repos/${owner}/${repo}/releases/latest`;
}

export function genericFeedUrl(owner: string, repo: string, channel: UpdateChannel): string | null {
  const tag = releaseTag(channel);
  return tag ? `https://github.com/${owner}/${repo}/releases/download/${tag}` : null;
}

export function updateManifestName(platform: string): string {
  if (platform === 'darwin') {
    return 'latest-mac.yml';
  }
  if (platform === 'linux') {
    return 'latest-linux.yml';
  }
  if (platform === 'win32') {
    return 'latest.yml';
  }
  throw new Error(`Unsupported update platform: ${platform}`);
}

export function updateAssetName(platform: string, arch: string, bundleName: string): string {
  if (platform === 'darwin') {
    if (arch === 'arm64' || arch === 'x64') {
      return `${bundleName}-darwin-${arch}.zip`;
    }
    throw new Error(`Unsupported macOS update architecture: ${arch}`);
  }
  if (platform === 'win32') {
    if (arch !== 'x64') {
      throw new Error(`Unsupported Windows update architecture: ${arch}`);
    }
    return `${bundleName}-win32-x64.zip`;
  }
  if (platform === 'linux') {
    if (arch !== 'x64') {
      throw new Error(`Unsupported Linux update architecture: ${arch}`);
    }
    return `${bundleName}-linux-x64.deb`;
  }
  throw new Error(`Unsupported update platform: ${platform}`);
}

export function parseUpdateManifest(source: string): UpdateManifest {
  const parsed: unknown = yaml.parse(source);
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error('Update manifest must be an object');
  }
  const candidate = parsed as Record<string, unknown>;
  if (typeof candidate.version !== 'string' || !validate(candidate.version)) {
    throw new Error('Update manifest contains an invalid semantic version');
  }
  if (!Array.isArray(candidate.files) || candidate.files.length === 0) {
    throw new Error('Update manifest does not contain any files');
  }
  const files = candidate.files.map((entry): UpdateManifestFile => {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) {
      throw new Error('Update manifest contains an invalid file entry');
    }
    const file = entry as Record<string, unknown>;
    if (
      typeof file.url !== 'string' ||
      !isSafeAssetName(file.url) ||
      typeof file.sha512 !== 'string' ||
      !isSha512Base64(file.sha512) ||
      typeof file.size !== 'number' ||
      !Number.isSafeInteger(file.size) ||
      file.size <= 0
    ) {
      throw new Error('Update manifest contains invalid file integrity metadata');
    }
    return {
      url: file.url,
      sha512: file.sha512,
      size: file.size,
    };
  });
  return { version: candidate.version, files };
}

export function selectManifestFile(
  manifest: UpdateManifest,
  assetName: string
): UpdateManifestFile {
  const selected = manifest.files.find(
    (file) => file.url.toLowerCase() === assetName.toLowerCase()
  );
  if (!selected) {
    throw new Error(`Update manifest does not contain ${assetName}`);
  }
  return selected;
}

export function isNewerVersion(latestVersion: string, currentVersion: string): boolean {
  if (!validate(latestVersion) || !validate(currentVersion)) {
    throw new Error('Cannot compare invalid update versions');
  }
  return compareVersions(latestVersion, currentVersion) > 0;
}

export function isAllowedGitHubUrl(value: string): boolean {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return false;
  }
  if (url.protocol !== 'https:' || url.username || url.password) {
    return false;
  }
  const host = url.hostname.toLowerCase();
  return (
    host === 'github.com' ||
    host === 'api.github.com' ||
    host === 'objects.githubusercontent.com' ||
    host === 'github-releases.githubusercontent.com' ||
    host.endsWith('.githubusercontent.com')
  );
}

function isSafeAssetName(value: string): boolean {
  return (
    value.length > 0 &&
    value.length <= 255 &&
    !value.includes('/') &&
    !value.includes('\\') &&
    value !== '.' &&
    value !== '..' &&
    !value.startsWith('-') &&
    !containsControlCharacter(value)
  );
}

function containsControlCharacter(value: string): boolean {
  return Array.from(value).some((character) => {
    const codePoint = character.codePointAt(0);
    return codePoint !== undefined && (codePoint <= 31 || codePoint === 127);
  });
}

function isSha512Base64(value: string): boolean {
  try {
    const decoded = Buffer.from(value, 'base64');
    return decoded.length === 64 && decoded.toString('base64') === value;
  } catch {
    return false;
  }
}
