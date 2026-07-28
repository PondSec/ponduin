<div align="center">
  <img src="documentation/static/img/ponduin-logo.png" alt="Ponduin" width="220" />

# Ponduin

**Local AI Agent · Privacy First**

Ponduin is a local AI agent developed by [PondSec](https://pondsec.com). It combines
private on-device operation with model-independent automation for coding, terminal
workflows, file management and agent-driven tasks.

[Website](https://ponduin.de) · [Documentation](https://ponduin.de/docs) · [PondSec](https://pondsec.com)
</div>

## Local by design

Ponduin runs on your machine and keeps the execution environment under your
control. You decide which model provider, local runtime, extensions and tools are
available to the agent.

- **Local operation** — run the agent and its tools directly on your computer.
- **Model independent** — connect supported cloud providers or local models.
- **Automation** — coordinate repeatable, multi-step technical workflows.
- **Coding** — inspect repositories, edit files, execute commands and validate changes.
- **Terminal access** — use the native CLI for focused shell workflows.
- **File management** — work with project files and local directories.
- **Agent capabilities** — extend workflows through MCP-compatible integrations.
- **Privacy focused** — retain control over local data, tools and provider selection.

Ponduin is available as a desktop application and as a command-line tool for
macOS, Linux and Windows.

## Command line

Build the CLI:

```bash
cargo build --release --bin ponduin
```

Start Ponduin:

```bash
ponduin
```

View all commands:

```bash
ponduin --help
```

## Desktop application

Install the UI dependencies and create a local package:

```bash
cd ui
pnpm install
cd desktop
pnpm run make
```

Development mode:

```bash
cd ui/desktop
pnpm start
```

## Configuration

Ponduin uses the `PONDUIN_` namespace for product-specific environment variables.
The default configuration file is `config.yaml` inside the platform-specific
Ponduin configuration directory.

Common values include:

```text
PONDUIN_PROVIDER
PONDUIN_MODEL
PONDUIN_PATH_ROOT
PONDUIN_CONFIG_DIR
```

Project-specific agent context belongs in `.ponduinhints` or `AGENTS.md`.

## Documentation

- [Quickstart](https://ponduin.de/docs/quickstart)
- [Installation](https://ponduin.de/docs/getting-started/installation)
- [Providers](https://ponduin.de/docs/getting-started/providers)
- [Extensions](https://ponduin.de/docs/getting-started/using-extensions)
- [Troubleshooting](https://ponduin.de/docs/troubleshooting/diagnostics-and-reporting)

## Legal

Ponduin is developed by PondSec. Copyright © 2026 Joshua Dean Pond, PondSec and
Ponduin. Licensing, attribution and third-party notices
for the source tree are documented in [LICENSE](LICENSE) and the included
component license files.
