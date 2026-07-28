import { createRequire } from "node:module";
import { dirname, join } from "node:path";

const PLATFORMS: Record<string, string> = {
  "darwin-arm64": "@pondsec/ponduin-binary-darwin-arm64",
  "darwin-x64": "@pondsec/ponduin-binary-darwin-x64",
  "linux-arm64": "@pondsec/ponduin-binary-linux-arm64",
  "linux-x64": "@pondsec/ponduin-binary-linux-x64",
  "win32-x64": "@pondsec/ponduin-binary-win32-x64",
};

/**
 * Resolves the path to the ponduin binary.
 *
 * Resolution order:
 *   1. `PONDUIN_BINARY` environment variable (explicit override)
 *   2. Platform-specific `@pondsec/ponduin-binary-*` optional dependency
 *
 * @throws if no binary can be found
 */
export function resolvePonduinBinary(): string {
  const envBinary = process.env.PONDUIN_BINARY;
  if (envBinary) return envBinary;

  const key = `${process.platform}-${process.arch}`;
  const pkg = PLATFORMS[key];
  if (!pkg) {
    throw new Error(
      `No ponduin binary available for ${key}. Set PONDUIN_BINARY to the path of a ponduin binary.`,
    );
  }

  try {
    const require = createRequire(import.meta.url);
    const pkgDir = dirname(require.resolve(`${pkg}/package.json`));
    const binName = process.platform === "win32" ? "ponduin.exe" : "ponduin";
    return join(pkgDir, "bin", binName);
  } catch {
    throw new Error(
      `ponduin binary package ${pkg} is not installed. Set PONDUIN_BINARY or install the native package.`,
    );
  }
}
