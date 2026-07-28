---
title: Coding Agent Architecture
sidebar_position: 4
---

# Coding Agent Architecture

This document records the technical baseline and target architecture for
ponduin's local coding capabilities. The design adds an internal coding agent
to the existing agent core and integrates with the provider, permission,
context, CLI, and desktop systems. It does not replace the general agent,
depend on the extension system, or require a cloud service.

## Design constraints

- Existing providers, extensions, recipes, sessions, interfaces, and permission
  modes remain compatible.
- The coding agent and its core tools are internal agent capabilities, not MCP
  or platform extensions.
- Coding behavior is model-independent and works with local providers.
- Core repository analysis, search, editing, Git inspection, and validation work
  offline.
- Repository content is untrusted data, not an instruction source.
- Mutating operations stay inside an explicitly selected workspace.
- Only the explicitly selected `auto` permission mode executes allowed coding
  operations without confirmation.
- Hard security boundaries remain enforced in every permission mode.
- A test is only reported as successful when its process ran and exited
  successfully.
- Optional language servers, embeddings, and external services cannot be
  prerequisites for the core workflow.

## Repository baseline

The repository is a Rust workspace with Electron, React, TypeScript, Python, and
documentation subprojects.

| Area | Existing implementation | Coding-agent decision |
| --- | --- | --- |
| Workspace and package management | Root `Cargo.toml`, Hermit, `Justfile`, pnpm workspaces, Python `pyproject.toml` files | Reuse the existing build environments and detect commands from project manifests |
| Agent loop | `crates/ponduin/src/agents/agent.rs` | Add a coding strategy layer around the existing loop |
| Built-in tools | `agents/platform_extensions/developer` and `agents/platform_extensions/analyze` | Reuse their proven algorithms where appropriate, but expose coding tools directly from the agent core |
| Extension system | `McpClientTrait`, `PlatformExtensionDef`, `PLATFORM_EXTENSIONS`, external MCP support | Preserve it for general-agent compatibility; the internal coding agent does not register or dispatch its tools through it |
| Providers | `Provider`, `ProviderDef`, provider registry, declarative providers, ACP providers | Derive coding behavior from model capabilities without provider-specific branches |
| Model metadata | `ModelConfig`, `ModelInfo`, canonical model catalog | Add a backward-compatible capability profile that consumes existing metadata |
| Prompts | `PromptManager`, templates, extension instructions, prompt overrides | Compose internal coding-mode instructions through keyed prompt extras |
| Context | token counting, structured compaction, tool-pair summarization, fast-model fallback | Add repository context selection before content reaches conversation history |
| Project instructions | `.ponduinhints`, `AGENTS.md`, referenced files, hierarchical subdirectory hints | Preserve the loader and expose the resolved hierarchy to repository analysis |
| File editing | exact unique text replacement and full-file writes | Add workspace enforcement, content versions, preview, patching, atomic batches, and rollback |
| Shell execution | separate stdout/stderr, timeout, cancellation, output limits, process cleanup | Add a deterministic command policy, bounded environment, and explicit execution records |
| Security | permission inspector, security scanner, egress inspector, adversary inspector | Add always-on deterministic workspace and command guards before optional classifiers |
| Loop control | maximum turns, recipe retries, repetition inspector, stop-hook cap | Add coding-progress fingerprints for errors, diffs, and validation attempts |
| Git | hardened internal `git_command` helper and limited internal Git usage | Add first-class read tools and policy-gated write tools; never use repository hooks |
| Storage | SQLite sessions, extension data, structured summaries | Store coding task metadata only; do not persist source bodies or secrets by default |
| CLI | `ponduin-cli`, interactive session, recipes, review command | Add task-mode and repository-oriented commands without changing existing defaults |
| Desktop | Electron/React UI over ACP with extension, permission, session, and diagnostics views | Add coding status, plan, files, diffs, and validation results through ACP types |
| Logging | structured tracing, local rolling files, optional OTLP/Langfuse, opt-in PostHog | Redact coding arguments and paths where necessary; retain explicit opt-in for network telemetry |
| Tests and CI | Rust unit/integration tests, Vitest, Playwright, Docusaurus tests, Clippy and formatting jobs | Add focused fixtures and preserve the existing CI commands |
| Documentation | Docusaurus guides and architecture pages | Document configuration, safety, local-model setup, workflows, and verified limits |

The source snapshot contains a single squashed ponduin baseline commit, so an
exact file-by-file comparison with the original Goose history is unavailable.
Goose-derived concepts that remain visible include the agent loop, MCP
extensions, provider abstraction, recipes, sessions, permission modes, and
context compaction. These are treated as compatibility surfaces.

## Current strengths

The existing implementation already provides:

- a default-enabled developer extension with write, exact edit, shell, tree, and
  image tools;
- a default-enabled tree-sitter analyzer for Rust, Python, JavaScript,
  TypeScript, TSX, Go, Java, Kotlin, Ruby, and Swift;
- Git-ignore-aware directory walking;
- function, class, import, call, and cross-file call-graph extraction;
- bounded shell runtime, cancellation, separate streams, exit codes, and output
  truncation;
- a hardened internal Git command constructor that disables unsafe repository
  configuration such as filesystem monitor hooks;
- hierarchical project hints and `AGENTS.md` discovery;
- context thresholds, structured summaries, and configurable fast-model use;
- provider-independent model names, context limits, output limits, reasoning
  metadata, tool shims, and canonical model metadata;
- permission routing, optional prompt-injection classifiers, egress checks, and
  repeated-tool-call limits.

## Gaps that must be closed

The existing extension-backed developer and analyzer tools resolve absolute
paths and relative paths without enforcing a canonical workspace boundary.
File writes can overwrite an existing file without a prior content version,
and exact edits do not detect a change that happened after the model read the
file. There is no atomic multi-file edit or rollback journal. These tools
remain compatible for general sessions but are not the security boundary for
the internal coding agent.

Repository analysis exposes syntax structure but not a reusable repository
profile, framework and entry-point detection, build and test discovery,
sensitive-file classification, incremental index, or token-budgeted context
selection.

Shell safety relies heavily on permission mode and optional classifiers.
Deterministic blocking for destructive commands must be active in every mode.
When the user explicitly selects `auto`, allowed coding operations run without
confirmation. `approve` and `smart_approve` continue to route confirmation
requests through the existing permission system, and `chat` exposes no coding
tools.

Git does not have a first-class tool surface for status, branch, diffs, history,
staging, commits, or guarded rollback. Validation results are not normalized
into successful, failed, unavailable, skipped, or blocked states.

`PonduinMode` is a permission-routing mode (`auto`, `approve`,
`smart_approve`, or `chat`). It must remain that way for compatibility. Coding,
debugging, refactoring, repository analysis, test generation, documentation, and
review are task strategies, not permission modes.

## Target module layout

The reusable core belongs below `crates/ponduin/src/coding/`:

```text
coding/
├── mod.rs
├── agent.rs
├── capabilities.rs
├── config.rs
├── context.rs
├── git.rs
├── instructions.rs
├── patch.rs
├── repository.rs
├── report.rs
├── search.rs
├── strategy.rs
├── task.rs
├── tools.rs
├── validation.rs
└── workspace.rs
```

The modules have narrow responsibilities:

- `workspace`: canonical root, allowed paths, traversal checks, symlink checks,
  file identity, and safe temporary files;
- `repository`: repository profile, languages, manifests, frameworks, entry
  points, ignored/generated/sensitive paths, and cached fingerprints;
- `search`: filename, text, regex, import, test, TODO, and symbol queries with
  bounded results;
- `patch`: content versions, previews, unified patches, atomic edit batches, and
  rollback records;
- `git`: side-effect-free repository inspection plus explicitly policy-gated
  writes using the hardened Git process constructor;
- `validation`: detected commands, bounded execution, and normalized outcomes;
- `capabilities`: provider-neutral model capability profile and workload sizing;
- `context`: ranked repository snippets, token budgets, summaries, cache
  invalidation, and incremental selection;
- `agent`: internal coding-agent lifecycle and integration with the existing
  reply loop;
- `strategy`: task-mode policies and prompt/tool presets;
- `task`: plan, status, attempts, errors, diffs, validations, and progress
  fingerprints;
- `tools`: internal tool schemas and direct dispatch to coding services;
- `report`: factual completion reports derived only from task records;
- `config`: typed, validated views over the existing ponduin configuration;
- `instructions`: resolved repository instructions tagged as untrusted project
  context.

The existing surfaces consume this core:

- `Agent` owns an optional internal `CodingAgent` and directly appends its tools
  beside existing internal platform tools;
- tool dispatch recognizes internal coding tool names before extension
  resolution, following the same pattern as existing internal final-output and
  scheduling tools;
- reusable parsing and process algorithms may be moved out of the existing
  developer and analyzer extensions into neutral core services, while their
  public extension tools remain as compatibility adapters;
- `PromptManager` adds a keyed internal coding strategy prompt without
  replacing the base system prompt;
- the agent loop owns a coding task record only when an internal coding
  strategy is active;
- ACP and SDK types expose task state to CLI and desktop clients;
- the desktop reuses current session, extension, permission, and diff
  components.

## Task strategies

`CodingTaskMode` is a separate serialized enum:

- `general`
- `coding`
- `debugging`
- `refactoring`
- `repository_analysis`
- `test_generation`
- `documentation`
- `review`

Each strategy selects instructions, recommended tools, planning threshold,
validation behavior, and report sections. It is configured independently from
`PonduinMode`.

`PonduinMode` remains the sole confirmation setting:

- `auto`: allowed internal coding tools execute without asking;
- `smart_approve`: sensitive internal coding tools ask;
- `approve`: every mutating internal coding tool asks;
- `chat`: internal coding tools are unavailable.

Workspace escape, path traversal, symlink escape, protected secret access, and
hard-denied destructive commands are rejected even in `auto`. This is a denial,
not a confirmation request.

Small tasks may edit directly when they remain below the configured file and
risk thresholds. Larger tasks produce a plan with:

- goal and assumptions;
- relevant components and files;
- risks and rollback approach;
- intended changes;
- tests and other validation.

## Model capability profile

`ModelCapabilities` is derived from the current `ModelConfig`, provider model
metadata, and optional user overrides. Unknown fields use conservative values.

The profile includes:

- context window and maximum output;
- native or emulated tool calling;
- structured output;
- reasoning;
- coding suitability;
- text and image input;
- embeddings;
- relative speed and resource class.

The profile controls chunk size, number of files per context batch, planning
granularity, and maximum autonomous repair scope. A small local model gets
smaller batches and more explicit steps. A stronger model uses the same
interfaces with larger bounded work units.

## Workspace security boundary

The selected session working directory is the default coding workspace. Every
path operation follows this sequence:

1. Reject empty paths and lexical parent traversal.
2. Resolve relative paths against the canonical workspace root.
3. Canonicalize the nearest existing ancestor.
4. Reject paths whose canonical ancestor is outside the workspace.
5. Reject symlinks that resolve outside the workspace.
6. Recheck the destination immediately before mutation.
7. Compare the expected content digest when the operation depends on a prior
   read.

Reads of explicit external paths can only be added later behind a separate
permission. Writes, moves, deletes, patches, shell working directories, and Git
operations cannot escape the workspace. `auto` suppresses approval prompts for
allowed operations; it does not expand the workspace.

Repository files can contain prompt injection. Instruction files are labelled
as repository-provided context and cannot change permission rules, workspace
roots, provider settings, or system instructions.

## File mutation protocol

Full-file overwrite remains available for new, small files. Existing files use
patch-oriented changes by default.

A mutation request contains the expected digest from a prior read. The patch
engine:

1. loads the current bytes;
2. verifies the digest;
3. applies the change in memory;
4. validates all paths and patches in a batch;
5. writes temporary files inside the workspace;
6. atomically replaces destinations;
7. retains an in-memory or task-scoped rollback record;
8. returns a structured diff and new digest.

Batch failure leaves all original files intact. Rollback applies only changes
recorded by the current task and fails closed when another process changed a
file afterward.

## Shell and Git policy

Shell commands retain the existing timeout, cancellation, output limits, and
separate stream capture. The coding layer adds:

- a canonical workspace working directory;
- a bounded environment with an explicit inheritance policy;
- maximum timeout and output settings;
- deterministic parsing and classification;
- rejection of interactive commands unless explicitly supported;
- task records for command, exit code, duration, and truncation state.

Commands that target roots, user accounts, system packages, credentials,
production resources, or irreversible deletion are denied. In `auto`, other
allowed commands execute without approval. In `smart_approve` and `approve`,
material network, install, or destructive impact follows the existing approval
rules. The policy is additive to the existing security inspectors.

Git reads use the existing hardened `git_command()` helper. Repository status,
branch, diff, staged diff, log, changed files, and untracked files are read-only.
Branch creation, staging, committing, and rollback are separate mutating tools.
Push, force push, branch deletion, and hard reset are not part of the autonomous
core workflow.

## Validation model

Command detection reads manifests, CI files, `Makefile`, `Justfile`, and
repository documentation without executing them. Detected commands include
their evidence and confidence.

Each validation produces one of:

- `passed`
- `failed`
- `not_found`
- `unavailable`
- `skipped`
- `blocked`

The record contains the command, working directory, exit code, duration,
bounded stdout and stderr, and reason. Completion reports are generated from
these records, so an unexecuted check cannot be presented as passed.

## Coding loop and progress detection

The coding strategy uses the current agent loop:

1. understand the task;
2. inspect repository metadata;
3. select relevant files and symbols;
4. create a plan when required;
5. apply controlled mutations;
6. run relevant validation;
7. classify failures;
8. perform a bounded targeted repair;
9. review the final diff;
10. produce a factual report.

Repair attempts are limited by configuration. A progress fingerprint includes
the normalized error, changed-file digests, diff digest, validation command,
and tool-call signature. Repeated fingerprints, unchanged diffs, or a growing
error count stop automatic repair and report the block.

## Configuration

Coding settings use the existing config file and environment-variable
resolution. Initial keys are:

```text
PONDUIN_CODING_ENABLED
PONDUIN_CODING_MODE
PONDUIN_CODING_MAX_ITERATIONS
PONDUIN_CODING_MAX_REPAIR_ATTEMPTS
PONDUIN_CODING_MAX_CONTEXT_TOKENS
PONDUIN_CODING_MAX_FILES_PER_BATCH
PONDUIN_CODING_PLAN_FILE_THRESHOLD
PONDUIN_CODING_AUTO_TEST
PONDUIN_CODING_AUTO_FORMAT
PONDUIN_CODING_INDEXING
PONDUIN_CODING_LSP
PONDUIN_CODING_TREE_SITTER
PONDUIN_CODING_EMBEDDINGS
PONDUIN_CODING_SHELL_TIMEOUT
PONDUIN_CODING_OUTPUT_LIMIT
```

Defaults enable local analysis and Tree-sitter, keep embeddings and language
servers optional, and limit iterations and timeouts. Confirmation behavior is
always taken from the explicit `PonduinMode` setting; coding configuration
cannot silently enable `auto`.

## Implementation phases

### Phase 1: architecture

1. Record the current architecture and gaps.
2. Establish the reusable coding module boundary.
3. Define compatibility, security, configuration, and test rules.

### Phase 2: coding foundations

1. Implement the internal coding-agent tool registry and direct dispatch.
2. Implement workspace boundaries and repository detection.
3. Reuse the hint loader for hierarchical instructions.
4. Add bounded filename and text search.
5. Add versioned reads, patch preview, atomic edit batches, and rollback.
6. Add deterministic shell policy while retaining the existing runner.
7. Add safe Git inspection and command discovery.
8. Test traversal, symlink escape, conflicts, timeout, command blocking, and
   Git hook isolation.

### Phase 3: repository intelligence

1. Build repository profiles, language and framework adapters, and entry-point
   detection.
2. Refactor the analyzer behind a symbol-index interface.
3. Add repository maps and incremental fingerprints.
4. Add ranked, token-budgeted context selection.
5. Add model capabilities and workload sizing.

### Phase 4: coding workflow

1. Add task strategies and planning thresholds.
2. Add normalized validation.
3. Add bounded repair and debugging workflows.
4. Add progress fingerprints and loop detection.
5. Add structured task status and completion reports.

### Phase 5: integration and advanced adapters

1. Add test-generation, refactoring, and review strategy behavior.
2. Add optional language-server and embedding adapters.
3. Expose task state through ACP, SDK, CLI, and desktop views.
4. Reuse existing diff, extension, permission, and session UI components.

### Phase 6: stabilization

1. Run unit, integration, regression, formatting, Clippy, type, UI, and
   documentation checks.
2. Verify Rust, TypeScript/web, Python backend, and mixed-project fixtures.
3. Test small-context capability profiles and stronger-model profiles without
   making network calls.
4. Review security boundaries, performance, compatibility, and final diffs.
5. Document local setup, Ollama, Qwen3:8B, stronger models, workflows, limits,
   and troubleshooting.

## Test matrix

Core fixtures stay small and deterministic:

| Fixture | Required evidence |
| --- | --- |
| Rust/Cargo | manifests, symbols, tests, build and Clippy command detection |
| TypeScript/web | pnpm/npm scripts, components, imports, typecheck and test detection |
| Python backend | `pyproject.toml`, package imports, pytest and type/lint command detection |
| Go service | `go.mod`, packages, tests, build and vet detection |
| Mixed repository | nested instructions, multiple manifests, generated and ignored paths |
| Security repository | traversal paths, escaping symlinks, malicious scripts, secrets, Git hooks |

Tests verify behavior rather than requiring every external tool to be installed.
Unavailable tools produce an `unavailable` validation result.

## Compatibility and migration

- Existing sessions default to `general` task mode.
- Existing `PonduinMode` serialization is unchanged.
- Existing extension tool names remain valid, while internal coding tools use a
  reserved `coding__` namespace.
- Stronger safety checks can reject operations that were previously unsafe; the
  rejection includes a precise reason and safe alternative.
- New model fields are optional and conservative when absent.
- New configuration keys have safe defaults and do not require migration.
- Existing providers do not need to implement a new trait method.
- External MCP extensions and ACP providers continue to use their current
  interfaces, but are not required by the internal coding agent.

## Completion evidence

The coding architecture is complete only when the acceptance criteria are
covered by executable tests and the final report distinguishes passed, failed,
unavailable, skipped, and blocked checks. Documentation and UI claims must not
exceed the verified implementation.
