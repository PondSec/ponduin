import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import type { Message } from '../types/message';
import WorkflowProgressPill from './WorkflowProgressPill';

class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}

beforeAll(() => {
  vi.stubGlobal('ResizeObserver', ResizeObserverStub);
});

afterAll(() => {
  vi.unstubAllGlobals();
});

function workflowMessages(): Message[] {
  const tool = (id: string, name: string, argumentsValue: Record<string, unknown> = {}) => ({
    type: 'toolRequest' as const,
    id,
    toolCall: { value: { name: `coding__${name}`, arguments: argumentsValue } },
  });
  const result = (id: string, rawOutput?: string) => ({
    type: 'toolResponse' as const,
    id,
    toolResult: { status: 'success' },
    ...(rawOutput ? { metadata: { rawOutput } } : {}),
  });

  return [
    {
      role: 'assistant',
      created: 1,
      metadata: { userVisible: true, agentVisible: true },
      content: [
        tool('start', 'workflow_start'),
        result('start'),
        tool('plan', 'workflow_set_plan', { intended_change: 'Normalize labels to lowercase.' }),
        result('plan'),
        tool('apply', 'apply_changes', {
          changes: [{ operation: 'write', path: 'lib.rs', content: 'updated' }],
        }),
        result('apply'),
        tool('diff', 'git_diff'),
        result('diff', '--- a/lib.rs\n+++ b/lib.rs\n-old\n+new\n'),
        tool('complete', 'workflow_complete'),
        result('complete'),
      ],
    },
  ];
}

describe('WorkflowProgressPill', () => {
  it('shows live workflow progress and concrete steps on hover', async () => {
    const user = userEvent.setup();
    render(
      <WorkflowProgressPill messages={workflowMessages()} progressMessage="Running final checks" />
    );

    const trigger = screen.getByRole('button', { name: 'Workflow, Schritt 5 von 5' });
    expect(trigger).toHaveTextContent('Schritt 5/5');
    expect(trigger).toHaveTextContent('1 Datei geändert');
    expect(trigger).toHaveTextContent('+1');
    expect(trigger).toHaveTextContent('-1');

    await user.hover(trigger);

    const tooltip = within(await screen.findByRole('tooltip'));
    expect(tooltip.getByText(/Plan festlegen/)).toBeInTheDocument();
    expect(tooltip.getByText('Normalize labels to lowercase.')).toBeInTheDocument();
    expect(tooltip.getByText('Running final checks')).toBeInTheDocument();
  });
});
