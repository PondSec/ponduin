import { execFile } from 'child_process';
import { readFile, writeFile } from 'fs/promises';
import { join } from 'path';
import { promisify } from 'util';
import { expect, test } from './fixtures';

const workingDir = process.env.PONDUIN_E2E_WORKING_DIR;
const enabled = process.env.PONDUIN_LIVE_AGENT_E2E === '1' && Boolean(workingDir);
const execFileAsync = promisify(execFile);

test.setTimeout(30 * 60_000);

test.describe('live native coding agent repair GUI', () => {
  test.skip(
    !enabled,
    'set PONDUIN_LIVE_AGENT_E2E=1 and PONDUIN_E2E_WORKING_DIR to run local-model E2E'
  );

  test('repairs a real failing Python unittest before reporting completion', async ({
    ponduinPage,
  }) => {
    const root = workingDir!;
    await writeFile(
      join(root, 'hello.py'),
      'def greet(name):\n    return f"Hi, {name}!"\n',
      'utf8'
    );
    await writeFile(
      join(root, 'test_hello.py'),
      [
        'import unittest',
        '',
        'from hello import greet',
        '',
        '',
        'class GreetTest(unittest.TestCase):',
        '    def test_greet(self):',
        '        self.assertEqual(greet("Ada"), "Hello, Ada!")',
        '',
        '',
        'if __name__ == "__main__":',
        '    unittest.main()',
        '',
      ].join('\n'),
      'utf8'
    );
    const input = ponduinPage.locator('[data-testid="chat-input"]');
    await input.waitFor({ state: 'visible', timeout: 60_000 });
    await input.fill(
      'Arbeite autonom im aktuellen Projekt. Diagnostiziere den bestehenden Fehler in hello.py und test_hello.py. Führe zuerst python3 -m unittest -v aus, analysiere bei Fehlern die konkrete Ausgabe, repariere die Ursache und führe denselben Test erneut erfolgreich aus. Fasse erst nach echter erfolgreicher Validierung zusammen.'
    );
    await input.press('Enter');
    await expect(input).toHaveValue('', { timeout: 30_000 });

    await expect
      .poll(async () => readFile(join(root, 'test_hello.py'), 'utf8').catch(() => ''), {
        timeout: 900_000,
      })
      .toContain('from hello import greet');
    await expect
      .poll(
        async () => {
          const result = await execFileAsync('python3', ['-m', 'unittest', '-v'], {
            cwd: root,
          }).catch((error) => error);
          return 'stdout' in result
            ? `${result.stdout}${result.stderr}`
            : `${errorOutput(result, 'stdout')}${errorOutput(result, 'stderr')}`;
        },
        { timeout: 300_000 }
      )
      .toContain('OK');
  });
});

function errorOutput(error: unknown, field: 'stdout' | 'stderr'): string {
  return typeof error === 'object' && error !== null && field in error
    ? String(error[field as keyof typeof error] ?? '')
    : '';
}
