---
sidebar_position: 105
title: Using ponduin in ACP Clients
sidebar_label: ponduin in ACP Clients
---

Client applications that support the [Agent Client Protocol (ACP)](https://agentclientprotocol.com/) can connect natively to ponduin. This integration allows you to seamlessly interact with ponduin directly from the client.

:::warning Experimental Feature
ACP is an emerging specification that enables clients to communicate with AI agents like ponduin. This feature has limited adoption and may evolve as the protocol develops.
:::

## How It Works
After you configure ponduin as an agent in the ACP client, you gain access to ponduin's core agent functionality, including its extensions and tools. ponduin also automatically loads any [configured MCP servers](#using-mcp-servers-from-acp-clients) from your ACP client alongside its own extensions, making their tools available without additional configuration.

The client manages the ponduin lifecycle automatically, including:

- **Initialization**: The client runs the `ponduin acp` command to initialize the connection
- **Communication**: The client communicates with ponduin over stdio using JSON-RPC
- **Multiple Sessions**: The client manages multiple concurrent conversations, each with isolated state
- **Model and Mode Switching**: The client can switch models and modes mid-session without restarting
- **File Operations**: The client handles file reads and writes, so ponduin sees changes not yet saved to disk and edits show as native diffs
- **Terminal**: The client runs commands in its own terminal, so output appears alongside the conversation

:::info Session Persistence
ACP sessions are saved to ponduin's session history where you can access and manage them using ponduin. Access to session history in ACP clients might vary.
:::

:::tip Reference Implementation
The [ponduin for VS Code](/docs/experimental/vs-code-extension) extension uses ACP to communicate with ponduin. See the [vscode-ponduin](https://github.com/PondSec/vscode-ponduin) repository for implementation details.
:::

## Setup in ACP Clients
Any editor or IDE that supports ACP can connect to ponduin as an agent server. Check the [official ACP clients list](https://agentclientprotocol.com/overview/clients) for available clients with links to their documentation.

### Example: Zed Editor Setup

ACP was originally developed by [Zed](https://zed.dev/). Zed offers two ways to add ponduin, and you can use either one.

#### Option 1: Install from the ACP Registry (recommended)

ponduin is published in the [ACP Registry](https://agentclientprotocol.com/registry), and Zed 1.5.0 and later has built-in registry support, so it can download and run ponduin for you, with no manual configuration and no pre-installed CLI required.

1. Open Zed
2. Open Agent Settings
3. Click `Add Agent`, then choose `Install from Registry`
4. Select `ponduin`

A registry-installed ponduin runs the same `ponduin acp` server and reads your existing ponduin configuration, so your providers, models, and extensions carry over. Zed keeps the installed version up to date for you.

#### Option 2: Configure ponduin as a Custom Agent

Use a custom agent if you want to run your own ponduin binary (for example, a local development build) or pass environment overrides.

##### Prerequisites

Ensure you have both Zed and ponduin CLI installed:

- **Zed**: Download from [zed.dev](https://zed.dev/)
- **ponduin CLI**: Follow the [installation guide](/docs/getting-started/installation)

  - Verify ponduin is installed: `ponduin --version`

  - Temporarily run `ponduin acp` to test that ACP support is working:

    ```bash
    ponduin acp
    ```

    Press `Ctrl+C` to exit the test.

##### Add ponduin to Your Zed Settings

1. Open Zed
2. Open Agent Settings, click `Add Agent`, then choose `Add Custom Agent`. Zed scaffolds an `agent_servers` entry and opens your settings file
3. Edit the entry so it runs ponduin:

```json
{
  "agent_servers": {
    "ponduin": {
      "type": "custom",
      "command": "ponduin",
      "args": ["acp"]
    }
  },
}
```

You should now be able to interact with ponduin directly in Zed. Your ACP sessions use the same extensions that are enabled in your ponduin configuration, and your tools (Developer, Computer Controller, etc.) work the same way as in regular ponduin sessions.

#### Start Using ponduin in Zed

After adding ponduin with either option above:

1. **Open the Agent Panel**: Click the sparkles agent icon in Zed's status bar
2. **Create New Thread**: Click the `+` button to show thread options
3. **Select ponduin**: Choose `New ponduin` to start a new conversation with ponduin
4. **Start Chatting**: Interact with ponduin directly from the agent panel

#### Advanced Configuration

##### Overriding Provider and Model

By default, ponduin will use the provider and model defined in your [configuration file](/docs/guides/config-files). You can override this for specific ACP configurations using the `PONDUIN_PROVIDER` and `PONDUIN_MODEL` environment variables.

The following Zed settings example configures two ponduin agent instances. This is useful for:
- Comparing model performance on the same task
- Using cost-effective models for simple tasks and powerful models for complex ones

```json
{
  "agent_servers": {
    "ponduin": {
      "type": "custom",
      "command": "ponduin",
      "args": ["acp"]
    },
    "ponduin (GPT-4o)": {
      "type": "custom",
      "command": "ponduin",
      "args": ["acp"],
      "env": {
        "PONDUIN_PROVIDER": "openai",
        "PONDUIN_MODEL": "gpt-4o"
      }
    }
  },
}
```

## Using MCP Servers from ACP Clients

MCP servers configured in the ACP client's `context_servers` are automatically available to ponduin. This allows you to use those MCP servers when using both native client features and the ponduin agent integration.

**Example (Zed):**

```json
{
  "context_servers": {
    "filesystem": {
      "command": "npx",
      "args": [
        "-y",
        "@modelcontextprotocol/server-filesystem",
        "/path/to/allowed/dir"
      ]
    }
  },
  "agent_servers": {
    "ponduin": {
      "type": "custom",
      "command": "ponduin",
      "args": ["acp"]
    }
  },
}
```

To find out what tools are available, just ask ponduin while it's running in the client.

:::info
All MCP servers in `context_servers` are automatically available to ponduin, provided that they use stdio (command-based) or HTTP transports. ponduin doesn't support servers that use the deprecated SSE transport.

If a server in `context_servers` has the same name as a ponduin extension, ponduin uses its own [configuration](/docs/guides/config-files).
:::

## TUI Client

For terminal-based workflows, ponduin provides a TUI (Terminal User Interface) client that communicates with ponduin via ACP. This is useful for developers who prefer working entirely in the terminal or need a lightweight alternative to the desktop app.

### Features

- **Full terminal-based chat interface** - Interactive conversation UI rendered directly in your terminal
- **Real-time streaming responses** - See ponduin's responses as they're generated
- **Tool call visualization** - View tool executions with status indicators, inputs, and outputs
- **Permission dialogs** - Approve or reject tool permissions inline
- **Keyboard navigation** - Navigate conversation history and scroll through responses
- **Markdown rendering** - Formatted output for code blocks, lists, and other markdown elements
- **Message queuing** - Queue messages while ponduin is processing

### Installation

```bash
cd ui/text
npm install
```

### Running the TUI

**Option 1: Auto-launch server (recommended)**

The TUI will automatically start the ponduin acp server if you have it installed:

```bash
npm start
```

**Option 2: Connect to a custom server**

For servers that support the draft standard ACP over Streamable HTTP https://github.com/agentclientprotocol/agent-client-protocol/pull/721

```bash
npm start -- --server http://HOST:PORT

# example server
PONDUIN_SERVER__SECRET_KEY='a-long-random-secret' cargo run -p ponduin-cli --bin ponduin -- serve
```

### Server Authentication

Set the `PONDUIN_SERVER__SECRET_KEY` environment variable to authenticate the ACP endpoint. `ponduin serve` refuses to start without this secret unless you explicitly pass `--dangerously-unauthenticated`:

```bash
PONDUIN_SERVER__SECRET_KEY='a-long-random-secret' ponduin serve
```

Clients authenticate by sending the token in the `X-Secret-Key` header, or as a `?token=` query parameter for WebSocket connections (the browser WebSocket API can't set custom headers). Requests without a matching token receive `401 Unauthorized`, including WebSocket handshakes.

ACP WebSocket Origin validation allows loopback web origins by default. For `ponduin serve`, ACP CORS follows the same policy. If you pass any `--allowed-origin` values, that explicit list replaces the default loopback origins, so include every origin the client needs:

```bash
PONDUIN_SERVER__SECRET_KEY='a-long-random-secret' ponduin serve \
  --allowed-origin 'http://localhost:5173' \
  --allowed-origin 'app://localhost' \
  --allowed-origin 'https://app.example'
```

For local development only, `ponduin serve --dangerously-unauthenticated` starts without a secret and logs a warning. Do not use this mode with shell-capable builtins enabled unless the server is isolated from untrusted browser traffic.

### Single Prompt Mode

Send a single prompt and exit (useful for scripting):

```bash
npm start -- --text "What files are in this directory?"
```

### Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Enter` | Send message |
| `↑` / `↓` | Scroll current response |
| `Shift+↑` / `Shift+↓` | Navigate conversation history |
| `Tab` | Expand/collapse tool call details |
| `Ctrl+C` or `Esc` | Exit (or cancel permission dialog) |

### Permission Dialog

When ponduin requests permission to use a tool, a dialog appears with these options:

| Key | Action |
|-----|--------|
| `y` | Allow once |
| `a` | Always allow |
| `n` | Reject once |
| `N` | Always reject |
| `↑` / `↓` | Navigate options |
| `Enter` | Confirm selection |
| `Esc` | Cancel |

## Additional Resources

import ContentCardCarousel from '@site/src/components/ContentCardCarousel';
import chooseYourIde from '@site/blog/2025-10-24-intro-to-agent-client-protocol-acp/choose-your-ide.png';

<ContentCardCarousel
  items={[
    {
      type: 'video',
      title: 'Intro to Agent Client Protocol (ACP) | Vibe Code with ponduin',
      description: 'Watch how ACP lets you seamlessly integrate ponduin into your code editor to streamline fragmented workflows.',
      thumbnailUrl: 'https://img.youtube.com/vi/Hvu5KDTb6JE/maxresdefault.jpg',
      linkUrl: 'https://www.youtube.com/watch?v=Hvu5KDTb6JE',
      date: '2025-10-16',
      duration: '50:23'
    },
   {
      type: 'blog',
      title: 'Intro to Agent Client Protocol (ACP): The Standard for AI Agent-Editor Integration',
      description: 'Learn how to integrate AI agents like ponduin directly into your code editor via ACP, eliminating window-switching and vendor lock-in.',
      thumbnailUrl: chooseYourIde,
      linkUrl: '/blog/2025/10/24/intro-to-agent-client-protocol-acp',
      date: '2025-10-24',
      duration: '7 min read'
    }
  ]}
/>
