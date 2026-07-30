import { execFile } from 'child_process';
import { access, readFile } from 'fs/promises';
import { join } from 'path';
import { Page } from '@playwright/test';
import { promisify } from 'util';
import { expect, test } from './fixtures';

const workingDir = process.env.PONDUIN_E2E_WORKING_DIR;
const enabled = process.env.PONDUIN_LIVE_AGENT_E2E === '1' && Boolean(workingDir);
const execFileAsync = promisify(execFile);

test.setTimeout(30 * 60_000);

async function sendAndWaitForIdle(page: Page, prompt: string) {
  const input = page.locator('[data-testid="chat-input"]');
  await input.waitFor({ state: 'visible', timeout: 60_000 });
  await expect(input).toBeEnabled({ timeout: 60_000 });
  await input.fill(prompt);
  await input.press('Enter');
  await expect(input).toHaveValue('', { timeout: 30_000 });
  await page.waitForTimeout(750);
  await page
    .waitForSelector('[data-testid="loading-ponduin"]', { state: 'hidden', timeout: 240_000 })
    .catch(() => undefined);
}

test.describe('live native coding agent GUI', () => {
  test.skip(!enabled, 'set PONDUIN_LIVE_AGENT_E2E=1 and PONDUIN_E2E_WORKING_DIR to run local-model E2E');

  test('completes three consecutive autonomous coding requests in one desktop session', async ({ ponduinPage }) => {
    const root = workingDir!;
    await ponduinPage.waitForSelector('[data-testid="chat-input"]', { timeout: 60_000 });

    await sendAndWaitForIdle(
      ponduinPage,
      'Arbeite autonom im aktuellen Projekt. Erstelle hello.py mit greet(name), die exakt "Hello, <name>!" zurückgibt, sowie test_hello.py mit unittest. Führe python3 -m unittest -v aus. Nutze nur den aktuellen Projektordner und fasse kurz zusammen.'
    );
    await expect
      .poll(async () => readFile(join(root, 'hello.py'), 'utf8').catch(() => ''), { timeout: 480_000 })
      .toContain('def greet');
    await expect
      .poll(async () => readFile(join(root, 'test_hello.py'), 'utf8').catch(() => ''), { timeout: 480_000 })
      .toContain('unittest');

    await sendAndWaitForIdle(
      ponduinPage,
      'Nächster Auftrag im selben Projekt: verschiebe hello.py nach src/hello.py, passe test_hello.py an, erstelle docs/runbook.md mit dem Validierungsbefehl und führe den Test erneut aus. Wenn eine Planung nötig ist, plane und arbeite sie selbstständig ab.'
    );
    await expect
      .poll(async () => readFile(join(root, 'src', 'hello.py'), 'utf8').catch(() => ''), { timeout: 480_000 })
      .toContain('def greet');
    await expect
      .poll(async () => readFile(join(root, 'docs', 'runbook.md'), 'utf8').catch(() => ''), { timeout: 480_000 })
      .toContain('python3 -m unittest');

    await sendAndWaitForIdle(
      ponduinPage,
      'Dritter Auftrag: prüfe den Git-Status, stage und committe ausschließlich die Dateien, die du in dieser Sitzung erstellt oder geändert hast, mit einer klaren Commit-Nachricht. Fasse den Status und die ausgeführte Validierung zusammen.'
    );
    await access(join(root, '.git'));
    await expect
      .poll(
        async () => {
          const result = await execFileAsync('git', ['log', '-1', '--format=%s'], { cwd: root }).catch(
            () => ({ stdout: '' })
          );
          return result.stdout.trim();
        },
        { timeout: 480_000 }
      )
      .toMatch(/hello|agent|initial/i);
  });
});
