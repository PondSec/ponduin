import { describe, expect, it } from 'vitest';
import { getTextAndImageContent, type Message } from './message';

describe('getTextAndImageContent', () => {
  it('ignores malformed text blocks instead of rendering undefined', () => {
    const message = {
      role: 'assistant',
      content: [{ type: 'text' }],
    } as unknown as Message;

    expect(getTextAndImageContent(message).textContent).toBe('');
  });
});
