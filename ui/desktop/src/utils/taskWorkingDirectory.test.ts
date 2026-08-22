import { describe, expect, it } from 'vitest';
import { taskWorkingDirectory } from './taskWorkingDirectory';

describe('taskWorkingDirectory', () => {
  it.each(['workspace', 'user', 'computer'] as const)(
    'keeps the selected project directory with %s access',
    (accessScope) => {
      expect(taskWorkingDirectory('/projects/example', accessScope)).toBe('/projects/example');
    }
  );
});
