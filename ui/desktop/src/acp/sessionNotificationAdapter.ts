import type { PonduinSessionNotification_unstable } from '@pondsec/ponduin-sdk';
import type { RequestPermissionRequest, SessionNotification } from '@agentclientprotocol/sdk';
import type { Message } from '../types/message';
import {
  applyElicitationRequest as applyElicitationRequestToState,
  applyElicitationStatus as applyElicitationStatusToState,
  type ElicitationStatus,
} from './adapter/elicitations';
import { applyPonduinSessionNotification } from './adapter/ponduinSessionNotifications';
import { applyContentChunk, applyThoughtChunk } from './adapter/messages';
import { applyPermissionRequest as applyPermissionRequestToState } from './adapter/permissions';
import {
  type AcpChatStateChange,
  type AdapterState,
  cloneMessage,
  getPonduinActiveRunId,
  getPonduinQueuedSteer,
} from './adapter/shared';
import { applyToolCall, applyToolCallUpdate } from './adapter/tools';
import type { AcpElicitationRequest } from './elicitationRequests';

export type { AcpChatStateChange } from './adapter/shared';

export interface AcpSessionNotificationAdapter {
  apply(notification: SessionNotification): AcpChatStateChange[];
  applyPonduin(notification: PonduinSessionNotification_unstable): AcpChatStateChange[];
  applyPermissionRequest(request: RequestPermissionRequest): AcpChatStateChange[];
  applyElicitationRequest(request: AcpElicitationRequest): AcpChatStateChange[];
  applyElicitationStatus(elicitationId: string, status: ElicitationStatus): AcpChatStateChange[];
  getMessages(): Message[];
}

export function createAcpSessionNotificationAdapter(
  initialMessages: Message[] = [],
  localSteerTextByMessageId: Map<string, string> = new Map()
): AcpSessionNotificationAdapter {
  const state: AdapterState = {
    messages: initialMessages.map(cloneMessage),
    localSteerTextByMessageId: new Map(localSteerTextByMessageId),
    toolCallStatesById: new Map(),
  };

  return {
    apply(notification) {
      return applyAcpSessionNotification(state, notification);
    },
    applyPonduin(notification) {
      return applyPonduinSessionNotification(state, notification);
    },
    applyPermissionRequest(request) {
      return applyPermissionRequestToState(state, request);
    },
    applyElicitationRequest(request) {
      return applyElicitationRequestToState(state, request);
    },
    applyElicitationStatus(elicitationId, status) {
      return applyElicitationStatusToState(state, elicitationId, status);
    },
    getMessages() {
      return state.messages.map(cloneMessage);
    },
  };
}

function applyAcpSessionNotification(
  state: AdapterState,
  notification: SessionNotification
): AcpChatStateChange[] {
  const update = notification.update;

  switch (update.sessionUpdate) {
    case 'user_message_chunk':
      return applyContentChunk(state, 'user', update);
    case 'agent_message_chunk':
      return applyContentChunk(state, 'assistant', update);
    case 'agent_thought_chunk':
      return applyThoughtChunk(state, update);
    case 'tool_call':
      return applyToolCall(state, update);
    case 'tool_call_update':
      return applyToolCallUpdate(state, update);
    case 'session_info_update': {
      const activeRunId = getPonduinActiveRunId(update);
      const queuedSteerMessageId = getPonduinQueuedSteer(update);
      const changes: AcpChatStateChange[] = [];

      if (update.title || activeRunId !== undefined) {
        changes.push({
          type: 'sessionInfo',
          ...(update.title ? { name: update.title } : {}),
          ...(activeRunId !== undefined ? { activeRunId } : {}),
        });
      }

      if (queuedSteerMessageId) {
        changes.push({ type: 'localSteerConfirmed', messageId: queuedSteerMessageId });
      }

      return changes;
    }
    case 'usage_update':
      return [];
    default:
      return [];
  }
}
