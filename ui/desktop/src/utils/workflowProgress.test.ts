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
          plan_steps: [
            'Normalize labels to lowercase.',
            'Run the library test suite.',
            'Review the changed implementation.',
          ],
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
    ]);
    expect(progress?.steps.map((step) => step.label)).toEqual([
      'Normalize labels to lowercase.',
      'Run the library test suite.',
      'Review the changed implementation.',
    ]);
    expect(progress?.plan).toMatchObject({
      relevantFiles: [],
      validationCommands: [],
    });
  });

  it('hides progress before a concrete plan has been accepted', () => {
    const progress = getWorkflowProgress(
      messages([request('start', 'workflow_start'), response('start')])
    );

    expect(progress).toBeUndefined();
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

    expect(progress).toBeUndefined();
  });

  it('counts native write_file calls and exposes the fixed five workflow phases', () => {
    const progress = getWorkflowProgress(
      messages([
        request('start', 'workflow_start'),
        response('start'),
        request('plan', 'workflow_set_plan', { plan_steps: ['Make the minimal change.'] }),
        response('plan'),
        request('write', 'write_file', { path: 'normalizer.py', content: 'updated' }),
        response('write'),
        request('validate', 'run_process'),
        response('validate'),
      ])
    );

    expect(progress).toMatchObject({
      currentPhase: 'Validierung',
      currentStep: 4,
      changedFiles: 1,
      phaseCount: 5,
    });
    expect(progress?.phases.map((phase) => phase.label)).toEqual([
      'Analyse',
      'Planung',
      'Bearbeitung',
      'Validierung',
      'Review',
    ]);
  });

  it('shows editing immediately after the accepted editing transition', () => {
    const progress = getWorkflowProgress(
      messages([
        request('start', 'workflow_start'),
        response('start'),
        request('plan', 'workflow_set_plan', { plan_steps: ['Make the minimal change.'] }),
        response('plan'),
        request('edit', 'workflow_transition', { transition: 'begin_editing' }),
        response('edit'),
      ])
    );

    expect(progress).toMatchObject({ currentPhase: 'Bearbeitung', currentStep: 3 });
  });

  it('keeps the plan scope, validation command, and safeguards visible', () => {
    const progress = getWorkflowProgress(
      messages([
        request('start', 'workflow_start'),
        response('start'),
        request('plan', 'workflow_set_plan', {
          plan: {
            relevant_files: ['normalizer.py'],
            intended_changes: ['Normalize values to lowercase.'],
            risks: ['Do not change test behavior.'],
            rollback_strategy: 'Restore the previous implementation if validation fails.',
            validation: [
              {
                command: { program: 'python3', args: ['-m', 'unittest', '-v'] },
              },
            ],
          },
        }),
        response('plan'),
      ])
    );

    expect(progress?.plan).toEqual({
      relevantFiles: ['normalizer.py'],
      risks: ['Do not change test behavior.'],
      rollbackStrategy: 'Restore the previous implementation if validation fails.',
      validationCommands: ['python3 -m unittest -v'],
    });
  });

  it('expands a compact one-line plan into concrete change, validation, and review steps', () => {
    const progress = getWorkflowProgress(
      messages([
        request('start', 'workflow_start'),
        response('start'),
        request('plan', 'workflow_set_plan', {
          relevant_files: ['normalizer.py'],
          intended_change: 'Normalize values to lowercase.',
          validation_program: 'python3',
          args: ['-m', 'unittest', '-v'],
        }),
        response('plan'),
      ])
    );

    expect(progress?.steps.map((step) => step.label)).toEqual([
      'Change: Normalize values to lowercase. (normalizer.py).',
      'Validation: run python3 -m unittest -v and require a successful result.',
      'Review: read normalizer.py again and inspect only the retained planned change.',
    ]);
  });
});
