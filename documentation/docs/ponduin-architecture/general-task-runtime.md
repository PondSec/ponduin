---
title: General Task Runtime
sidebar_position: 5
---

# General Task Runtime

The general task runtime is the provider-independent foundation for bounded,
resumable local work. It is implemented in `crates/ponduin/src/task.rs` and
keeps runtime facts separate from model-generated plans.

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
