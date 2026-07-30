import { execFile } from 'child_process';
import { readFile } from 'fs/promises';
import { join } from 'path';
import { promisify } from 'util';
import { expect, test } from './fixtures';

const workingDir = process.env.PONDUIN_E2E_WORKING_DIR;
const enabled = process.env.PONDUIN_LIVE_AGENT_E2E === '1' && Boolean(workingDir);
const execFileAsync = promisify(execFile);

test.setTimeout(30 * 60_000);

test.describe('live native coding agent repair GUI', () => {
  test.skip(!enabled, 'set PONDUIN_LIVE_AGENT_E2E=1 and PONDUIN_E2E_WORKING_DIR to run local-model E2E');

  test('repairs a real failing Python unittest before reporting completion', async ({ ponduinPage }) => {
    const root = workingDir!;
    const input = ponduinPage.locator('[data-testid="chat-input"]');
    await input.waitFor({ state: 'visible', timeout: 60_000 });
    await input.fill(
      'Arbeite autonom im aktuellen Projekt. Erstelle hello.py mit greet(name), die exakt "Hello, <name>!" zurückgibt, sowie test_hello.py mit unittest. Führe python3 -m unittest -v aus. Wenn der Test fehlschlägt, analysiere die konkrete Ausgabe, repariere die Ursache und führe ihn erneut erfolgreich aus. Fasse erst nach echter erfolgreicher Validierung zusammen.'
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
          return 'stdout' in result ? result.stdout : '';
        },
        { timeout: 300_000 }
      )
      .toContain('OK');
  });
});
