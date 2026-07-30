<div align="center">
  <img src="documentation/static/img/ponduin-logo.png" alt="Ponduin logo" width="220" />

# Ponduin

### A privacy-first AI agent that works where your files, tools and projects already live.

Ponduin is an extensible local AI agent developed by [PondSec](https://pondsec.com).
It combines a desktop application, a native command-line interface and an agent runtime
for coding, terminal automation, file operations and multi-step technical workflows.

[Website](https://ponduin.de) · [Documentation](https://ponduin.de/docs) · [Report an issue](https://github.com/PondSec/ponduin/issues) · [PondSec](https://pondsec.com)

</div>

> [!IMPORTANT]
> Ponduin is under active development. Interfaces, configuration options and packaging
> steps may change while the project is being stabilized.

## Why Ponduin?

Most AI assistants live somewhere else. Your code, shell, files and infrastructure do not.
Ponduin brings the agent into the local execution environment while keeping model choice,
tool access and data flow under your control.

- **Local-first execution** — run the agent and its tools directly on your computer.
- **Provider independent** — use supported cloud providers or connect compatible local models.
- **Coding workflows** — inspect repositories, edit files, execute commands and validate changes.
- **Terminal automation** — coordinate repeatable shell and system workflows from the CLI or desktop app.
- **File operations** — work with project files and directories inside explicitly selected workspaces.
- **Extensible tooling** — add capabilities through MCP-compatible integrations and agent tools.
- **Project-aware context** — provide repository instructions through `.ponduinhints` or `AGENTS.md`.
- **Privacy by control** — decide which models, providers, extensions, tools and directories are available.

## Interfaces

Ponduin provides two primary ways to work:

| Interface | Best for |
| --- | --- |
| **Desktop application** | Interactive conversations, project work, configuration and visual workflows |
| **Command-line interface** | Terminal-focused tasks, scripting, debugging and development |

## Architecture at a glance

```text
┌──────────────────────────────────────────────────────────┐
│                    Ponduin Desktop                       │
│              Electron · React · TypeScript              │
└───────────────────────────┬──────────────────────────────┘
                            │
┌───────────────────────────▼──────────────────────────────┐
│                 Native Ponduin Runtime                   │
│                         Rust                             │
├──────────────────────────────────────────────────────────┤
│ Providers · Agent orchestration · Tools · Extensions     │
│ Workspace access · Terminal execution · File operations  │
└───────────────────────────┬──────────────────────────────┘
                            │
             ┌──────────────┴──────────────┐
             ▼                             ▼
      Local model runtime            Cloud provider
       such as Ollama                  when selected
```

The desktop application bundles the native `ponduin` executable. A desktop package without
that executable may open but cannot start the local backend.

## Requirements

### Core development

- Git
- Rust and Cargo via `rustup`
- A C/C++ build toolchain
- CMake

### Desktop development

- Node.js `24.x`
- pnpm `10.30` or newer

On macOS with Homebrew:

```bash
brew install cmake node pnpm
```

Install Rust separately when it is not already available:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

Verify the toolchain:

```bash
rustc --version
cargo --version
cmake --version
node --version
pnpm --version
```

## Getting started

Clone the repository and enter the project:

```bash
git clone https://github.com/PondSec/ponduin.git
cd ponduin
```

### Build the native CLI

```bash
cargo build --release --bin ponduin
```

The resulting executable is located at:

```text
target/release/ponduin
```

Run it directly:

```bash
./target/release/ponduin
```

View available commands:

```bash
./target/release/ponduin --help
```

## Desktop development

Install the JavaScript dependencies from the UI workspace:

```bash
cd ui
pnpm install
```

Start the desktop application in development mode:

```bash
cd desktop
pnpm start
```

For direct Electron Forge development:

```bash
pnpm run start-gui
```

## Build the macOS desktop application

The macOS package requires both the frontend and the native Rust backend. Building only the
Electron project produces an incomplete app bundle, which is a particularly charming way to
spend an evening debugging a file that was never copied.

### 1. Build the native backend

From the repository root:

```bash
cargo build --release --bin ponduin
```

If the build fails inside `llama-cpp-sys` with a message asking whether CMake is installed:

```bash
brew install cmake
cargo build --release --bin ponduin
```

### 2. Copy the backend into the desktop bundle sources

From the repository root:

```bash
mkdir -p ui/desktop/src/bin
cp target/release/ponduin ui/desktop/src/bin/ponduin
chmod +x ui/desktop/src/bin/ponduin
```

### 3. Create the Apple Silicon application bundle

```bash
cd ui/desktop
rm -rf out
pnpm run bundle:default
```

The generated files are located at:

```text
ui/desktop/out/Ponduin-darwin-arm64/Ponduin.app
ui/desktop/out/Ponduin-darwin-arm64/Ponduin.zip
```

### 4. Verify the bundled backend

Do not skip this check. The app can exist while its backend very much does not.

```bash
ls -lh out/Ponduin-darwin-arm64/Ponduin.app/Contents/Resources/bin/ponduin
```

A valid result should show an executable file rather than `No such file or directory`.

### 5. Install the application locally

```bash
pkill -x Ponduin 2>/dev/null || true
rm -rf /Applications/Ponduin.app

ditto \
  out/Ponduin-darwin-arm64/Ponduin.app \
  /Applications/Ponduin.app

chmod +x /Applications/Ponduin.app/Contents/Resources/bin/ponduin
xattr -dr com.apple.quarantine /Applications/Ponduin.app
open /Applications/Ponduin.app
```

### Intel macOS build

```bash
cd ui/desktop
rm -rf out
pnpm run bundle:intel
```

The Intel archive is created inside:

```text
out/Ponduin-darwin-x64/
```

> [!NOTE]
> Cross-architecture builds may require the matching Rust target and native dependencies.
> Building on the target architecture is the most reliable route during development.

## Packaging commands

Run these commands from `ui/desktop`:

| Command | Purpose |
| --- | --- |
| `pnpm start` | Start the repository development workflow |
| `pnpm run start-gui` | Start Electron Forge directly |
| `pnpm run package` | Create an unpacked Electron application |
| `pnpm run make` | Run configured Electron Forge makers |
| `pnpm run bundle:default` | Build and ZIP the Apple Silicon macOS application |
| `pnpm run bundle:intel` | Build and ZIP the Intel macOS application |
| `pnpm run typecheck` | Run TypeScript checks |
| `pnpm run lint:check` | Run type, lint and localization checks |
| `pnpm run test:run` | Run unit tests once |
| `pnpm run test:integration` | Run integration tests |
| `pnpm run test-e2e` | Run Playwright end-to-end tests |

## Publishing a manual GitHub release

A macOS `.app` is a directory bundle, not a single uploadable file. Distribute the generated
ZIP archive or a DMG rather than attempting to upload the `.app` directory directly.

Create a tag and release with the Apple Silicon ZIP:

```bash
VERSION=v1.44.1

git tag -a "$VERSION" -m "Ponduin $VERSION"
git push origin "$VERSION"

gh release create "$VERSION" \
  ui/desktop/out/Ponduin-darwin-arm64/Ponduin.zip \
  --repo PondSec/ponduin \
  --title "Ponduin $VERSION" \
  --generate-notes \
  --verify-tag
```

Upload or replace an asset on an existing release:

```bash
gh release upload "$VERSION" \
  ui/desktop/out/Ponduin-darwin-arm64/Ponduin.zip \
  --repo PondSec/ponduin \
  --clobber
```

### Auto-update status

The current local Forge ZIP workflow does not generate the `latest-mac.yml` integrity manifest
required by `electron-updater`. Manual GitHub downloads work, but native macOS auto-updates require
a release pipeline that publishes the application archive together with the matching update manifest.

## Configuration

Ponduin uses the `PONDUIN_` namespace for product-specific environment variables. The default
configuration file is `config.yaml` inside the platform-specific Ponduin configuration directory.

Common variables include:

```text
PONDUIN_PROVIDER
PONDUIN_MODEL
PONDUIN_PATH_ROOT
PONDUIN_CONFIG_DIR
```

Example shell configuration:

```bash
export PONDUIN_PROVIDER="ollama"
export PONDUIN_MODEL="qwen3:8b"
export PONDUIN_PATH_ROOT="$HOME/Developer"
```

Project-specific instructions belong in one of these files at the project root:

```text
.ponduinhints
AGENTS.md
```

Treat configuration files as sensitive when they contain API keys, provider credentials, internal
hosts or private paths. Do not commit secrets to the repository.

## Local models

Ponduin can be used with compatible local model runtimes. A typical Ollama workflow is:

```bash
ollama pull qwen3:8b
ollama serve
```

Then select the Ollama provider and the exact model tag in Ponduin. Model names are literal:
`qwen3` and `qwen3:8b` may resolve differently, because apparently even model tags needed their
own small bureaucracy.

Local models differ significantly in tool use, planning, context capacity and coding quality.
The agent runtime can improve reliability, but it cannot turn a small model into a frontier model.
Use a model appropriate for the complexity and risk of the task.

## Testing and quality checks

### Rust

From the repository root:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

### Desktop

From `ui/desktop`:

```bash
pnpm run typecheck
pnpm run lint:check
pnpm run format:check
pnpm run test:run
pnpm run test:integration
pnpm run test-e2e
```

Run the checks relevant to your change before opening a pull request. Packaging changes should also
be validated by launching the generated application, not merely by admiring a successful build log.

## Troubleshooting

### `is cmake not installed?`

Install CMake and rebuild the Rust binary:

```bash
brew install cmake
cargo build --release --bin ponduin
```

### `Ponduin binary not found`

Confirm that the native binary was copied before packaging:

```bash
ls -lh ui/desktop/src/bin/ponduin
```

Then rebuild the bundle and verify the final location:

```bash
cd ui/desktop
rm -rf out
pnpm run bundle:default
ls -lh out/Ponduin-darwin-arm64/Ponduin.app/Contents/Resources/bin/ponduin
```

### `stable release does not contain the required latest-mac.yml integrity manifest`

The selected GitHub release does not contain the update metadata expected by `electron-updater`.
The installed application can still be used, but updates must be installed manually until the
release workflow publishes `latest-mac.yml` alongside the macOS archive.

### macOS blocks the locally built application

For a trusted local development build:

```bash
xattr -dr com.apple.quarantine /Applications/Ponduin.app
```

Public distribution should use proper Apple code signing and notarization rather than asking every
user to bypass Gatekeeper.

### Clean rebuild

```bash
cd ui/desktop
rm -rf out .vite
pnpm install
pnpm run bundle:default
```

## Repository structure

```text
ponduin/
├── crates/                 Rust crates and core runtime components
├── documentation/          Documentation site and static assets
├── examples/               Example configurations and integrations
├── ui/
│   ├── desktop/            Electron desktop application
│   └── packages/           Shared UI packages and SDK components
├── Cargo.toml              Rust workspace configuration
├── README.md               Project overview and build guide
└── LICENSE                 Licensing terms
```

## Security model

Ponduin can execute tools and commands with the permissions of the user who starts it. Treat agent
access like shell access:

- Review the selected workspace before enabling tools.
- Use least-privilege credentials and separate development environments.
- Keep secrets outside repositories and prompts whenever possible.
- Inspect destructive or infrastructure-changing actions before approval.
- Use sandboxes, containers or disposable test systems for untrusted code.
- Never assume model output is safe merely because it sounds confident.

Security issues should be reported privately through PondSec's established security contact rather
than disclosed in a public issue before remediation is available.

## Documentation

- [Quickstart](https://ponduin.de/docs/quickstart)
- [Installation](https://ponduin.de/docs/getting-started/installation)
- [Providers](https://ponduin.de/docs/getting-started/providers)
- [Extensions](https://ponduin.de/docs/getting-started/using-extensions)
- [Diagnostics and reporting](https://ponduin.de/docs/troubleshooting/diagnostics-and-reporting)

## Contributing

1. Create a focused branch from the current development branch.
2. Keep changes small enough to review and test properly.
3. Add or update tests for behavioral changes.
4. Run formatting, linting and relevant test suites.
5. Document user-visible configuration or workflow changes.
6. Open a pull request with the motivation, implementation notes and verification steps.

Avoid committing generated packages, local binaries, credentials, logs or machine-specific files.

## Legal

Ponduin is developed by PondSec.

Copyright © 2026 Joshua Dean Pond, PondSec and Ponduin.

Licensing, attribution and third-party notices are documented in [LICENSE](LICENSE) and the
component-specific license files included in the source tree.
