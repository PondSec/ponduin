---
title: Internal Coding Agent
sidebar_position: 11
sidebar_label: Internal Coding Agent
---

# Internal Coding Agent

ponduin includes a provider-independent coding agent in its core. It does not
use an extension or require a cloud service. The same repository, editing,
validation, review, and Git tools work with local Ollama models,
OpenAI-compatible local servers, llama.cpp, LM Studio, vLLM, and the existing
optional cloud providers.

The coding agent is disabled by default. Enabling it does not change the
current provider or permission mode.

## Enable it

### Desktop

1. Open **Settings**.
2. Open **Chat**.
3. Turn on **Enable internal coding agent**.
4. Choose a task mode.
5. Restart ponduin.

Select **Autonomous** as the ponduin permission mode only when ordinary coding
tool calls should run without confirmation. Hard security denials remain
active.

### CLI

Run:

```bash
ponduin configure
```

Choose the internal coding-agent configuration, enable it, select a workflow,
and restart the active CLI session.

The equivalent minimal `config.yaml` settings are:

```yaml
PONDUIN_CODING_ENABLED: true
PONDUIN_CODING_MODE: coding
```

Environment variables with the same names override saved configuration for the
current process.

## Task modes and permissions

Task mode controls the workflow. Permission mode independently controls
confirmation.

| Coding task mode | Behavior |
| --- | --- |
| `coding` | Implement in small patches and validate each step |
| `debugging` | Form and test a cause hypothesis before editing |
| `refactoring` | Preserve behavior through reversible structural changes |
| `repository_analysis` | Map architecture and risks; read-only by default |
| `test_generation` | Reuse the existing test framework and add behavior-focused tests |
| `documentation` | Document repository-verified behavior |
| `review` | Report actionable local-diff findings; read-only by default |

| `PONDUIN_MODE` | Coding-tool behavior |
| --- | --- |
| `auto` | Allowed tools run without confirmation |
| `smart_approve` | Read-only tools run; mutations and sensitive actions ask |
| `approve` | Tool calls ask for approval |
| `chat` | Coding and extension tools are unavailable |

`auto` is the only setting that removes ordinary confirmation. It never allows
workspace escape, protected secret access, destructive system commands,
unowned Git mutations, force-push, or hard reset.

## Supported internal tools

The tools are registered directly in the agent core under the reserved
`coding__` namespace.

| Area | Tools |
| --- | --- |
| Repository discovery | `repository_profile`, `repository_instructions`, `project_capabilities`, `repository_map` |
| Search and context | `find_files`, `search_text`, `search_symbols`, `find_references`, `select_context`, `prepare_context` |
| Versioned editing | `read_file`, `preview_changes`, `apply_changes`, `rollback_changes` |
| Processes and validation | `run_process`, `run_validation` |
| Git | `git_status`, `git_diff`, `git_history`, `git_create_branch`, `git_stage_owned`, `git_unstage_owned`, `git_commit_owned`, `git_push_owned`, `git_revert_owned` |
| Workflow evidence | `workflow_start`, `workflow_set_plan`, `workflow_update_memory`, `workflow_transition`, `workflow_status`, `workflow_complete` |
| Advanced analysis | `review_changes`, `lsp_query` |

Repository intelligence works without a language server. Tree-sitter supplies
native syntax information for supported languages, while bounded lexical
fallbacks cover other text-based projects. Language-server and local embedding
adapters are opt-in.

## Local setup with Ollama and Qwen3:8B

Qwen3:8B is small enough for many 16 GB Apple Silicon systems, but it has less
planning reliability than a larger coding model. ponduin compensates by using
smaller contexts, fewer files per change, and sequential validation.

Install and start Ollama, then pull the model:

```bash
ollama pull qwen3:8b
OLLAMA_CONTEXT_LENGTH=32768 ollama serve
```

In another terminal, run `ponduin configure` and select:

- provider: **Ollama**
- host: `http://localhost:11434`
- model: `qwen3:8b`
- internal coding agent: enabled
- task mode: **Coding**

For an 8B model, these conservative optional settings are a useful starting
point:

```yaml
PONDUIN_CODING_ENABLED: true
PONDUIN_CODING_MODE: coding
PONDUIN_CODING_MAX_CONTEXT_TOKENS: 8192
PONDUIN_CODING_MAX_FILES_PER_BATCH: 3
PONDUIN_CODING_PLAN_FILE_THRESHOLD: 1
PONDUIN_CODING_MODEL_TOOL_CALLING: supported
PONDUIN_CODING_MODEL_CODING_SUITABILITY: limited
PONDUIN_CODING_MODEL_SPEED: slow
PONDUIN_CODING_MODEL_RESOURCE_DEMAND: high
```

If the selected model does not call tools reliably, enable the existing Ollama
tool shim. The coding agent remains internal; only tool-call interpretation
uses the shim.

```yaml
PONDUIN_TOOLSHIM: true
PONDUIN_TOOLSHIM_OLLAMA_MODEL: qwen3:8b
```

Do not ask an 8B model to redesign a large application in one step. Start with
a concrete goal, let repository discovery run, and keep the requested change
small enough to validate independently.

## Stronger model example

Larger coding models use the same tools and security policy. For example, after
pulling a locally available coding model:

```bash
ollama pull qwen3-coder:30b
```

select `qwen3-coder:30b` in the existing Ollama provider and use:

```yaml
PONDUIN_CODING_MODEL_TOOL_CALLING: supported
PONDUIN_CODING_MODEL_STRUCTURED_OUTPUT: supported
PONDUIN_CODING_MODEL_CODING_SUITABILITY: strong
PONDUIN_CODING_MODEL_SPEED: balanced
PONDUIN_CODING_MODEL_RESOURCE_DEMAND: high
PONDUIN_CODING_MAX_CONTEXT_TOKENS: 32768
```

The same capability settings can describe a stronger model served by LM
Studio, vLLM, llama.cpp, an OpenAI-compatible local endpoint, or an optional
cloud provider. They are hints, not provider-specific branches.

## Configuration reference

All values use the existing ponduin configuration system.

| Setting | Default | Valid range or values |
| --- | --- | --- |
| `PONDUIN_CODING_ENABLED` | `false` | Boolean |
| `PONDUIN_CODING_MODE` | `general` | The seven coding task modes above, or `general` |
| `PONDUIN_CODING_MAX_ITERATIONS` | `50` | 1–1000 |
| `PONDUIN_CODING_MAX_REPAIR_ATTEMPTS` | `3` | 0–100 |
| `PONDUIN_CODING_MAX_CONTEXT_TOKENS` | `32768` | 1024–1000000 |
| `PONDUIN_CODING_MAX_FILES_PER_BATCH` | `20` | 1–1000; capability limits may reduce it |
| `PONDUIN_CODING_PLAN_FILE_THRESHOLD` | `4` | 1–1000; capability limits may reduce it |
| `PONDUIN_CODING_AUTO_TEST` | `true` | Boolean |
| `PONDUIN_CODING_AUTO_FORMAT` | `false` | Boolean |
| `PONDUIN_CODING_INDEXING` | `true` | Boolean |
| `PONDUIN_CODING_LSP` | `false` | Boolean |
| `PONDUIN_CODING_TREE_SITTER` | `true` | Boolean |
| `PONDUIN_CODING_EMBEDDINGS` | `false` | Boolean |
| `PONDUIN_CODING_SHELL_TIMEOUT` | `120` | 1–3600 seconds |
| `PONDUIN_CODING_OUTPUT_LIMIT` | `2097152` | 1024–104857600 bytes |
| `PONDUIN_CODING_MODEL_TOOL_CALLING` | `unknown` | `unknown`, `unsupported`, `supported` |
| `PONDUIN_CODING_MODEL_STRUCTURED_OUTPUT` | `unknown` | `unknown`, `unsupported`, `supported` |
| `PONDUIN_CODING_MODEL_CODING_SUITABILITY` | `unknown` | `unknown`, `limited`, `general`, `strong` |
| `PONDUIN_CODING_MODEL_MULTIMODALITY` | `unknown` | `unknown`, `unsupported`, `supported` |
| `PONDUIN_CODING_MODEL_EMBEDDING_SUPPORT` | `unknown` | `unknown`, `unsupported`, `supported` |
| `PONDUIN_CODING_MODEL_SPEED` | `unknown` | `unknown`, `slow`, `balanced`, `fast` |
| `PONDUIN_CODING_MODEL_RESOURCE_DEMAND` | `unknown` | `unknown`, `low`, `moderate`, `high` |

Invalid and out-of-range settings fail closed instead of silently expanding
agent authority.

## Typical workflow

A non-trivial coding task follows this evidence-backed sequence:

1. Profile the repository and load hierarchical project instructions as
   untrusted context.
2. Detect languages, manifests, entry points, dependencies, CI, and available
   validation commands.
3. Search symbols and references, then prepare a token-bounded context.
4. Record a plan when the capability-adjusted file threshold is reached.
5. Read complete file versions and preview a digest-bound patch.
6. Apply the complete batch atomically.
7. Run the narrowest relevant validation command.
8. Parse diagnostics and perform no more than the configured repair attempts.
9. Review the local diff and report only recorded results.

Validation distinguishes `passed`, `failed`, `not_present`,
`not_executable`, `skipped`, `blocked`, `timed_out`, and
`incomplete_output`. A command that did not run is never reported as passed.

## Security model

- Every read and write is checked against the canonical workspace.
- Parent traversal and symlink escape are rejected.
- Sensitive files, common secret formats, binaries, oversized files, and
  generated dependency/build directories are excluded.
- Repository instructions are labelled as untrusted and cannot change system
  policy.
- Existing files require the complete BLAKE3 digest from a prior read.
- Multi-file patches are validated before any destination is replaced.
- Rollback refuses to overwrite later user changes.
- Processes run without shell parsing, with a bounded environment, timeout,
  output cap, cancellation, and process-group cleanup.
- Interactive shells, package/system administration, credentials, production
  operations, and destructive commands are blocked.
- Git commands disable repository-controlled hooks and filters. Mutations are
  limited to files whose starting state and digest are owned by the current
  task.
- No repository source body or diagnostic content is persisted in coding task
  memory.
- Core coding functions are offline and add no telemetry.

## Known limits and troubleshooting

Model quality remains the largest variable. A model can still misunderstand a
requirement or choose a weak implementation; the patch, permission, validation,
and report layers limit impact but cannot make weak reasoning correct.

- If no coding tools appear, enable the coding agent, choose a mode other than
  `general`, leave `PONDUIN_MODE` out of `chat`, and restart ponduin.
- If a local model emits text instead of tool calls, use a tool-capable model
  or enable the Ollama tool shim.
- If context is slow or memory use is high, reduce
  `PONDUIN_CODING_MAX_CONTEXT_TOKENS` and
  `PONDUIN_CODING_MAX_FILES_PER_BATCH`.
- If `lsp_query` is blocked, set `PONDUIN_CODING_LSP: true` and install the
  appropriate language server. Core symbol search still works without it.
- If embedding evidence is absent, set `PONDUIN_CODING_EMBEDDINGS: true`.
  Embeddings are local, optional, bounded, and never replace lexical evidence.
- If a validation is `not_executable`, install the repository's required
  runtime or use one of its other discovered validation commands.
- A `blocked` mutation is a security or ownership decision. Inspect the reason
  instead of weakening the workspace boundary.

Large generated repositories, uncommon languages without Tree-sitter support,
and cross-process changes during a patch may reduce available context or cause
a safe conflict. Remote CI, debuggers, and arbitrary interactive programs are
not required for the core local workflow.

For implementation details, see
[Coding Agent Architecture](/docs/ponduin-architecture/coding-agent-architecture).
