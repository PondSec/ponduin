import { Switch } from '../../ui/switch';
import { useConfig } from '../../ConfigContext';
import { defineMessages, useIntl } from '../../../i18n';

const CODING_ENABLED_KEY = 'PONDUIN_CODING_ENABLED';
const CODING_MODE_KEY = 'PONDUIN_CODING_MODE';

const i18n = defineMessages({
  enable: {
    id: 'codingAgentSettings.enable',
    defaultMessage: 'Enable internal coding agent',
  },
  description: {
    id: 'codingAgentSettings.description',
    defaultMessage:
      'Adds repository analysis, safe editing, validation, review, and Git tools directly to Ponduin. No extension is used.',
  },
  taskMode: {
    id: 'codingAgentSettings.taskMode',
    defaultMessage: 'Coding task mode',
  },
  autonomous: {
    id: 'codingAgentSettings.autonomous',
    defaultMessage:
      'Autonomous mode is active: coding tools can run without confirmation. Hard security blocks still apply.',
  },
  gated: {
    id: 'codingAgentSettings.gated',
    defaultMessage:
      'Only Autonomous mode runs coding tools without confirmation. Manual and Smart retain approval gates; Chat disables all tools.',
  },
  restart: {
    id: 'codingAgentSettings.restart',
    defaultMessage: 'Coding-agent changes apply after restarting Ponduin.',
  },
  coding: {
    id: 'codingAgentSettings.mode.coding',
    defaultMessage: 'Coding',
  },
  debugging: {
    id: 'codingAgentSettings.mode.debugging',
    defaultMessage: 'Debugging',
  },
  refactoring: {
    id: 'codingAgentSettings.mode.refactoring',
    defaultMessage: 'Refactoring',
  },
  repositoryAnalysis: {
    id: 'codingAgentSettings.mode.repositoryAnalysis',
    defaultMessage: 'Repository analysis',
  },
  testGeneration: {
    id: 'codingAgentSettings.mode.testGeneration',
    defaultMessage: 'Test generation',
  },
  documentation: {
    id: 'codingAgentSettings.mode.documentation',
    defaultMessage: 'Documentation',
  },
  review: {
    id: 'codingAgentSettings.mode.review',
    defaultMessage: 'Review',
  },
});

const taskModes = [
  ['coding', i18n.coding],
  ['debugging', i18n.debugging],
  ['refactoring', i18n.refactoring],
  ['repository_analysis', i18n.repositoryAnalysis],
  ['test_generation', i18n.testGeneration],
  ['documentation', i18n.documentation],
  ['review', i18n.review],
] as const;

export const CodingAgentSettings = () => {
  const intl = useIntl();
  const { config, upsert } = useConfig();
  const enabled = config[CODING_ENABLED_KEY] === true;
  const configuredTaskMode =
    typeof config[CODING_MODE_KEY] === 'string' ? config[CODING_MODE_KEY] : 'coding';
  const taskMode =
    configuredTaskMode === 'general' || !taskModes.some(([value]) => value === configuredTaskMode)
      ? 'coding'
      : configuredTaskMode;
  const autonomous = (config.PONDUIN_MODE ?? 'auto') === 'auto';

  const handleEnabledChange = async (checked: boolean) => {
    if (checked && configuredTaskMode === 'general') {
      await upsert(CODING_MODE_KEY, 'coding', false);
    }
    await upsert(CODING_ENABLED_KEY, checked, false);
  };

  const handleTaskModeChange = async (value: string) => {
    await upsert(CODING_MODE_KEY, value, false);
  };

  return (
    <div className="space-y-3 px-2 py-2">
      <div className="flex items-center justify-between gap-4">
        <div>
          <h3 className="text-text-primary">{intl.formatMessage(i18n.enable)}</h3>
          <p className="text-xs text-text-secondary max-w-xl mt-[2px]">
            {intl.formatMessage(i18n.description)}
          </p>
        </div>
        <Switch
          checked={enabled}
          onCheckedChange={handleEnabledChange}
          variant="mono"
          aria-label={intl.formatMessage(i18n.enable)}
        />
      </div>

      <div className="flex items-center justify-between gap-4">
        <label htmlFor="coding-task-mode" className="text-sm text-text-primary">
          {intl.formatMessage(i18n.taskMode)}
        </label>
        <select
          id="coding-task-mode"
          value={taskMode}
          disabled={!enabled}
          onChange={(event) => handleTaskModeChange(event.target.value)}
          className="min-w-52 rounded border border-border-primary bg-background-primary px-3 py-2 text-sm text-text-primary disabled:cursor-not-allowed disabled:opacity-50"
        >
          {taskModes.map(([value, descriptor]) => (
            <option key={value} value={value}>
              {intl.formatMessage(descriptor)}
            </option>
          ))}
        </select>
      </div>

      <p className="text-xs text-text-secondary max-w-xl">
        {intl.formatMessage(autonomous ? i18n.autonomous : i18n.gated)}
      </p>
      <p className="text-xs text-text-secondary max-w-xl">{intl.formatMessage(i18n.restart)}</p>
    </div>
  );
};
