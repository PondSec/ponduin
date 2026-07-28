# @pondsec/ponduin-sdk

TypeScript client library for the Ponduin Agent Client Protocol (ACP).

This package provides:
- TypeScript types and Zod validators for Ponduin ACP extension methods
- A client for communicating with the Ponduin ACP server

## Installation

```bash
npm install @pondsec/ponduin-sdk
```

The native `ponduin` binaries are distributed as optional dependencies
and will be automatically installed for your platform.

## Development

### Prerequisites

- Node.js 18+
- Rust toolchain
- (Optional) Cross-compilation toolchains for building all platforms

### Building

```bash
# Build everything (schema + TypeScript)
npm run build

# Build just the schema (requires Rust)
npm run build:schema

# Build just the TypeScript
npm run build:ts

# Build native binary for current platform
npm run build:native

# Build native binaries for all platforms
npm run build:native:all
```

### Local Development with npm link

To use this package locally in another project (e.g., `@pondsec/ponduin`):

```bash
# In ui/sdk
npm run build
npm link

# In ui/text (or another project)
npm link @pondsec/ponduin-sdk
```

### Schema Generation

The TypeScript types are generated from Rust schemas defined in `crates/ponduin`.
The build process:

1. Builds the `generate-acp-schema` Rust binary
2. Runs it to generate `acp-schema.json` and `acp-meta.json`
3. Uses `@hey-api/openapi-ts` to generate TypeScript types and Zod validators
4. Generates a typed client in `src/generated/client.gen.ts`

To regenerate schemas after changing Rust types:

```bash
npm run build:schema
```

## Native Binary Packages

Platform-specific npm packages for the `ponduin` binary are located in
`ui/ponduin-binary/`:

| Package | Platform |
|---------|----------|
| `@pondsec/ponduin-binary-darwin-arm64` | macOS Apple Silicon |
| `@pondsec/ponduin-binary-darwin-x64` | macOS Intel |
| `@pondsec/ponduin-binary-linux-arm64` | Linux ARM64 |
| `@pondsec/ponduin-binary-linux-x64` | Linux x64 |
| `@pondsec/ponduin-binary-win32-x64` | Windows x64 |

These are published separately from `@pondsec/ponduin-sdk`.

### Building Native Binaries

```bash
# Build for current platform
npm run build:native

# Build for all platforms (requires cross-compilation toolchains)
npm run build:native:all

# Build for specific platform(s)
npx tsx scripts/build-native.ts darwin-arm64 linux-x64
```

## Publishing

Publishing is handled by GitHub Actions. See `.github/workflows/publish-npm.yml`.

For manual publishing:

```bash
# From repository root
./ui/scripts/publish.sh --real
```

This will:
1. Build and publish `@pondsec/ponduin-sdk`
2. Publish all native binary packages
3. Publish `@pondsec/ponduin` (which depends on the above)

## Usage

```typescript
import { PonduinClient } from "@pondsec/ponduin-sdk";

const client = new PonduinClient({
  // ... configuration
});

// Use the client
const result = await client.someMethod({ ... });
```

See the [main documentation](../../README.md) for more details.
