# Authorized Ponduin Distributions

Ponduin is distributed under the PondSec Ponduin Software License Agreement.
Creating, modifying, publishing, sublicensing, or distributing a customized
Ponduin build requires prior written authorization from PondSec.

## Internal PondSec builds

Authorized PondSec teams can prepare internal distributions with:

- preconfigured local or cloud model providers;
- approved MCP extensions;
- managed configuration and secrets;
- platform-specific packaging; and
- organization-specific defaults.

The implementation points remain:

| Area | Location |
|---|---|
| Core agent | `crates/ponduin/` |
| CLI | `crates/ponduin-cli/` |
| Desktop app | `ui/desktop/` |
| Text interface | `ui/text/` |
| Provider configuration | `crates/ponduin-providers/` |
| MCP extensions | `crates/ponduin-mcp/` |

## Distribution controls

Before an authorized build is released:

1. Confirm written PondSec approval and the intended audience.
2. Use only the Ponduin name, logo, bundle identifiers, and update endpoints.
3. Include the current root `LICENSE` and `THIRD_PARTY_NOTICES.md`.
4. Preserve all independent third-party notices in
   `crates/ponduin-mcp/licenses/`.
5. Run the full build, test, lint, and packaging checks documented in
   `AGENTS.md`.

Questions about authorization and commercial distribution should be directed
to PondSec through [pondsec.com](https://pondsec.com).
