import type { ToolCall, ToolCallUpdate } from '@agentclientprotocol/sdk';
import type { TokenState } from '../../types/chat';
import type { Message, NotificationEvent } from '../../types/message';

export type AcpChatStateChange =
  | { type: 'messages'; messages: Message[] }
  | { type: 'tokenState'; tokenState: Partial<TokenState> }
  | { type: 'progressMessage'; message: string | undefined }
  | {
      type: 'sessionInfo';
      name?: string;
      activeRunId?: string | null;
      ponduinMode?: string;
    }
  | { type: 'localSteerConfirmed'; messageId: string }
  | { type: 'notification'; notification: NotificationEvent };

export interface AdapterState {
  messages: Message[];
  localSteerTextByMessageId: Map<string, string>;
  toolCallStatesById: Map<string, ToolCallState>;
}

export type ToolCallState = Omit<ToolCallUpdate, '_meta'>;

export interface PonduinMessageMeta {
  messageId?: string;
  created?: number;
  steer?: boolean;
}

export interface ToolIdentity {
  toolName?: string;
  extensionName?: string;
}

export const DEFAULT_VISIBLE_MESSAGE_METADATA: Message['metadata'] = {
  userVisible: true,
  agentVisible: true,
};

export function messagesChange(state: AdapterState): AcpChatStateChange[] {
  return [{ type: 'messages', messages: state.messages.map(cloneMessage) }];
}

export function cloneMessage(message: Message): Message {
  return {
    ...message,
    content: message.content.map((content) => ({ ...content })),
    metadata: { ...message.metadata },
  };
}

export function getPonduinMessageMeta(update: { _meta?: unknown }): PonduinMessageMeta {
  if (!isRecord(update._meta)) {
    return {};
  }

  const ponduin = update._meta.ponduin;
  if (!isRecord(ponduin)) {
    return {};
  }

  return {
    created: typeof ponduin.created === 'number' ? ponduin.created : undefined,
    messageId: typeof ponduin.messageId === 'string' ? ponduin.messageId : undefined,
    steer: ponduin.steer === true ? true : undefined,
  };
}

export function getPonduinActiveRunId(update: { _meta?: unknown }): string | null | undefined {
  if (!isRecord(update._meta)) {
    return undefined;
  }

  const ponduin = update._meta.ponduin;
  if (!isRecord(ponduin) || !('activeRunId' in ponduin)) {
    return undefined;
  }

  return typeof ponduin.activeRunId === 'string' || ponduin.activeRunId === null
    ? ponduin.activeRunId
    : undefined;
}

export function getPonduinQueuedSteer(update: { _meta?: unknown }): string | undefined {
  if (!isRecord(update._meta)) return undefined;
  const ponduin = update._meta.ponduin;
  if (!isRecord(ponduin) || !isRecord(ponduin.queuedSteer)) return undefined;
  return typeof ponduin.queuedSteer.messageId === 'string'
    ? ponduin.queuedSteer.messageId
    : undefined;
}

export function rawInputToArguments(rawInput: unknown): Record<string, unknown> {
  return isRecord(rawInput) ? rawInput : {};
}

export function toolIdentity(update: ToolCall | ToolCallUpdate): ToolIdentity {
  if (!isRecord(update._meta)) {
    return {};
  }

  const ponduin = update._meta.ponduin;
  if (!isRecord(ponduin) || !isRecord(ponduin.toolCall)) {
    return {};
  }

  return {
    toolName: typeof ponduin.toolCall.toolName === 'string' ? ponduin.toolCall.toolName : undefined,
    extensionName:
      typeof ponduin.toolCall.extensionName === 'string'
        ? ponduin.toolCall.extensionName
        : undefined,
  };
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}
