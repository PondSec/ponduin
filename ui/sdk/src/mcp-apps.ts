import { RESOURCE_MIME_TYPE } from "@modelcontextprotocol/ext-apps/app-bridge";
import type {
  McpUiAppResourceConfig,
  McpUiAppToolConfig,
} from "@modelcontextprotocol/ext-apps/server";
import type {
  BlobResourceContents,
  ReadResourceResult,
  TextResourceContents,
  Tool,
} from "@modelcontextprotocol/sdk/types.js";

export const PONDUIN_MCP_UI_EXTENSION_ID = "io.modelcontextprotocol/ui" as const;

export interface PonduinMcpUiExtensionSettings {
  mimeTypes: string[];
}

export interface PonduinMcpHostCapabilities {
  extensions: Record<string, PonduinMcpUiExtensionSettings>;
}

export type PonduinToolUiMetadata = Extract<
  McpUiAppToolConfig["_meta"],
  { ui: unknown }
>["ui"];

export type PonduinToolMetadata = NonNullable<Tool["_meta"]> & {
  ui?: PonduinToolUiMetadata;
  ponduin_extension?: string;
};

export type PonduinSessionTool = Tool & {
  meta?: PonduinToolMetadata;
  _meta?: PonduinToolMetadata;
};

export type PonduinTextResourceContents = TextResourceContents;

export type PonduinBlobResourceContents = BlobResourceContents;

export type PonduinResourceContents = TextResourceContents | BlobResourceContents;

export type PonduinReadResourceResult = ReadResourceResult;

export type PonduinResourceMetadata = NonNullable<
  Extract<NonNullable<McpUiAppResourceConfig["_meta"]>, { ui?: unknown }>["ui"]
>;

export interface PonduinMcpAppToolPayload {
  toolName: string;
  extensionName: string;
  resourceUri: string;
  toolMeta?: PonduinToolMetadata;
  resourceResult?: PonduinReadResourceResult | null;
  readError?: string;
}

export interface PonduinToolCallUpdateMeta {
  ponduin?: {
    mcpApp?: PonduinMcpAppToolPayload;
    [key: string]: unknown;
  };
  [key: string]: unknown;
}

export const DEFAULT_PONDUIN_MCP_HOST_CAPABILITIES: PonduinMcpHostCapabilities = {
  extensions: {
    [PONDUIN_MCP_UI_EXTENSION_ID]: {
      mimeTypes: [RESOURCE_MIME_TYPE],
    },
  },
};
