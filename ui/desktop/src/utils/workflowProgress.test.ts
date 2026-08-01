import { describe, expect, it } from 'vitest';
import type { Message, ToolRequest, ToolResponse } from '../types/message';
import { getWorkflowProgress } from './workflowProgress';

function request(
  id: string,
  name: string,
  argumentsValue: Record<string, unknown> = {}
): ToolRequest & {
  type: 'toolRequest';
} {
  return {
    type: 'toolRequest',
    id,
    toolCall: { value: { name: `coding__${name}`, arguments: argumentsValue } },
  };
}

function response(id: string, rawOutput?: string): ToolResponse & { type: 'toolResponse' } {
  return {
    type: 'toolResponse',
    id,
    toolResult: { status: 'success' },
    ...(rawOutput ? { metadata: { rawOutput } } : {}),
  };
}

function messages(content: Message['content']): Message[] {
  return [
    {
      role: 'assistant',
      created: 1,
      metadata: { userVisible: true, agentVisible: true },
      content,
    },
  ];
}

describe('getWorkflowProgress', () => {
  it('derives plan progress, changed files, and diff lines from successful workflow calls', () => {
    const progress = getWorkflowProgress(
      messages([
        request('start', 'workflow_start'),
        response('start'),
        request('plan', 'workflow_set_plan', {
          intended_change: 'Normalize labels to lowercase.',
        }),
        response('plan'),
        request('apply', 'apply_changes', {
          changes: [{ operation: 'write', path: 'lib.rs', content: 'updated' }],
        }),
        response('apply'),
        request('validate', 'run_process'),
        response('validate'),
        request('diff', 'git_diff'),
        response('diff', '--- a/lib.rs\n+++ b/lib.rs\n-old\n+new\n'),
        request('review', 'review_changes'),
        response('review'),
        request('complete', 'workflow_complete'),
        response('complete'),
      ])
    );

    expect(progress).toMatchObject({
      currentStep: 5,
      changedFiles: 1,
      additions: 1,
      deletions: 1,
    });
    expect(progress?.steps.map((step) => step.status)).toEqual([
      'complete',
      'complete',
      'complete',
      'complete',
      'complete',
    ]);
    expect(progress?.steps[1].detail).toBe('Normalize labels to lowercase.');
  });

  it('shows the active analysis step before a plan has been accepted', () => {
    const progress = getWorkflowProgress(
      messages([request('start', 'workflow_start'), response('start')])
    );

    expect(progress?.currentStep).toBe(1);
    expect(progress?.steps[0].status).toBe('active');
    expect(progress?.steps.slice(1).every((step) => step.status === 'pending')).toBe(true);
  });

  it('only shows the latest workflow in a session', () => {
    const progress = getWorkflowProgress(
      messages([
        request('old-start', 'workflow_start'),
        response('old-start'),
        request('old-complete', 'workflow_complete'),
        response('old-complete'),
        request('new-start', 'workflow_start'),
        response('new-start'),
      ])
    );

    expect(progress?.currentStep).toBe(1);
  });
});
