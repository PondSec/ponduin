import type { ToolListItem, ToolPermissionEntry, ToolPermissionLevel } from '@pondsec/ponduin-sdk';
import { getAcpClient } from './acpConnection';

export type { ToolListItem, ToolPermissionEntry, ToolPermissionLevel };

export async function listTools(
  sessionId: string,
  extensionName?: string
): Promise<ToolListItem[]> {
  const client = await getAcpClient();
  const response = await client.ponduin.toolsList_unstable({
    sessionId,
    extensionName: extensionName ?? null,
  });
  return response.tools ?? [];
}

export async function setToolPermissions(toolPermissions: ToolPermissionEntry[]): Promise<void> {
  const client = await getAcpClient();
  await client.ponduin.toolsPermissionsSet_unstable({ toolPermissions });
}
