---
title: Production Roadmap
sidebar_position: 5
---

# Ponduin production roadmap

This is the operational record for making Ponduin a dependable local coding
agent. Statuses in this document describe observed behaviour, not intended
architecture. Evidence is linked where it is available; unknown measurements
remain unknown.

## Vision

Ponduin should let a developer hand it a local repository and a bounded task,
then reliably explore the project, make the smallest correct change, validate
the result, recover from failures, and explain the evidence. It must work with
local models and local resources while keeping workspace, command, and
permission boundaries intact.

## Current state

The Rust coding core contains repository inspection, bounded search, versioned
file editing, patching, validation, Git operations, workflow evidence,
checkpointing, recovery information, and permission-aware tool dispatch. The
desktop application exposes the normal agent path.

Those components are not yet a production claim. The first live desktop repair
run with Ollama `qwen2.5-coder:7b` did not reach a single tool call: two
  unstructured routing responses caused the backend to reject the task before
  the model received coding tools. This was a runtime routing failure, not evidence
  that the model cannot repair the fixture. The bounded router now falls back to
  the existing bounded coding tools after inconclusive attempts. Existing workspace,
  command, permission, and tool policies still apply. A second observed issue is
  that `qwen2.5-coder:7b` can serialize a tool request as JSON assistant text.
  The Ollama transport now recognizes the strict `{ "name", "arguments" }` form.

## Capability matrix

Reifegrad: 0 absent, 1 design/stub, 2 controlled tests, 3 simple live task, 4
reliable across live tasks, 5 production-ready. “Not yet” means no recorded
live evaluation, not that the source implementation is absent.

| Area | Status | Reifegrad | Live E2E | Known issue |
| --- | --- | ---: | --- | --- |
| Repository exploration | implemented | 2 | not yet | no completed live task recorded |
| Code search and reads | implemented | 2 | not yet | no completed live task recorded |
| File editing | implemented | 2 | not yet | no completed live task recorded |
| Multi-file changes | implemented | 2 | not yet | no completed live task recorded |
| Build and test execution | implemented | 2 | blocked before use | router prevented tool exposure |
| Failure diagnosis | implemented | 2 | blocked before use | router prevented tool exposure |
| Repair loop | implemented | 2 | blocked before use | router prevented tool exposure |
| Git operations | implemented | 2 | not yet | no live evaluation recorded |
| Planning and replanning | implemented | 2 | not yet | no live evaluation recorded |
| Checkpoint and resume | implemented | 2 | not yet | no live evaluation recorded |
| Context and evidence | implemented | 2 | not yet | no live evaluation recorded |
| Local-model reliability | measured failure | 0 | 0/3 completed task successes | repair is not yet externally verified |
| Desktop coding workflow | partial | 1 | tool execution observed | model repeats tests and has not repaired fixture |

## Known defects

### P0 — Inconclusive coding routing aborts a task

- **Symptom:** The desktop reports `The selected model did not return exactly
  one valid semantic coding-routing decision after 2 bounded attempts.`
- **Reproduction:** In the desktop application, with Ollama
  `qwen2.5-coder:7b`, ask the agent to run the failing Python fixture test and
  repair `normalizer.py`. The prompt and fixture are recorded in the evaluation
  result.
- **Cause:** `Agent::decide_coding_tool_exposure` treated two invalid or timed
  out routing-only responses as a terminal error, although the normal main turn
  can safely receive the existing bounded coding tool set.
- **Impact:** A coding task can fail before repository exploration, shell,
  editing, validation, evidence, or recovery execute.
- **Priority:** P0.
- **Status:** Fixed and regression-tested. The repeat desktop run reached
  coding tools instead of emitting the terminal router error.

### P0 — Ollama text-form tool requests were not transported

- **Symptom:** `qwen2.5-coder:7b` emitted a syntactically valid
  `{"name":"coding__workflow_start","arguments":...}` object as visible
  assistant text, so no tool ran.
- **Reproduction:** Repeat the Python fixture task after the routing fix with
  the default Ollama configuration.
- **Cause:** The Ollama response adapter supported native and XML tool calls,
  but not this text-form JSON representation.
- **Impact:** A model could select a tool correctly but the agent would loop on
  its visible JSON instead of executing it.
- **Priority:** P0.
- **Status:** Fixed with a strict JSON fallback and deterministic parser test.
  A GUI repeat showed a real `Workflow Start` tool call without the optional
  tool interpreter. Full fixture repair remains unverified.

### P1 — Live reliability coverage is too narrow

- **Symptom:** No controlled live-provider task has yet verified an autonomous
  repair success.
- **Cause:** Historical results focus on deterministic coding-core tests; the
  first targeted desktop evaluation exposed the P0 router blocker.
- **Impact:** Production readiness and model reliability cannot be claimed.
- **Priority:** P1.
- **Status:** First permanent Python repair fixture added; expand only after
  the P0 repeat run is measured.

### P1 — Full local validation is limited by the synchronized workspace

- **Symptom:** broad formatting and frontend checks can remain blocked in file
  reads while iCloud-synchronised duplicate files exist.
- **Cause:** workspace I/O state, not a source failure established by a test.
- **Impact:** full local gates may be unavailable; targeted checks must be
  reported separately.
- **Priority:** P1.
- **Status:** do not delete or alter user files; validate in the widest safe
  scope available and record any exact blockage.

## Prioritized roadmap

| Priority | Problem and next change | Success criteria | Test strategy | Status |
| --- | --- | --- | --- | --- |
| P0 | Stop repeated validation before new evidence | an unchanged failed test forces repository/file inspection or an explicit block | controlled Python repair fixture with tool-call trace | pending |
| P1 | Establish small cross-language live fixture suite | objective results for exploration, repair, failed-test, multi-file, and feature tasks | controlled Python, Rust, and TypeScript fixtures, multiple runs where useful | pending |
| P1 | Record tool-flow traces and outcome metrics | report captures tool use, runtime, verification, and failure class without invented values | machine-readable result schema plus external checks | pending |
| P1 | Resolve/reproduce broad-gate I/O limitations | full checks run in a stable checkout or record a concrete source failure | isolated worktree or CI | pending |
| P2 | Measure recovery after failed repair | failed strategy causes a materially different next action or an explicit block | adversarial fixture with a required retry | pending |
| P2 | Measure resume and checkpoint fidelity | resumed task retains only valid evidence and reaches objective result | interrupted controlled fixture | pending |

## Completed work

| Date | Work | Commit | Result | Remaining limitation |
| --- | --- | --- | --- | --- |
| 2026-08-14 | Causal repair and capability-feedback evidence | `ea2efc21c2c1dd0b7346e051af547d735d0be287` | 197 focused coding tests passed; documented in phase 2 results | no live autonomous repair succeeded |
| 2026-08-17 | Initial real desktop repair baseline | current `dev` before router fallback | Python fixture test was red; desktop task failed before any tool call with invalid routing output | outcome is 0/1, so all productive claims remain withheld |
| 2026-08-17 | Bounded router fallback and Ollama JSON tool fallback | pending | focused Rust tests pass; GUI reached real coding tools with and without the optional interpreter | fixture is still red; no autonomous repair success yet |

## Current top priorities

1. Prevent repeated unchanged validation from consuming the repair budget.
2. Achieve the first externally verified repair in the existing Python fixture.
3. Classify the next observed live failure rather than guessing at model limits.
4. Add only the next fixture category justified by the measured result.
5. Capture objective verification, tool activity, and runtime for every live run.
6. Run broad Rust and desktop gates in a checkout where synchronized I/O does
   not prevent a result.

## Evaluation evidence

The current machine-readable record is
`evals/agent/production-readiness-results.json`. Historic deterministic results
are in `evals/coding-agent/reliability-phase-2-results.json`. A live task counts
as successful only when the fixture's external verification passes, not when
the model says it completed the work.
