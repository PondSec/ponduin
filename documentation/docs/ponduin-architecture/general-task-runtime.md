---
title: General Task Runtime
sidebar_position: 5
---

# General Task Runtime

The general task runtime is the provider-independent foundation for bounded,
resumable local work. Its in-memory state is implemented in
`crates/ponduin/src/task.rs`; the durable runtime in
`crates/ponduin/src/task/durable.rs` persists and coordinates that same state.
It keeps runtime facts separate from model-generated plans.

## Task and goal state

A task retains the original user goal and owns a bounded hierarchy of goals.
Each goal has an identifier, parent, dependencies, capability requirements,
action budget, status, and evidence. The task state is authoritative:

- a model may propose a goal or replacement plan, but cannot complete it by
  writing text;
- a completed goal needs valid runtime evidence;
- dependencies must be complete before a dependent goal can start;
- an obsolete plan step is retained for audit while its replacement receives a
  new goal identifier;
- a blocked goal does not implicitly complete unrelated goals.

Task states are `running`, `paused`, `waiting`, `blocked`, `completed`,
`failed`, and `cancelled`. `waiting` carries a structured user-input request;
it is not used to claim background execution. A request contains the missing
information, why it is needed, the blocked goal, and any safe options.

## Long-horizon memory and checkpoints

The runtime stores compact, structured memory only: the original goal,
assumptions, open questions, completed goals, known failures, failed
strategies, capabilities, and relevant resources. Collections are bounded and
tool output bodies are not kept in the task summary.

`TaskCheckpoint` serializes the complete authoritative state. On resume, the
host refreshes resource fingerprints before running another action. Evidence
whose resource fingerprint changed is marked invalid. Completion then fails
closed until fresh evidence is recorded.

The bounded event history records operational facts without chain-of-thought:

```text
TaskStarted → GoalAdded → GoalStarted → ActionRecorded
           → EvidenceAdded → Replanned → GoalCompleted → TaskCompleted
```

The same history records pauses, resumes, policy blocks, user-input waits, and
evidence invalidation.

## Durable lifecycle and storage

The durable runtime has one authoritative state model: `TaskRuntime` remains
the source of truth, wrapped by a versioned `DurableTaskState`. The wrapper
adds workspace identity, the repository head when locally available, durable
tool-call state, artifact fingerprints, review state, and completion reason.
It deliberately does not persist arbitrary prompts, model context, raw tool
output, or secrets.

Task state is stored locally under one directory per task, normally
`.ponduin/tasks/<task-id>/` in the selected workspace:

```text
state.json       atomic, inspectable state snapshot
journal.jsonl    append-only execution journal
```

Each snapshot is written to a unique temporary file, synced, atomically
renamed, and followed by a parent-directory sync. A truncated `state.json` is
rejected rather than partially restored. `schema_version` is explicit; the
runtime currently migrates the initial v0 wrapper to v1 and rejects future
unknown versions fail-closed.

The journal is derived from the runtime event sequence and is synced after the
snapshot. On a restart, a valid snapshot with journal records missing because
of an interruption replays those missing event records; a malformed journal is
rejected. This preserves a compact snapshot for fast loading without losing
the inspectable causal history.

The lifecycle is:

```text
created → running → paused/waiting → resumed → completed|blocked|failed|cancelled
                 └─ checkpoint → new process → load → recovery → refresh → resume
```

`ponduin task run`, `list`, `inspect`, `resume`, `input`, `steer`, `cancel`,
and `events` use this API directly. `resume` creates a new runtime instance,
performs recovery, refreshes tracked workspace digests, invalidates only
affected evidence, and creates replacement goals only where a replan is
required.

## Execution journal and tool recovery

The durable journal uses the runtime's stable task, goal, tool-call, and event
sequence identifiers. It records creation, goals, plan revisions, tool
request/start/success/failure, evidence and artifact changes, budget use,
checkpoints, recovery, user steering, review, and terminal state. It contains
structured summaries, not unrestricted outputs.

Every durable tool call stores its descriptor and an execution contract:

- read/write access and side effect are policy controls;
- idempotence, retry safety, recoverability, and resumability decide whether a
  call interrupted after `ToolStarted` may be retried;
- a required relative scope and declared artifact paths constrain recorded
  mutations;
- an optional timeout turns a late result into a failed action; and
- the existing retry semantics control a bounded retry only for a safe
  contract.

A crash after `ToolStarted` never becomes a synthetic success. A read-only,
idempotent, retry-safe, recoverable call is returned to `requested` for an
explicit retry. A potentially mutating or otherwise unsafe call becomes
`unknown_outcome` and the task waits for a persisted user decision. A tool
already recorded as successful is not invoked again by resume.

Action recording consumes the same task and per-goal budgets as the Phase 3
runtime and emits a `BudgetConsumed` event. Completion still needs valid
evidence for each completed goal.

## Workspace refresh and human steering

Evidence and changed artifacts retain BLAKE3 resource fingerprints. Resume
recomputes each tracked resource, including deletion. Changed evidence is
invalidated while unrelated evidence is retained. A completed goal with stale
evidence can be made obsolete and replaced by a new goal; the old goal remains
in the journal for audit.

Additional user guidance is a persisted `UserSteering` event and a normalized
constraint. The caller names the affected goals, avoiding hidden broad plan
changes. Only those goals are invalidated or replaced; an unrelated pending or
completed goal remains unchanged. A paused task is safely refreshed before
such a controlled replan and returns to paused state afterwards.

## Surface boundary

The CLI is the integrated surface and performs real store creation, process
boundary reload, inspection, and recovery. `TaskStore` and `DurableTask` are
the shared Rust boundary intended for ACP and desktop rather than a second
surface-specific state machine.

ACP still owns its existing session lifecycle and desktop still owns its
session-oriented view model. Neither surface has been rewired to create or
execute these durable tasks in this change, so ACP and desktop integration are
not claimed as complete. Their required next step is to map an ACP or desktop
task control to a `TaskId` and call this shared boundary; no parallel planner
or persistence format should be added.

## Failure states

The durable layer has explicit outcomes: retry scheduled, require user input,
blocked by a budget or policy, failed tool action, invalid checkpoint/journal,
and stale-evidence replan. It does not silently continue after a provider or
tool failure. Provider-specific timeout and error handling remains in the
provider/session integration and is not yet forwarded into this journal.

## Planning, recovery, and budgets

Planning and execution are separate. A replan needs a concrete reason and
consumes a finite replan budget. The former goal becomes `obsolete`, so its
observations remain auditable and cannot be silently retried. The runtime also
enforces task and per-goal action budgets, repair attempts, repeated failure
fingerprints, process time, tool errors, total-duration metadata, and prompt
context size.

The expected validation order remains cost-aware:

```text
cheap relevant observation → targeted validation → broad verification
```

A cheap result cannot replace the evidence required by the requested end state.

## Capability registry and progressive disclosure

`ToolRegistry` describes each internal tool with its domain, read/write access,
risk, preconditions, side effect, required capability, produced evidence, cost,
latency, network requirement, workspace requirement, and retry semantics.

`ToolDisclosureRequest` exposes only tools relevant to a task domain and
current policy. It filters unavailable capabilities, write operations when
write permission is absent, network-required tools when network is disallowed,
and tools whose workspace or risk requirements are not met. Expanding a skill
adds only that skill's domains and capabilities; it never grants every tool.

The bundled skills are deliberately small:

- `coding` for repository changes and validation;
- `git` for repository state;
- `filesystem` for scoped local organization;
- `system-inspection` for read-only facts;
- `document-processing` for structured local text inspection.

Skills provide guidance and validation patterns. They do not create another
agent and cannot weaken permission, workspace, secret, or Git boundaries.

## Safe local tooling

The filesystem service reuses the coding workspace boundary. It resolves
canonical in-workspace paths, rejects traversal and escaping symlinks, limits
search results, classifies metadata without reading content, and only copies or
moves files after an explicit write policy. It never overwrites a destination,
and it rejects mutations of recognized sensitive files such as `.env` and
private keys.

System inspection is read-only and reports observed operating-system,
architecture, logical CPU, RAM, disk, process-count, network-interface,
executable, and developer-tool availability facts. Unavailable facts remain
unavailable rather than being inferred.

Existing coding process and Git services remain the execution layer for coding
tasks; this runtime does not add a second general shell executor.

## Completion and security model

Task completion is allowed only when every non-obsolete goal is completed with
valid evidence. A process exit code, model statement, or plan text alone is not
completion evidence. Policy denial, workspace escape, sensitive-file access,
and stale resource evidence remain explicit non-success states.

Repository files and task inputs are untrusted data. They cannot alter the
runtime's workspace boundary, disclosure policy, permission mode, or system
instructions.
