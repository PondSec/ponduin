# ponduin ACP TUI

Early stage and part of ponduin's broader move to ACP

https://github.com/PondSec/ponduin/issues/6642
https://github.com/PondSec/ponduin/discussions/7309

## Running

The TUI launches the ponduin ACP server by spawning `ponduin acp`. Which binary it spawns is resolved by `@pondsec/ponduin-sdk`:

1. the `PONDUIN_BINARY` environment variable, if set, otherwise
2. the platform's prebuilt `@pondsec/ponduin-binary-*` package (an optional dependency of the pinned `@pondsec/ponduin-sdk`).

```bash
cd ui/text
pnpm install   # pulls the pinned @pondsec/ponduin-sdk and its matching @pondsec/ponduin-binary-* package
pnpm start     # tsx src/tui.tsx — runs against the released binary, no Rust build
```

The TUI pins a specific `@pondsec/ponduin-sdk` version, so `pnpm start` always runs against a ponduin binary that matches the SDK.

### Building ponduin from local source

To test local Rust changes, run the dev launcher directly. It builds a debug binary (`cargo build -p ponduin-cli` → `target/debug/ponduin`) from the workspace root and points the TUI at it via `PONDUIN_BINARY`:

```bash
node scripts/dev-start.mjs
```

If your changes touch the ACP schema, also point the TUI at the in-repo SDK so the two stay matched: set `@pondsec/ponduin-sdk` to `workspace:*` in `package.json` and re-run `pnpm install`. Otherwise the locally built binary may not match the pinned published SDK's schema. Revert that change before committing — the TUI is meant to stay frozen on its pinned SDK version.

To run any other prebuilt binary, set `PONDUIN_BINARY=/path/to/ponduin` and use `pnpm start`.

### Custom server URL

To connect to an already-running server instead of spawning a binary:

```bash
pnpm start -- --server http://localhost:8080
```
