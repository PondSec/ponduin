import { Check, Circle, LoaderCircle } from 'lucide-react';
import { useMemo } from 'react';
import type { Message } from '../types/message';
import { getWorkflowProgress } from '../utils/workflowProgress';
import { Tooltip, TooltipContent, TooltipTrigger } from './ui/Tooltip';

type WorkflowProgressPillProps = {
  messages: Message[];
  progressMessage?: string;
};

export default function WorkflowProgressPill({
  messages,
  progressMessage,
}: WorkflowProgressPillProps) {
  const progress = useMemo(() => getWorkflowProgress(messages), [messages]);

  if (!progress) {
    return null;
  }

  return (
    <div className="relative z-20 mb-3 flex justify-center px-4" aria-live="polite">
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            aria-label={`Workflow, Schritt ${progress.currentStep} von ${progress.phaseCount}`}
            className="flex items-center gap-2 rounded-full border border-border-primary bg-background-secondary px-4 py-2 text-sm text-text-secondary shadow-sm transition-colors hover:bg-background-tertiary"
          >
            <LoaderCircle
              className="size-4 shrink-0 animate-spin text-blue-400"
              aria-hidden="true"
            />
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
        <TooltipContent side="top" className="w-[min(28rem,calc(100vw-2rem))] p-3 text-left">
          <p className="mb-2 text-xs font-semibold uppercase tracking-wide text-text-inverse/60">
            Konkreter Plan
          </p>
          <ol className="space-y-2">
            {progress.steps.map((step, index) => (
              <li key={step.label} className="flex items-start gap-2">
                <StepIcon status={step.status} />
                <div>
                  <p className="font-medium">
                    {index + 1}. {step.label}
                  </p>
                  {step.detail && <p className="mt-0.5 text-text-inverse/70">{step.detail}</p>}
                </div>
              </li>
            ))}
          </ol>
          {progressMessage && (
            <p className="mt-3 border-t border-white/15 pt-2">{progressMessage}</p>
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
  return <Circle className="mt-0.5 size-4 shrink-0 text-text-inverse/50" aria-hidden="true" />;
}
