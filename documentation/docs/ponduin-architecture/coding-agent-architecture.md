---
title: Coding Agent Architecture
sidebar_position: 4
---

# Coding Agent Architecture

This document records the technical baseline and implemented architecture for
ponduin's local coding capabilities. The implementation adds an internal coding agent
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
| Prompts | `PromptManager`, templates, extension instructions, prompt overrides | Compose model-selected internal coding guidance through keyed prompt extras |
| Context | token counting, structured compaction, tool-pair summarization, fast-model fallback | Add repository context selection before content reaches conversation history |
| Project instructions | `.ponduinhints`, `AGENTS.md`, referenced files, hierarchical subdirectory hints | Preserve the loader and expose the resolved hierarchy to repository analysis |
| File editing | exact unique text replacement and full-file writes | Add workspace enforcement, content versions, preview, patching, atomic batches, and rollback |
| Shell execution | separate stdout/stderr, timeout, cancellation, output limits, process cleanup | Add a deterministic command policy, bounded environment, and explicit execution records |
| Security | permission inspector, security scanner, egress inspector, adversary inspector | Add always-on deterministic workspace and command guards before optional classifiers |
| Loop control | maximum turns, recipe retries, repetition inspector, stop-hook cap | Add coding-progress fingerprints for errors, diffs, and validation attempts |
| Git | hardened internal `git_command` helper and limited internal Git usage | Add first-class read tools and policy-gated write tools; never use repository hooks |
| Storage | SQLite sessions, extension data, structured summaries | Store coding task metadata only; do not persist source bodies or secrets by default |
| CLI | `ponduin-cli`, interactive session, recipes, review command | Make repository-oriented capabilities available without an activation or task-mode command |
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

## Baseline gaps closed by the coding core

The extension-backed developer and analyzer tools remain available for
compatibility, but they are not the internal coding agent's security boundary.
The coding core now enforces a canonical workspace, content-version checks,
atomic multi-file changes, safe moves, and task-scoped rollback.

Repository analysis now provides reusable profiles, language and ecosystem
detection, manifests, dependencies, entry points, generated and sensitive-file
classification, a cached symbol index, reference search, and token-bounded
context selection. Default exclusions and repository ignore files prevent
dependency, build, cache, and generated directories from consuming the index.

The process layer deterministically blocks unsafe commands in every permission
mode. It uses argument vectors rather than shell parsing and records bounded
stdout, stderr, exit status, duration, truncation, timeout, and diagnostics.

Git has first-class status, diff, history, branch, staging, commit, push, revert,
and unstage tools. Mutations require task ownership of every affected digest,
and hardened Git configuration disables repository-controlled hooks and
content filters.

Validation and workflow records distinguish real success from failure, missing
tools, policy skips, blocks, timeouts, and incomplete output.

`PonduinMode` remains a permission-routing mode (`auto`, `approve`,
`smart_approve`, or `chat`). Coding, debugging, refactoring, repository
analysis, test generation, documentation, and review remain independent task
strategies.

## Implemented module layout

The reusable core is implemented below `crates/ponduin/src/coding/`:

```text
coding/
├── mod.rs
├── agent.rs
├── capabilities.rs
├── config.rs
├── context.rs
├── diagnostic.rs
├── embedding.rs
├── file.rs
├── git.rs
├── instructions.rs
├── intelligence.rs
├── lsp.rs
├── patch.rs
├── process.rs
├── project.rs
├── repository.rs
├── review.rs
├── search.rs
├── sensitive.rs
├── strategy.rs
├── tools.rs
├── validation.rs
├── workflow.rs
└── workspace.rs
```

The modules have narrow responsibilities:

- `workspace`: canonical root, allowed paths, traversal checks, symlink checks,
  file identity, and safe temporary files;
- `repository`: repository profile, languages, manifests, frameworks, entry
  points, ignore handling, bounded walking, and repository limits;
- `project`: dependencies, ecosystems, CI, and build/test/lint/type command
  discovery without executing manifests;
- `sensitive`: centralized protected-file classification;
- `search`: bounded filename, text, regex, import, test, TODO, and scoped
  queries;
- `intelligence`: cached repository maps, Tree-sitter-backed symbols,
  references, imports, and source fingerprints;
- `file`: complete versioned reads with BLAKE3 digests;
- `patch`: content versions, previews, unified patches, atomic edit batches, and
  move and rollback records;
- `git`: side-effect-free repository inspection plus explicitly policy-gated
  ownership-safe writes using the hardened Git process constructor;
- `process`: deterministic command policy, bounded environment, process-group
  cleanup, separate streams, timeout, and diagnostic extraction;
- `validation`: detected commands, bounded execution, and normalized outcomes;
- `diagnostic`: structured, redacted Rust, TypeScript, Python, and generic
  diagnostics;
- `capabilities`: provider-neutral model capability profile and workload sizing;
- `context`: ranked repository snippets, token budgets, summaries, cache
  invalidation, and incremental selection;
- `embedding`: optional bounded local hybrid ranking that retains lexical
  evidence;
- `lsp`: optional bounded language-server queries with sanitized locations;
- `review`: severity-ordered local added-line review findings;
- `agent`: internal coding-agent lifecycle and integration with the existing
  reply loop;
- `strategy`: model-routing guidance for semantic per-turn tool selection;
- `workflow`: plan, status, bounded memory, attempts, errors, changed files,
  validations, progress fingerprints, and factual completion reports;
- `tools`: internal tool schemas and direct dispatch to coding services;
- `config`: typed, validated views over the existing ponduin configuration;
- `instructions`: resolved repository instructions tagged as untrusted project
  context.

The existing surfaces consume this core:

- `Agent` owns an internal `CodingAgent` and directly appends its tools
  beside existing internal platform tools;
- tool dispatch recognizes internal coding tool names before extension
  resolution, following the same pattern as existing internal final-output and
  scheduling tools;
- reusable parsing is shared with the existing analyzer where doing so
  preserves its public extension tools;
- `PromptManager` adds keyed internal model-routing guidance without
  replacing the base system prompt;
- the selected model decides semantically whether the current turn needs
  coding tools; there is no keyword, regular-expression, or host-side branch
  that classifies user prompts;
- coding capabilities require no CLI or desktop opt-in and do not change the
  provider;
- the desktop reuses the current permission and session surfaces.

## Model-selected request routing

Every eligible turn exposes the same bounded internal tool set and routing
guidance to the active model. Using the complete conversation, the model decides
whether to answer normally or use an implementation, debugging, refactoring,
repository-analysis, test-generation, documentation, or review approach. It
re-evaluates that decision on each new turn. No task mode is serialized or
selected by the user.

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
Branch creation, staging, unstaging, committing, push, and guarded revert are
separate mutating tools. A push is limited to the current owned branch and
configured remote; force-push, branch deletion, hard reset, repository-local
remotes, and foreign staged content are denied.

## Validation model

Command detection reads manifests, CI files, `Makefile`, `Justfile`, and
repository documentation without executing them. Detected commands include
their evidence and confidence.

Each validation produces one of:

- `passed`
- `failed`
- `not_present`
- `not_executable`
- `skipped`
- `blocked`
- `timed_out`
- `incomplete_output`

The record contains a command fingerprint, working directory, exit code,
duration, normalized diagnostic fingerprint, error count, planned check IDs,
and an evidence scope. Source output is bounded for the current tool response,
but the workflow retains metadata rather than source or diagnostic bodies.
Completion reports are generated from these records, so an unexecuted check
cannot be presented as passed.

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
error count stop automatic repair and report the block. Every repair attempt
requires a recorded approach and a fingerprint of its hypothesis; it cannot
silently retry the same tactic. Hypothesis text is not retained in workflow
memory.

## Causal repair evidence

A failed validation can start one `RepairEpisode`. The episode links a stable
episode ID and workflow ID to the original diagnostic fingerprint and error
count, a hypothesis fingerprint, repair approach, target files, planned
validation binding, applied mutation IDs and file digests, and subsequent
validation bindings. It has an explicit outcome (`verified`, `improved`,
`failed`, `inconclusive`, or lifecycle states) and a progress classification:
meaningful progress, partial progress, no progress, regression, or unknown.

A passed validation is verified only when it matches the repair's intended
validation command or planned check. A smaller error count is partial progress;
a different diagnostic with an equal or greater error count is a regression,
not success. The next repair must select a different deterministic approach
after a non-improving attempt.

Validation evidence records the active mutation IDs, a revision fingerprint,
and the files it covers. A later mutation invalidates only evidence whose
declared coverage overlaps that mutation; unrelated scoped evidence remains
available. Before review and completion, the tool runtime recalculates every
retained mutation's file digest. An external change causes a stale-evidence
error and prevents review or completion.

Capability feedback is also task-local. Successful and failed checks establish
that a command ran, while missing, non-executable, policy-blocked, timed-out,
and incomplete checks retain distinct feedback. Guidance then selects a
permitted alternative or reports a blocker instead of repeatedly invoking the
same unsuitable command. This feedback never becomes global provider state.

## Configuration

Coding settings use the existing config file and environment-variable
resolution. The keys are:

```text
PONDUIN_CODING_MAX_ITERATIONS
PONDUIN_CODING_MAX_REPAIR_ATTEMPTS
PONDUIN_CODING_MAX_CONTEXT_TOKENS
PONDUIN_CODING_MAX_FILES_PER_BATCH
PONDUIN_CODING_AUTO_TEST
PONDUIN_CODING_AUTO_FORMAT
PONDUIN_CODING_INDEXING
PONDUIN_CODING_LSP
PONDUIN_CODING_TREE_SITTER
PONDUIN_CODING_EMBEDDINGS
PONDUIN_CODING_SHELL_TIMEOUT
PONDUIN_CODING_OUTPUT_LIMIT
PONDUIN_CODING_MODEL_TOOL_CALLING
PONDUIN_CODING_MODEL_STRUCTURED_OUTPUT
PONDUIN_CODING_MODEL_CODING_SUITABILITY
PONDUIN_CODING_MODEL_MULTIMODALITY
PONDUIN_CODING_MODEL_EMBEDDING_SUPPORT
PONDUIN_CODING_MODEL_SPEED
PONDUIN_CODING_MODEL_RESOURCE_DEMAND
```

Defaults enable local analysis and Tree-sitter, keep embeddings and language
servers optional, and limit iterations and timeouts. Confirmation behavior is
always taken from the explicit `PonduinMode` setting; coding tuning cannot
silently enable `auto`.

The complete defaults, ranges, local-model examples, and troubleshooting steps
are in the [Internal Coding Agent guide](/docs/guides/internal-coding-agent).

## Delivered implementation phases

### Phase 1: architecture

- Recorded the baseline, extension points, compatibility rules, and security
  boundaries.
- Established the internal provider-independent module and direct dispatch
  boundary.

### Phase 2: coding foundations

- Added workspace boundaries, instructions, search, versioned reads, atomic
  patches and moves, rollback, bounded process execution, and hardened Git
  reads and ownership-safe writes.
- Covered traversal, symlink escape, conflicts, timeout, command blocking, Git
  hook isolation, content filters, and foreign changes.

### Phase 3: repository intelligence

- Added polyglot profiles, ecosystem adapters, entry points, dependencies,
  command discovery, Tree-sitter indexing, repository maps, references,
  fingerprints, ranked token budgets, and model capability sizing.

### Phase 4: coding workflow

- Added task strategies, planning thresholds, normalized validation,
  diagnostics, bounded repair, loop detection, bounded task memory, review, and
  evidence-derived completion reports.

### Phase 5: integration and advanced adapters

- Added optional language-server and local embedding adapters.
- Integrated always-available, model-selected request routing into the existing
  reply loop without GUI or CLI activation steps.
- Kept all coding tools internal and retained existing extension behavior.

### Phase 6: stabilization

- Added mixed Rust, TypeScript, Python, Go, and Java acceptance coverage with a
  real failing validation and repair.
- Added a bounded medium-repository fixture with generated-directory
  exclusions.
- Exercised compact local and stronger-model capability profiles.
- Added updater provenance, workflow, CLI, desktop, documentation, and
  full-workspace regression coverage.

### Phase 7: causal repair reliability

- Added repair episodes that bind diagnostic, hypothesis and approach
  fingerprints to concrete mutations, validation evidence, progress, and
  outcome.
- Added scoped validation invalidation and a runtime digest check before review
  or completion, preventing stale mutation evidence from proving success.
- Added local capability feedback and deterministic scenarios for alternative
  repair selection, partial progress, regression, timeout, and external file
  changes.

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
| Repair reliability | causal failure-to-repair links, stale evidence, partial progress, regression, timeout, and alternative strategy selection |

Tests verify behavior rather than requiring every external tool to be installed.
Unavailable tools produce a `not_executable` validation result.

## Compatibility and migration

- Existing coding enable/task-mode keys are ignored; coding capabilities are
  core behavior and the model selects their use per request.
- Existing `PonduinMode` serialization is unchanged.
- Existing extension tool names remain valid, while internal coding tools use a
  reserved `coding__` namespace.
- Stronger safety checks can reject operations that were previously unsafe; the
  rejection includes a precise reason and safe alternative.
- New model fields are optional and conservative when absent.
- Coding tuning keys have bounded defaults and do not require migration.
- Existing providers do not need to implement a new trait method.
- External MCP extensions and ACP providers continue to use their current
  interfaces, but are not required by the internal coding agent.

## Completion evidence

Executable tests cover repository detection, default exclusions, file and
symbol search, reference discovery, bounded context, conflicts, atomic
multi-file changes, rollback, traversal and symlink escape, process timeout and
blocking, Git reads and owned writes, validation discovery and classification,
capability sizing, iteration limits, loop detection, review findings, factual
reports, and compatibility behavior.

The workflow reliability tests additionally verify that completion cannot reuse
validation evidence after an overlapping mutation, an externally altered
retained file blocks review, a repair cannot begin without its failure evidence
and approach, equivalent failed approaches are rejected, and timeout or missing
validation never becomes a success claim. The tests are deterministic and do
not require a live provider.

The mixed-project acceptance test discovers five ecosystems, applies and rolls
back a three-file change atomically, runs a real successful Cargo check,
captures an injected syntax failure as `failed`, repairs it, and observes a
subsequent `passed` result. The medium-repository test indexes 900 relevant
files while excluding 300 generated dependency files and keeps profile,
search, index, and context work bounded.
