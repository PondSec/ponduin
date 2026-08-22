import type { FileAccessScope } from './settings';

export function taskWorkingDirectory(
  requestedDirectory: string,
  _accessScope: FileAccessScope
): string {
  return requestedDirectory;
}
