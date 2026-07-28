import PonduinLogo from './PonduinLogo';
import AnimatedIcons from './AnimatedIcons';
import PonduinPulse from './PonduinPulse';
import { ChatState } from '../types/chatState';
import { defineMessages, useIntl } from '../i18n';

interface LoadingPonduinProps {
  message?: string;
  chatState?: ChatState;
}

const i18n = defineMessages({
  loadingConversation: {
    id: 'loadingPonduin.loadingConversation',
    defaultMessage: 'loading conversation...',
  },
  thinking: {
    id: 'loadingPonduin.thinking',
    defaultMessage: 'ponduin is thinking…',
  },
  streaming: {
    id: 'loadingPonduin.streaming',
    defaultMessage: 'ponduin is working on it…',
  },
  waiting: {
    id: 'loadingPonduin.waiting',
    defaultMessage: 'ponduin is waiting…',
  },
  compacting: {
    id: 'loadingPonduin.compacting',
    defaultMessage: 'ponduin is compacting the conversation...',
  },
  idle: {
    id: 'loadingPonduin.idle',
    defaultMessage: 'ponduin is working on it…',
  },
  restartingAgent: {
    id: 'loadingPonduin.restartingAgent',
    defaultMessage: 'restarting session...',
  },
});

const STATE_ICONS: Record<ChatState, React.ReactNode> = {
  [ChatState.LoadingConversation]: <AnimatedIcons className="flex-shrink-0" cycleInterval={600} />,
  [ChatState.Thinking]: <AnimatedIcons className="flex-shrink-0" cycleInterval={600} />,
  [ChatState.Streaming]: <PonduinPulse className="flex-shrink-0" cycleInterval={150} />,
  [ChatState.WaitingForUserInput]: (
    <AnimatedIcons className="flex-shrink-0" cycleInterval={600} variant="waiting" />
  ),
  [ChatState.Compacting]: <AnimatedIcons className="flex-shrink-0" cycleInterval={600} />,
  [ChatState.Idle]: <PonduinLogo size="small" hover={false} />,
  [ChatState.RestartingAgent]: <AnimatedIcons className="flex-shrink-0" cycleInterval={600} />,
};

const STATE_MESSAGE_KEYS: Record<ChatState, keyof typeof i18n> = {
  [ChatState.LoadingConversation]: 'loadingConversation',
  [ChatState.Thinking]: 'thinking',
  [ChatState.Streaming]: 'streaming',
  [ChatState.WaitingForUserInput]: 'waiting',
  [ChatState.Compacting]: 'compacting',
  [ChatState.Idle]: 'idle',
  [ChatState.RestartingAgent]: 'restartingAgent',
};

const LoadingPonduin = ({ message, chatState = ChatState.Idle }: LoadingPonduinProps) => {
  const intl = useIntl();
  const displayMessage = message || intl.formatMessage(i18n[STATE_MESSAGE_KEYS[chatState]]);
  const icon = STATE_ICONS[chatState];

  return (
    <div className="w-full animate-fade-slide-up">
      <div
        data-testid="loading-indicator"
        className="flex items-center gap-2 text-xs text-text-primary py-2"
      >
        {icon}
        {displayMessage}
      </div>
    </div>
  );
};

export default LoadingPonduin;
