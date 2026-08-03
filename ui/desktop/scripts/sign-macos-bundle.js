const { execFileSync } = require('node:child_process');
const { mkdtempSync, rmSync } = require('node:fs');
const { basename, join, resolve } = require('node:path');
const { tmpdir } = require('node:os');

const appBundle = process.argv[2];
const archive = process.argv[3];

if (!appBundle || !archive) {
  throw new Error('Usage: node scripts/sign-macos-bundle.js <app bundle> <archive>');
}

const bundlePath = resolve(appBundle);
const archivePath = resolve(archive);

if (process.platform !== 'darwin') {
  throw new Error('macOS bundle signing requires darwin');
}

execFileSync('xattr', ['-cr', bundlePath], {
  stdio: 'inherit',
});

const run = (command, args) => {
  execFileSync(command, args, {
    stdio: 'inherit',
  });
};

const createArchive = (sourceBundle) => {
  run('ditto', [
    '-c',
    '-k',
    '--sequesterRsrc',
    '--keepParent',
    sourceBundle,
    archivePath,
  ]);
};

if (process.env.APPLE_TEAM_ID) {
  run('codesign', ['--verify', '--deep', '--strict', bundlePath]);
  createArchive(bundlePath);
  process.exit(0);
}

const stagingDirectory = mkdtempSync(join(tmpdir(), 'ponduin-macos-bundle-'));
const stagedBundle = join(stagingDirectory, basename(bundlePath));

try {
  run('ditto', [bundlePath, stagedBundle]);
  run('xattr', ['-cr', stagedBundle]);
  run('codesign', ['--force', '--deep', '--sign', '-', stagedBundle]);
  run('codesign', ['--verify', '--deep', '--strict', stagedBundle]);
  createArchive(stagedBundle);
} finally {
  rmSync(stagingDirectory, { recursive: true, force: true });
}
