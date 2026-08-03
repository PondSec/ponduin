import type { Message, ToolRequest, ToolResponse } from '../types/message';

export type WorkflowProgressStep = {
  detail: string;
  label: string;
  status: 'active' | 'complete' | 'pending';
};

export type WorkflowProgress = {
  additions?: number;
  changedFiles: number;
  currentStep: number;
  deletions?: number;
  phaseCount: number;
  steps: WorkflowProgressStep[];
};

const WORKFLOW_PHASE_COUNT = 5;

type CodingToolCall = {
  arguments: Record<string, unknown>;
  id: string;
  name: string;
};

export function getWorkflowProgress(messages: Message[]): WorkflowProgress | undefined {
  const successfulToolIds = successfulToolResponseIds(messages);
  const calls = codingToolCalls(messages, successfulToolIds);
  const latestWorkflowStart = calls.map((call) => call.name).lastIndexOf('workflow_start');
  const workflowCalls = latestWorkflowStart >= 0 ? calls.slice(latestWorkflowStart) : calls;

  if (workflowCalls.length === 0) {
    return undefined;
  }

  const plan = [...workflowCalls].reverse().find((call) => call.name === 'workflow_set_plan');
  if (!plan) {
    return undefined;
  }
  const planSteps = concretePlanSteps(plan.arguments);
  if (planSteps.length === 0) {
    return undefined;
  }

  const changedFiles = changedFileCount(workflowCalls);
  const currentStep = currentStepFor(workflowCalls);
  const lineCounts = diffLineCounts(messages, workflowCalls);
  const steps = planSteps.map((label, index, all) => ({
    label,
    detail: stepDetail(index, all.length, currentStep, changedFiles),
    status: stepStatus(index, all.length, currentStep),
  }));

  return {
    currentStep,
    phaseCount: WORKFLOW_PHASE_COUNT,
    changedFiles,
    steps,
    ...(lineCounts ? lineCounts : {}),
  };
}

function successfulToolResponseIds(messages: Message[]): Set<string> {
  const ids = new Set<string>();
  for (const message of messages) {
    for (const content of message.content) {
      if (content.type === 'toolResponse' && toolResponseSucceeded(content)) {
        ids.add(content.id);
      }
    }
  }
  return ids;
}

function codingToolCalls(messages: Message[], successfulToolIds: Set<string>): CodingToolCall[] {
  const calls: CodingToolCall[] = [];
  for (const message of messages) {
    for (const content of message.content) {
      if (content.type !== 'toolRequest' || !successfulToolIds.has(content.id)) {
        continue;
      }
      const call = codingToolCall(content);
      if (call) {
        calls.push(call);
      }
    }
  }
  return calls;
}

function codingToolCall(request: ToolRequest): CodingToolCall | undefined {
  const value = record(request.toolCall.value);
  if (!value || typeof value.name !== 'string' || !value.name.startsWith('coding__')) {
    return undefined;
  }
  return {
    id: request.id,
    name: value.name.slice('coding__'.length),
    arguments: record(value.arguments) ?? {},
  };
}

function toolResponseSucceeded(response: ToolResponse): boolean {
  return response.toolResult.status === 'success';
}

function currentStepFor(calls: CodingToolCall[]): number {
  const names = new Set(calls.map((call) => call.name));
  const transitions = new Set(
    calls
      .filter((call) => call.name === 'workflow_transition')
      .map((call) => call.arguments.transition)
  );
  if (names.has('workflow_complete')) {
    return 5;
  }
  if (names.has('review_changes') || transitions.has('begin_review')) {
    return 4;
  }
  if (names.has('run_validation') || names.has('run_process') || transitions.has('begin_validation')) {
    return 4;
  }
  if (names.has('apply_changes')) {
    return 3;
  }
  if (names.has('workflow_set_plan')) {
    return 2;
  }
  return 1;
}

function stepStatus(
  index: number,
  stepCount: number,
  currentStep: number
): WorkflowProgressStep['status'] {
  if (currentStep === 5) {
    return 'complete';
  }
  const activeIndex = Math.min(Math.max(currentStep - 1, 0), stepCount - 1);
  if (index < activeIndex) {
    return 'complete';
  }
  return index === activeIndex ? 'active' : 'pending';
}

function stepDetail(
  index: number,
  stepCount: number,
  currentStep: number,
  changedFiles: number
): string {
  const activeIndex = Math.min(Math.max(currentStep - 1, 0), stepCount - 1);
  if (currentStep === 5) {
    return 'Abgeschlossen und durch den Workflow belegt.';
  }
  if (index !== activeIndex) {
    return '';
  }
  if (currentStep === 1) {
    return 'Der Agent erfasst die dafür notwendigen Projektinformationen.';
  }
  if (currentStep === 3) {
    return changedFiles > 0
      ? `${changedFiles} Datei${changedFiles === 1 ? ' wurde' : 'en wurden'} bereits geändert.`
      : 'Die Änderung wird vorbereitet.';
  }
  if (currentStep === 4) {
    return 'Die aktuelle Revision wird geprüft.';
  }
  return 'Der konkrete Plan wurde akzeptiert.';
}

function changedFileCount(calls: CodingToolCall[]): number {
  const paths = new Set<string>();
  for (const call of calls) {
    if (call.name !== 'apply_changes') {
      continue;
    }
    const changes = Array.isArray(call.arguments.changes) ? call.arguments.changes : [];
    for (const change of changes) {
      const entry = record(change);
      for (const key of ['path', 'destination']) {
        const path = entry?.[key];
        if (typeof path === 'string' && path.length > 0) {
          paths.add(path);
        }
      }
    }
  }
  return paths.size;
}

function concretePlanSteps(argumentsValue: Record<string, unknown>): string[] {
  const plan = record(argumentsValue.plan);
  return firstNonEmptyStringArray(
    argumentsValue.plan_steps,
    argumentsValue.steps,
    plan?.plan_steps,
    plan?.steps,
    plan?.intended_changes,
    argumentsValue.intended_change
  );
}

function firstNonEmptyStringArray(...values: unknown[]): string[] {
  for (const value of values) {
    const items = Array.isArray(value)
      ? value.filter(
          (item): item is string => typeof item === 'string' && item.trim().length > 0
        )
      : typeof value === 'string' && value.trim()
        ? [value]
        : [];
    if (items.length > 0) {
      return [...new Set(items.map((item) => item.trim()))];
    }
  }
  return [];
}

function diffLineCounts(
  messages: Message[],
  calls: CodingToolCall[]
): { additions: number; deletions: number } | undefined {
  const diffIds = new Set(calls.filter((call) => call.name === 'git_diff').map((call) => call.id));
  if (diffIds.size === 0) {
    return undefined;
  }

  let additions = 0;
  let deletions = 0;
  let sawDiff = false;
  for (const message of messages) {
    for (const content of message.content) {
      if (content.type !== 'toolResponse' || !diffIds.has(content.id)) {
        continue;
      }
      const diff = textValues(content.metadata).join('\n');
      for (const line of diff.split('\n')) {
        if (line.startsWith('+++') || line.startsWith('---')) {
          continue;
        }
        if (line.startsWith('+')) {
          additions += 1;
          sawDiff = true;
        } else if (line.startsWith('-')) {
          deletions += 1;
          sawDiff = true;
        }
      }
    }
  }
  return sawDiff ? { additions, deletions } : undefined;
}

function textValues(value: unknown): string[] {
  if (typeof value === 'string') {
    return [value];
  }
  if (Array.isArray(value)) {
    return value.flatMap(textValues);
  }
  const object = record(value);
  if (!object) {
    return [];
  }
  return Object.values(object).flatMap(textValues);
}

function record(value: unknown): Record<string, unknown> | undefined {
  return typeof value === 'object' && value !== null
    ? (value as Record<string, unknown>)
    : undefined;
}
