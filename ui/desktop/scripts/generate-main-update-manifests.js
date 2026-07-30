#!/usr/bin/env node

const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');

function usage() {
  console.error(
    'Usage: node scripts/generate-main-update-manifests.js --version <version> [--directory <path>]'
  );
}

function parseArgs(argv) {
  const args = {
    directory: process.cwd(),
    version: '',
  };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--version') {
      args.version = argv[++i] || '';
    } else if (arg === '--directory') {
      args.directory = argv[++i] || '';
    } else {
      usage();
      process.exit(1);
    }
  }

  if (!args.version || !args.directory) {
    usage();
    process.exit(1);
  }

  args.version = args.version.replace(/^v/, '');
  args.directory = path.resolve(args.directory);
  return args;
}

function requireFile(filePath) {
  const stats = fs.statSync(filePath);
  if (!stats.isFile() || stats.size <= 0) {
    throw new Error(`Missing or empty release artifact: ${filePath}`);
  }
  return stats;
}

function copyReleaseArtifact(directory, sourceName, updateName) {
  const sourcePath = path.join(directory, sourceName);
  const targetPath = path.join(directory, updateName);
  requireFile(sourcePath);
  if (path.resolve(sourcePath) !== path.resolve(targetPath)) {
    fs.copyFileSync(sourcePath, targetPath, fs.constants.COPYFILE_EXCL);
  }
  return targetPath;
}

function findStandardDeb(directory) {
  const candidates = fs
    .readdirSync(directory)
    .filter((name) => name.endsWith('.deb') && !name.endsWith('-vulkan.deb'));
  if (candidates.length !== 1) {
    throw new Error(
      `Expected exactly one standard Linux .deb artifact, found ${candidates.length}`
    );
  }
  return candidates[0];
}

function sha512(filePath) {
  const hash = crypto.createHash('sha512');
  hash.update(fs.readFileSync(filePath));
  return hash.digest('base64');
}

function yamlString(value) {
  return JSON.stringify(value);
}

function manifestEntry(filePath) {
  const stats = requireFile(filePath);
  return {
    url: path.basename(filePath),
    sha512: sha512(filePath),
    size: stats.size,
  };
}

function writeManifest(directory, name, version, files) {
  const entries = files.map(manifestEntry);
  const manifest = [
    `version: ${yamlString(version)}`,
    'files:',
    ...entries.flatMap((entry) => [
      `  - url: ${yamlString(entry.url)}`,
      `    sha512: ${yamlString(entry.sha512)}`,
      `    size: ${entry.size}`,
    ]),
    `path: ${yamlString(entries[0].url)}`,
    `sha512: ${yamlString(entries[0].sha512)}`,
    `releaseDate: ${yamlString(new Date().toISOString())}`,
    '',
  ].join('\n');

  fs.writeFileSync(path.join(directory, name), manifest, { flag: 'wx' });
}

function generate({ directory, version }) {
  const macArm = copyReleaseArtifact(directory, 'Ponduin.zip', 'Ponduin-darwin-arm64.zip');
  const macIntel = copyReleaseArtifact(
    directory,
    'Ponduin_intel_mac.zip',
    'Ponduin-darwin-x64.zip'
  );
  const windows = path.join(directory, 'Ponduin-win32-x64.zip');
  requireFile(windows);
  const linux = copyReleaseArtifact(directory, findStandardDeb(directory), 'Ponduin-linux-x64.deb');

  writeManifest(directory, 'latest-mac.yml', version, [macArm, macIntel]);
  writeManifest(directory, 'latest.yml', version, [windows]);
  writeManifest(directory, 'latest-linux.yml', version, [linux]);
}

try {
  generate(parseArgs(process.argv.slice(2)));
} catch (error) {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
}
