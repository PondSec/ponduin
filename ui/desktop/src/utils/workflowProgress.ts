import type { Message, ToolRequest, ToolResponse } from '../types/message';

export type WorkflowProgressStep = {
  detail: string;
  label: string;
  status: 'active' | 'complete' | 'pending';
};

export type WorkflowPlanDetails = {
  relevantFiles: string[];
  risks: string[];
  rollbackStrategy?: string;
  validationCommands: string[];
};

export type WorkflowProgress = {
  additions?: number;
  changedFiles: number;
  currentPhase: string;
  currentStep: number;
  deletions?: number;
  phaseCount: number;
  phases: WorkflowProgressStep[];
  plan: WorkflowPlanDetails;
  steps: WorkflowProgressStep[];
};

const WORKFLOW_PHASES = ['Analyse', 'Planung', 'Bearbeitung', 'Validierung', 'Review'];

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
  const planDetails = workflowPlanDetails(plan.arguments);
  const planSteps = concretePlanSteps(plan.arguments);
  if (planSteps.length === 0) {
    return undefined;
  }

  const changedFiles = changedFileCount(workflowCalls);
  const currentStep = currentStepFor(workflowCalls);
  const workflowCompleted = workflowCalls.some((call) => call.name === 'workflow_complete');
  const lineCounts = diffLineCounts(messages, workflowCalls);
  const phases = WORKFLOW_PHASES.map((label, index) => ({
    label,
    detail: phaseDetail(index, currentStep),
    status: phaseStatus(index, currentStep),
  }));
  const steps = planSteps.map((label, index) => ({
    label,
    detail: stepDetail(index, currentStep, changedFiles, workflowCompleted),
    status: stepStatus(index, currentStep, workflowCompleted),
  }));

  return {
    currentPhase: WORKFLOW_PHASES[currentStep - 1],
    currentStep,
    phaseCount: WORKFLOW_PHASES.length,
    changedFiles,
    phases,
    plan: planDetails,
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
    return 5;
  }
  if (names.has('run_validation') || names.has('run_process') || transitions.has('begin_validation')) {
    return 4;
  }
  if (
    names.has('apply_changes') ||
    names.has('write_file') ||
    transitions.has('begin_editing')
  ) {
    return 3;
  }
  if (names.has('workflow_set_plan')) {
    return 2;
  }
  return 1;
}

function phaseStatus(index: number, currentStep: number): WorkflowProgressStep['status'] {
  if (index + 1 < currentStep) {
    return 'complete';
  }
  return index + 1 === currentStep ? 'active' : 'pending';
}

function phaseDetail(index: number, currentStep: number): string {
  if (index + 1 !== currentStep) {
    return '';
  }
  return [
    'Relevante Dateien und Anforderungen werden erfasst.',
    'Der konkrete Umsetzungsplan wird festgelegt.',
    'Die geplante Änderung wird vorgenommen.',
    'Die Änderung wird mit einer echten Prüfung validiert.',
    'Die validierte Änderung wird abschließend geprüft.',
  ][index];
}

function stepStatus(
  index: number,
  currentStep: number,
  workflowCompleted: boolean
): WorkflowProgressStep['status'] {
  if (workflowCompleted) {
    return 'complete';
  }
  const actionPhase = index + 3;
  if (actionPhase < currentStep) {
    return 'complete';
  }
  return actionPhase === currentStep ? 'active' : 'pending';
}

function stepDetail(
  index: number,
  currentStep: number,
  changedFiles: number,
  workflowCompleted: boolean
): string {
  if (workflowCompleted) {
    return 'Abgeschlossen und durch den Workflow belegt.';
  }
  const actionPhase = index + 3;
  if (actionPhase !== currentStep) {
    return '';
  }
  if (actionPhase === 3) {
    return changedFiles > 0
      ? `${changedFiles} Datei${changedFiles === 1 ? ' wurde' : 'en wurden'} bereits geändert.`
      : 'Die Änderung wird vorbereitet.';
  }
  if (actionPhase === 4) {
    return 'Die definierte Validierung wird ausgeführt.';
  }
  return 'Die validierte Änderung wird abschließend geprüft.';
}

function changedFileCount(calls: CodingToolCall[]): number {
  const paths = new Set<string>();
  for (const call of calls) {
    if (call.name === 'apply_changes') {
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
    } else if (call.name === 'write_file' && typeof call.arguments.path === 'string') {
      paths.add(call.arguments.path);
    }
  }
  return paths.size;
}

function concretePlanSteps(argumentsValue: Record<string, unknown>): string[] {
  const plan = record(argumentsValue.plan);
  const explicitSteps = firstNonEmptyStringArray(
    argumentsValue.plan_steps,
    argumentsValue.steps,
    plan?.plan_steps,
    plan?.steps
  );
  const intendedChanges = firstNonEmptyStringArray(
    plan?.intended_changes,
    argumentsValue.intended_change
  );
  if (explicitSteps.length >= 3) {
    return explicitSteps;
  }
  if (intendedChanges.length >= 3) {
    return intendedChanges;
  }

  const relevantFiles = firstNonEmptyStringArray(plan?.relevant_files, argumentsValue.relevant_files);
  const scope = relevantFiles.length > 0 ? relevantFiles.join(', ') : 'the planned files';
  const change = intendedChanges[0] ?? 'Apply the planned behavior change.';
  const command = plannedValidationCommands(plan, argumentsValue)[0] ?? 'the planned validation command';

  return [
    `Change: ${change} (${scope}).`,
    `Validation: run ${command} and require a successful result.`,
    `Review: read ${scope} again and inspect only the retained planned change.`,
  ];
}

function workflowPlanDetails(argumentsValue: Record<string, unknown>): WorkflowPlanDetails {
  const plan = record(argumentsValue.plan);
  const relevantFiles = firstNonEmptyStringArray(plan?.relevant_files, argumentsValue.relevant_files);
  const risks = firstNonEmptyStringArray(plan?.risks, argumentsValue.risks);
  const rollbackStrategy = firstNonEmptyStringArray(
    plan?.rollback_strategy,
    argumentsValue.rollback_strategy
  )[0];
  const validationCommands = plannedValidationCommands(plan, argumentsValue);

  return { relevantFiles, risks, rollbackStrategy, validationCommands };
}

function plannedValidationCommands(
  plan: Record<string, unknown> | undefined,
  argumentsValue: Record<string, unknown>
): string[] {
  const checks = [
    ...(Array.isArray(plan?.tests) ? plan.tests : []),
    ...(Array.isArray(plan?.validation) ? plan.validation : []),
  ];
  const commands = checks.flatMap((check) => {
    const command = record(record(check)?.command);
    const program = command?.program;
    if (typeof program !== 'string' || program.length === 0) {
      return [];
    }
    const args = Array.isArray(command?.args)
      ? command.args.filter((arg): arg is string => typeof arg === 'string')
      : [];
    return [[program, ...args].join(' ')];
  });
  if (commands.length > 0) {
    return [...new Set(commands)];
  }

  const program = argumentsValue.validation_program;
  if (typeof program !== 'string' || program.length === 0) {
    return [];
  }
  const args = Array.isArray(argumentsValue.args)
    ? argumentsValue.args.filter((arg): arg is string => typeof arg === 'string')
    : [];
  return [[program, ...args].join(' ')];
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
