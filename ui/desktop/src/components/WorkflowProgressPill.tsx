import { Check, Circle, LoaderCircle } from 'lucide-react';
import { useMemo } from 'react';
import type { Message } from '../types/message';
import { getWorkflowProgress } from '../utils/workflowProgress';
import { Tooltip, TooltipContent, TooltipTrigger } from './ui/Tooltip';

type WorkflowProgressPillProps = {
  active: boolean;
  messages: Message[];
  progressMessage?: string;
};

export default function WorkflowProgressPill({
  active,
  messages,
  progressMessage,
}: WorkflowProgressPillProps) {
  const progress = useMemo(() => getWorkflowProgress(messages), [messages]);

  if (!active || !progress) {
    return null;
  }

  return (
    <div className="relative z-20 mb-3 flex justify-center px-4" aria-live="polite">
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            aria-label={`Workflow: ${progress.currentPhase}, Schritt ${progress.currentStep} von ${progress.phaseCount}`}
            className="flex items-center gap-2 rounded-full border border-border-primary bg-background-secondary px-4 py-2 text-sm text-text-secondary shadow-sm transition-colors hover:bg-background-tertiary"
          >
            <LoaderCircle
              className="size-4 shrink-0 animate-spin text-blue-400"
              aria-hidden="true"
            />
            <span className="font-medium text-text-primary">{progress.currentPhase}</span>
            <span aria-hidden="true">·</span>
            <span>
              Schritt {progress.currentStep}/{progress.phaseCount}
            </span>
            <span aria-hidden="true">·</span>
            <span>
              {progress.changedFiles} Datei{progress.changedFiles === 1 ? '' : 'en'} geändert
            </span>
            {progress.additions !== undefined && (
              <span className="font-medium text-text-success">+{progress.additions}</span>
            )}
            {progress.deletions !== undefined && (
              <span className="font-medium text-red-400">-{progress.deletions}</span>
            )}
          </button>
        </TooltipTrigger>
        <TooltipContent
          side="top"
          className="w-[min(28rem,calc(100vw-2rem))] border border-border-primary bg-background-secondary p-3 text-left text-text-primary shadow-xl"
          arrowClassName="bg-background-secondary fill-background-secondary"
        >
          <p className="mb-2 text-xs font-semibold uppercase tracking-wide text-text-secondary">
            Workflow-Phasen
          </p>
          <ol className="mb-4 space-y-2">
            {progress.phases.map((phase, index) => (
              <li key={phase.label} className="flex items-start gap-2">
                <StepIcon status={phase.status} />
                <div>
                  <p className="font-medium">
                    {index + 1}. {phase.label}
                  </p>
                  {phase.detail && <p className="mt-0.5 text-text-secondary">{phase.detail}</p>}
                </div>
              </li>
            ))}
          </ol>
          <p className="mb-2 text-xs font-semibold uppercase tracking-wide text-text-secondary">
            Konkreter Plan
          </p>
          {progress.plan.relevantFiles.length > 0 && (
            <p className="mb-2 text-text-secondary">
              <span className="font-medium text-text-primary">Umfang:</span>{' '}
              {progress.plan.relevantFiles.join(', ')}
            </p>
          )}
          <ol className="space-y-2">
            {progress.steps.map((step, index) => (
              <li key={step.label} className="flex items-start gap-2">
                <StepIcon status={step.status} />
                <div>
                  <p className="font-medium">
                    {index + 1}. {step.label}
                  </p>
                  {step.detail && <p className="mt-0.5 text-text-secondary">{step.detail}</p>}
                </div>
              </li>
            ))}
          </ol>
          {progress.plan.validationCommands.length > 0 && (
            <section className="mt-3 border-t border-border-primary pt-2">
              <p className="text-xs font-semibold uppercase tracking-wide text-text-secondary">
                Validierung
              </p>
              {progress.plan.validationCommands.map((command) => (
                <p key={command} className="mt-1 font-mono text-sm text-text-primary">
                  {command}
                </p>
              ))}
            </section>
          )}
          {(progress.plan.risks.length > 0 || progress.plan.rollbackStrategy) && (
            <section className="mt-3 border-t border-border-primary pt-2">
              <p className="text-xs font-semibold uppercase tracking-wide text-text-secondary">
                Absicherung
              </p>
              {progress.plan.risks.map((risk) => (
                <p key={risk} className="mt-1 text-text-secondary">{risk}</p>
              ))}
              {progress.plan.rollbackStrategy && (
                <p className="mt-1 text-text-secondary">{progress.plan.rollbackStrategy}</p>
              )}
            </section>
          )}
          {progressMessage && (
            <p className="mt-3 border-t border-border-primary pt-2 text-text-secondary">
              {progressMessage}
            </p>
          )}
        </TooltipContent>
      </Tooltip>
    </div>
  );
}

function StepIcon({ status }: { status: 'active' | 'complete' | 'pending' }) {
  if (status === 'complete') {
    return <Check className="mt-0.5 size-4 shrink-0 text-text-success" aria-hidden="true" />;
  }
  if (status === 'active') {
    return (
      <LoaderCircle
        className="mt-0.5 size-4 shrink-0 animate-spin text-blue-300"
        aria-hidden="true"
      />
    );
  }
  return <Circle className="mt-0.5 size-4 shrink-0 text-text-secondary" aria-hidden="true" />;
}
