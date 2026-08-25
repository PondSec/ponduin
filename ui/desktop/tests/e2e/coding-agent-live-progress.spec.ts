import { expect, test } from './fixtures';

const workingDir = process.env.PONDUIN_E2E_WORKING_DIR;
const enabled = process.env.PONDUIN_LIVE_AGENT_E2E === '1' && Boolean(workingDir);

test.setTimeout(30 * 60_000);

test.describe('live native coding agent progress GUI', () => {
  test.skip(
    !enabled,
    'set PONDUIN_LIVE_AGENT_E2E=1 and PONDUIN_E2E_WORKING_DIR to run local-model E2E'
  );

  test('shows real tool progress without synthetic workflow messages', async ({ ponduinPage }) => {
    const input = ponduinPage.locator('[data-testid="chat-input"]');
    const toolCalls = ponduinPage.getByTestId('tool-call-progress');
    const toolCallCount = await toolCalls.count();

    await input.waitFor({ state: 'visible', timeout: 60_000 });
    await input.fill(
      'Arbeite autonom im aktuellen Projekt. Erkläre vor dem ersten Tool-Aufruf kurz den nächsten Arbeitsschritt. Prüfe dann den Projektinhalt, erstelle progress_check.py mit einer Funktion add(a, b), führe einen passenden Python-Befehl aus und fasse erst nach der Validierung zusammen.'
    );
    await input.press('Enter');
    await expect(input).toHaveValue('', { timeout: 30_000 });
    const stopButton = ponduinPage.getByRole('button', { name: 'Stop', exact: true });
    await stopButton.waitFor({ state: 'visible', timeout: 30_000 });

    await expect(toolCalls).toHaveCount(toolCallCount + 1, {
      timeout: 240_000,
    });
    await expect(toolCalls.nth(toolCallCount)).toBeVisible({ timeout: 240_000 });
    await stopButton.waitFor({ state: 'hidden', timeout: 900_000 });
  });
});
