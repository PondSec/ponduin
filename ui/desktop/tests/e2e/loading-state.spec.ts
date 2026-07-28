import { test, expect } from './fixtures';

test.describe('Loading State', () => {
  test('shows a model placeholder while creating a new chat session', async ({ ponduinPage }) => {
    await ponduinPage.waitForSelector('[data-testid="chat-input"]', { timeout: 30000 });

    const chatInput = await ponduinPage.waitForSelector('[data-testid="chat-input"]');
    await chatInput.fill('Respond with the single word hello.');
    await chatInput.press('Enter');

    await ponduinPage.waitForSelector('[data-testid="loading-indicator"]', {
      state: 'visible',
      timeout: 10000,
    });

    const loadingModel = ponduinPage.locator('[data-testid="model-loading-state"]');
    await expect(loadingModel).toHaveText(/loading model/i, { timeout: 10000 });

    await ponduinPage.screenshot({
      path: test.info().outputPath('loading-state-fresh-session.png'),
      fullPage: true,
    });
  });
});
