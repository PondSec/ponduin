import { describe, expect, it } from 'vitest';
import {
  genericFeedUrl,
  isAllowedGitHubUrl,
  isNewerVersion,
  parseUpdateManifest,
  releaseApiUrl,
  resolveUpdateChannel,
  selectManifestFile,
  updateAssetName,
  updateManifestName,
} from './updateSource';

const sha512 = Buffer.alloc(64, 7).toString('base64');

describe('updateSource', () => {
  it('uses the attested main release by default and keeps stable explicit', () => {
    expect(resolveUpdateChannel(undefined)).toBe('main');
    expect(resolveUpdateChannel('main')).toBe('main');
    expect(resolveUpdateChannel('stable')).toBe('stable');
    expect(resolveUpdateChannel('nightly')).toBe('stable');
    expect(releaseApiUrl('PondSec', 'ponduin', 'main')).toContain('/releases/tags/canary');
    expect(releaseApiUrl('PondSec', 'ponduin', 'stable').endsWith('/releases/latest')).toBe(true);
    expect(
      genericFeedUrl('PondSec', 'ponduin', 'main')?.endsWith('/releases/download/canary')
    ).toBe(true);
    expect(genericFeedUrl('PondSec', 'ponduin', 'stable')).toBeNull();
  });

  it('selects platform manifests and immutable asset names', () => {
    expect(updateManifestName('darwin')).toBe('latest-mac.yml');
    expect(updateManifestName('linux')).toBe('latest-linux.yml');
    expect(updateManifestName('win32')).toBe('latest.yml');
    expect(updateAssetName('darwin', 'arm64', 'Ponduin')).toBe('Ponduin-darwin-arm64.zip');
    expect(updateAssetName('darwin', 'x64', 'Ponduin')).toBe('Ponduin-darwin-x64.zip');
    expect(updateAssetName('win32', 'x64', 'Ponduin')).toBe('Ponduin-win32-x64.zip');
    expect(updateAssetName('linux', 'x64', 'Ponduin')).toBe('Ponduin-linux-x64.deb');
    expect(() => updateAssetName('linux', 'arm64', 'Ponduin')).toThrow();
    expect(() => updateAssetName('win32', 'arm64', 'Ponduin')).toThrow();
    expect(() => updateManifestName('freebsd')).toThrow();
  });

  it('parses integrity metadata and rejects traversal or malformed hashes', () => {
    const manifest = parseUpdateManifest(`
version: "1.44.1-main.42+abcdef0"
files:
  - url: "Ponduin-darwin-arm64.zip"
    sha512: "${sha512}"
    size: 1234
`);
    expect(selectManifestFile(manifest, 'ponduin-darwin-arm64.zip').size).toBe(1234);

    expect(() =>
      parseUpdateManifest(`
version: "1.44.1-main.42"
files:
  - url: "../Ponduin.zip"
    sha512: "${sha512}"
    size: 1234
`)
    ).toThrow();
    expect(() =>
      parseUpdateManifest(`
version: "1.44.1-main.42"
files:
  - url: "Ponduin.zip"
    sha512: "not-a-digest"
    size: 1234
`)
    ).toThrow();
    expect(() =>
      parseUpdateManifest(`
version: "1.44.1-main.42"
files:
  - url: "Ponduin\\n.zip"
    sha512: "${sha512}"
    size: 1234
`)
    ).toThrow();
  });

  it('compares monotonic main builds and restricts updater URLs', () => {
    expect(isNewerVersion('1.44.1-main.43+bbbbbbb', '1.44.1-main.42+aaaaaaa')).toBe(true);
    expect(isNewerVersion('1.44.1', '1.44.1-main.43+bbbbbbb')).toBe(true);
    expect(
      isAllowedGitHubUrl('https://github.com/PondSec/ponduin/releases/download/canary/a.zip')
    ).toBe(true);
    expect(isAllowedGitHubUrl('https://objects.githubusercontent.com/private/a.zip')).toBe(true);
    expect(isAllowedGitHubUrl('http://github.com/PondSec/ponduin/a.zip')).toBe(false);
    expect(isAllowedGitHubUrl('https://token@example.com/a.zip')).toBe(false);
    expect(isAllowedGitHubUrl('https://example.com/a.zip')).toBe(false);
  });
});
