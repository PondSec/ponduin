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
  steps: WorkflowProgressStep[];
};

type CodingToolCall = {
  arguments: Record<string, unknown>;
  id: string;
  name: string;
};

const STEP_LABELS = [
  'Arbeitsbereich analysieren',
  'Plan festlegen',
  'Änderung umsetzen',
  'Prüfen und reviewen',
  'Ergebnis abschließen',
] as const;

export function getWorkflowProgress(messages: Message[]): WorkflowProgress | undefined {
  const successfulToolIds = successfulToolResponseIds(messages);
  const calls = codingToolCalls(messages, successfulToolIds);
  const latestWorkflowStart = calls.map((call) => call.name).lastIndexOf('workflow_start');
  const workflowCalls = latestWorkflowStart >= 0 ? calls.slice(latestWorkflowStart) : calls;

  if (workflowCalls.length === 0) {
    return undefined;
  }

  const names = new Set(workflowCalls.map((call) => call.name));
  const plan = [...workflowCalls].reverse().find((call) => call.name === 'workflow_set_plan');
  const changedFiles = changedFileCount(workflowCalls);
  const currentStep = currentStepFor(names);
  const lineCounts = diffLineCounts(messages, workflowCalls);
  const planDetail = plan ? planSummary(plan.arguments) : undefined;
  const steps = STEP_LABELS.map((label, index) => ({
    label,
    detail: stepDetail(index + 1, planDetail, changedFiles),
    status: stepStatus(index + 1, currentStep),
  }));

  return {
    currentStep,
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

function currentStepFor(names: Set<string>): number {
  if (names.has('workflow_complete')) {
    return 5;
  }
  if (names.has('review_changes') || names.has('workflow_transition')) {
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

function stepStatus(step: number, currentStep: number): WorkflowProgressStep['status'] {
  if (step < currentStep || currentStep === 5) {
    return 'complete';
  }
  return step === currentStep ? 'active' : 'pending';
}

function stepDetail(step: number, planDetail: string | undefined, changedFiles: number): string {
  switch (step) {
    case 1:
      return 'Relevante Dateien und Projekthinweise werden gelesen.';
    case 2:
      return planDetail ?? 'Der Agent legt Ziel, Dateien und Prüfungen fest.';
    case 3:
      return changedFiles > 0
        ? `${changedFiles} Datei${changedFiles === 1 ? ' wurde' : 'en wurden'} geändert.`
        : 'Die geplante Änderung wird versionsgesichert angewendet.';
    case 4:
      return 'Tests, Validierung und Änderungsreview laufen mit aktuellen Ergebnissen.';
    default:
      return 'Der Agent schließt erst nach erfolgreicher Prüfung ab.';
  }
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

function planSummary(argumentsValue: Record<string, unknown>): string | undefined {
  const plan = record(argumentsValue.plan);
  const intendedChange = argumentsValue.intended_change ?? plan?.intended_changes;
  if (typeof intendedChange === 'string' && intendedChange.trim()) {
    return intendedChange.trim();
  }
  if (Array.isArray(intendedChange)) {
    const changes = intendedChange.filter((item): item is string => typeof item === 'string');
    if (changes.length > 0) {
      return changes.join(' ');
    }
  }
  return undefined;
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
