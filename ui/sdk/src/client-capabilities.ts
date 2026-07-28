import type { PonduinMcpHostCapabilities } from "./mcp-apps.js";

export interface PonduinClientCapabilitiesMeta {
  ponduin?: {
    mcpHostCapabilities?: PonduinMcpHostCapabilities;
    customNotifications?: boolean;
  };
}
