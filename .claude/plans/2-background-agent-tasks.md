---
issue: 2
title: Background agent tasks nested under their parent
branch: feat/background-agent-tasks
status: approved
approved_by: Ernesto
approved_at: 2026-10-20
autonomy: pr
source: request
mvp:
size: L
est_tokens: 1500000
est_lines: 700
shape: milestones
feature_branch: feat/background-agent-tasks
base: master
touches_api: true
---

# #2 Background agent tasks nested under their parent

Fork-only work (`erodriiguezz/herdr`), numbered locally because issues are disabled on the fork. #1 (progress checklist) is parked; this branches from `master`, not from #1.

## 1. Approval summary

**What:** In herdr's agent sidebar, each Claude pane row gets collapsible child rows, one per background Agent-tool task. Each child has a descriptive name and its own status (working ◐, done ✓, failed ×). Clicking a child focuses the parent's pane. A finished child stays until that parent pane is focused.

**Why:** Background tasks go unnoticed. Claude Code's own agents view lists them flat, mixed with every other session, under vague names ("Deliver 2", "Dev"). Herdr shows nothing, because its Claude hook drops every subagent event.

**How:** The Claude hook reports each background task to a new server-side `pane.report_agent_task` method. The TUI receives the list through a new optional companion message (`endpoint.agent-tasks.v1`, `agent_tasks` capability) and draws it as a tree under the parent row. Turned down: putting children into the frozen `ClientShellAgent.tokens` field, and a new `shell.snapshot.v2` codec.

**Blast radius**
- Touches: terminal server state (new in-memory task list), API schema (new methods), CLI, endpoint companion message, agent sidebar render and hit-testing, Claude integration hook and installer, docs.
- Files to touch:
  - `src/terminal/metadata.rs` (or a sibling `agent_tasks.rs`) — task list per terminal, limits, sanitising, clearing on session change or exit
  - `src/api/schema/panes.rs` — `PaneReportAgentTaskParams`, `PaneAgentTasksParams`, the result types
  - `src/api/schema.rs`, `src/api/schema/operations.rs` — register the methods
  - `src/app/api/panes.rs`, `src/app/api.rs` — method handlers; acknowledge finished tasks on pane focus
  - `src/cli/pane.rs` — `herdr pane tasks <pane>` (and a `report-task` leaf for manual testing)
  - `src/protocol/endpoint.rs` — `AGENT_TASKS_CAPABILITY`, `AGENT_TASKS_KIND`, `EndpointAgentTasks`, message builder
  - `src/server/client_shell.rs` — emit the companion message when the capability is negotiated and the list changes
  - `src/client/shell/agent_sidebar.rs` — child rows, `├─`/`└─` prefixes, `▾`/`▸` toggle, collapsed rollup status, child hits → parent `pane_id`
  - `src/client/shell/sidebar.rs` — reuse the existing tree prefix and group-toggle helpers (extract if needed, no behavior change)
  - `src/client/shell/tests/agents_worktrees_notifications.rs` — render and click tests
  - `src/integration/assets/claude/herdr-agent-state.sh`, `.ps1` — handle subagent events for background tasks, resolve the branch once, call the new method; bump `HERDR_INTEGRATION_VERSION` once from the latest release tag
  - `src/integration/claude_settings.rs`, `src/integration/mod.rs`, `src/integration/tests.rs` — install the subagent hooks idempotently, matching version constant
  - `src/integration/assets/herdr-agent-state.test.ts` — `.sh`/`.ps1` parity for the new requests
  - `tests/fixtures/endpoint-method-shapes-v1.json` — only if the test requires new methods listed there; existing entries are never changed
  - `docs/next/website/src/content/docs/socket-api.mdx`, `cli-reference.mdx`, `integrations.mdx` — document methods, CLI and behavior
  - `docs/context/product.md`, `docs/context/domain.md` — the Context rows below
- Also affected: users with the Claude integration get a re-install prompt; Claude runs one extra short hook process per subagent start/stop, never per keystroke or render.
- Left alone: `shell.snapshot.v1` and `ClientShellAgent`, frozen fixtures and digests, `PROTOCOL_VERSION`, detection manifests, saved session state, the other 18 integrations, Claude Code's own agents view.
- Rollback: revert and re-install the integration; nothing is persisted.

**Concerns:**
- Not verified yet: the `SubagentStart`/`SubagentStop` payloads in Claude Code 2.1.304. Specifically: whether they carry `agent_id`, `agent_type`, the description and a background flag, whether `SubagentStop` fires when a background task ends, and whether the task's cwd is its worktree. M1 captures them first. If background tasks can't be identified or their end isn't observable, the builder writes a Hand-off with the payloads instead of guessing.
- Children have only working/done/failed. A "needs input" child happens only if the capture shows a hook event for a blocked background task.
- #1's branch also edits the Claude hook and bumps the integration version. Whichever lands second rebases and keeps a single bump.
- The 2 macOS timeouts already failing on master (`detached_server_outlives_its_launcher_job`, `federated_launch_opens_local_directly_while_saved_ssh_is_unavailable`) are accepted as pre-existing, not as regressions.
- The sidebar render is a multiplicative path. Task rows are computed when a report arrives, not per frame, and are profiled at 1 and 15 panes.

**Security:**
- Data in and out: task label, agent type, branch and cwd from the local Claude hook → herdr server → API, CLI and sidebar.
- Who can access it: the local user only, through the existing same-user socket; no new listener.
- Stored: in memory per pane, until the parent is focused (finished tasks), the agent session changes, the agent exits, or herdr restarts.
- Abuse: a hostile local reporter floods tasks or injects terminal escapes. Controls: 20 tasks per pane (the oldest finished task is dropped first), labels capped at 200 chars, control characters stripped, stale `seq` dropped.

**Acceptance criteria**
- **Report and read** (AC-2.1) — `pane.report_agent_task` start then done for `t1` stores it; `pane.agent_tasks` and `herdr pane tasks <pane>` return label, type, branch and status; an older `seq` is ignored; an unknown pane returns the standard not-found error.
- **Name and limits** (AC-2.2) — label = description, else branch, else agent type; control characters stripped; labels capped at 200 chars; a 21st task drops the oldest finished one.
- **Lifecycle** (AC-2.3) — tasks clear when the agent session id changes or the agent exits, and don't survive a herdr restart.
- **Acknowledge on focus** (AC-2.4) — focusing the parent pane removes its done and failed tasks and keeps running ones.
- **Tree render** (AC-2.5) — a parent with 2 tasks renders `├─` and `└─` child rows with status icon and name; `▸` collapses them and the parent shows the highest child status; a parent with no tasks renders exactly as before.
- **Click** (AC-2.6) — clicking a child row sends `PaneFocus` for the parent pane.
- **Compatibility** (AC-2.7) — a client without the `agent_tasks` capability gets an unchanged v1 snapshot; frozen endpoint fixtures and digests are unchanged.
- **Hook install** (AC-2.8) — installing the Claude integration adds the subagent hooks exactly once (re-install doesn't duplicate them), the version is bumped once, and `.sh` and `.ps1` send the same requests.
- **Claude end to end** (AC-2.9) — in a throwaway herdr session, interactive Claude Code 2.1.304 with its normal tools (no `--tools` restriction) runs 2 background Agent tasks; 2 named child rows appear under that pane, turn ✓ when the tasks finish, and disappear after the pane is focused. CLI version and payloads are recorded.

**How it will be proven:** unit tests on terminal state with `AppState::test_new()`, API and CLI tests, sidebar render and hit tests, endpoint compatibility tests, installer and TS parity tests, `just check`, `just bench-render-scale` at 1 and 15 panes, and a live smoke via `herdr-throwaway-repro`. No browser: herdr is a TUI.

**Size** L · ~700 lines · ~1.5M tokens · 3 milestone PRs

**Out of scope:** Claude Code's own agents view; separate `claude` processes or cloud sessions as children; a task details popup; children for other integrations; the #1 progress checklist.

**Context:**
- product.md — new row "Background agent tasks" — Claude background Agent-tool tasks show as named child rows under their parent pane in the agent sidebar; finished ones stay until the parent is focused.
- domain.md — new row "Agent task" — a background unit of work started by an agent inside its pane, with no pane of its own; reported through `pane.report_agent_task`.

---

## 2. Grill

| # | Question | Answer |
| --- | --- | --- |
| 1 | Where is the screenshot ("Needs input / Working", `* dev`, `+ deliver #mvp-…`) from? | Claude Code's agents view. Herdr has no such list; rebuild the hierarchy in herdr. |
| 2 | How are the background sessions started? | Agent tool, run in background. |
| 3 | What does clicking a child do? | First "focus its pane"; after pushback (a background task has no pane), focus the parent's pane. |
| 4 | How long does a finished child stay? | Until the parent pane is focused. |
| 5 | Where does the child's name come from? | Agent description → worktree branch → agent type. |
| 6 | Relation to #1? | Parked; #2 takes precedence and branches from master. |

## 3. Design & architecture

### Approach
- **Runtime fact, server-owned.** The task list lives on the terminal beside the agent session (`src/terminal/metadata.rs` handles `pane.report_agent_session` today). It uses neutral names ("agent tasks"), per the runtime/client boundary rule. It's exposed via the JSON API and CLI first, so any integration can report tasks later.
- **Claude hook.** Today the hook exits on any payload with `agent_id` (`herdr-agent-state.sh:56-58`). The new branch handles `SubagentStart`/`SubagentStop` only for background tasks, plus `PreToolUse`/`PostToolUse` matched to `Agent` only if the capture shows the description or the background flag lives there. The hook runs `git -C <cwd> branch --show-current` once at task start, so the server does no filesystem I/O. Same request from `.sh` and `.ps1`.
- **To the client.** `ClientShellAgent` is inside the frozen `shell.snapshot.v1` and has no serde defaults, so it stays untouched. A new optional companion `endpoint.agent-tasks.v1` behind capability `agent_tasks` carries `{boot_id, revision, tasks: {pane_id: [task]}}`, mirroring `EndpointAgentCompletions` (`src/protocol/endpoint.rs:30-40,116`). It's sent when the list changes, never per frame. Old clients ignore it; a server without it simply shows no children.
- **Sidebar.** `agent_sidebar.rs` (`AgentRow`, `agent_row`, `render_agent_row`) gains child rows drawn with the worktree tree language already used in the spaces list (`sidebar.rs:711-722` prefixes, `:603-631` `▾`/`▸` toggle, `:633-661` rolled-up status). Each child row's hit maps to the parent `pane_id` and `Method::PaneFocus` (`agent_sidebar.rs:84`). Collapse state is TUI-only presentation state.
- **Retention.** On pane focus, the server drops that pane's finished tasks. That's shared runtime state, so every attached client sees the same thing.

### Alternatives considered
| Option | Why not |
| --- | --- |
| Encode children in `ClientShellAgent.tokens` | Hack on a frozen field; no structure, no click targets |
| New `shell.snapshot.v2` codec | Heavy codec negotiation for one optional list; the companion pattern already exists |
| Show children only in a popup | Doesn't solve "unnoticed"; the user wants them visible in the sidebar |
| Read Claude's agents view / session files from disk | Undocumented format, filesystem I/O, and no parent pane mapping; the hook already has `HERDR_PANE_ID` |

### Contract
| Method | Path / operation | Change | Breaking? |
| --- | --- | --- | --- |
| socket API | `pane.report_agent_task` | new | No |
| socket API | `pane.agent_tasks` | new | No |
| endpoint | `endpoint.agent-tasks.v1` + capability `agent_tasks` | new optional companion | No |
| CLI | `herdr pane tasks <pane>` (+ `report-task` leaf) | new | No |

### Impact set
| Consumer / area | Owner | How it's affected |
| --- | --- | --- |
| Old TUI clients / SSH / Cloud | herdr | Don't negotiate `agent_tasks`; unchanged |
| Claude integration users | users | Re-install prompt; new subagent hooks |
| Other integrations | herdr | Unaffected; may call the method later |

### Decisions
None as separate ADRs; the choices are recorded in this plan's alternatives table.

## 4. Plan

### PR split
| # | Branch | Scope | Est. lines |
| --- | --- | --- | --- |
| M1 | `feat/background-agent-tasks-p1-data` | Capture real Claude 2.1.304 subagent payloads (Hand-off if unusable), then server task state, `pane.report_agent_task` / `pane.agent_tasks`, acknowledgement on focus, CLI, API docs. AC-2.1, AC-2.2, AC-2.3, AC-2.4 | ~250 |
| M2 | `feat/background-agent-tasks-p2-logic` | Claude hook `.sh`/`.ps1`, installer, version bump, TS parity, integrations docs; live check through `herdr pane tasks`. AC-2.8, AC-2.9 (CLI part) | ~200 |
| M3 | `feat/background-agent-tasks-p3-ui` | `agent_tasks` companion message, sidebar tree, collapse, click, render bench, context rows. AC-2.5, AC-2.6, AC-2.7, AC-2.9 (sidebar part) | ~250 |

### Context
| File | Row | Change |
| --- | --- | --- |
| product.md | new: Background agent tasks | Claude background Agent-tool tasks show as named child rows under their parent pane; finished ones stay until the parent is focused. |
| domain.md | new: Agent task | Background unit of work started by an agent inside its pane, no pane of its own; reported via `pane.report_agent_task`. |

### Tests to add
- AC-2.1 — terminal state unit test: start/done/read, stale seq; API handler test for unknown pane; CLI output test.
- AC-2.2 — unit tests: label fallback, sanitising, 200-char cap, 20-task cap evicting the oldest finished.
- AC-2.3 — unit test with `AppState::test_new()`: session id change and agent exit clear tasks.
- AC-2.4 — unit test: `PaneFocus` drops finished tasks, keeps running.
- AC-2.5 — sidebar render tests in `src/client/shell/tests/agents_worktrees_notifications.rs`: tree prefixes, collapsed rollup, no-task parent unchanged.
- AC-2.6 — hit test: child click → `PaneFocus(parent)`.
- AC-2.7 — endpoint test: without the capability, the snapshot equals the v1 fixture and no companion is sent; frozen fixture tests stay green.
- AC-2.8 — installer tests in `src/integration/tests.rs` (idempotent hooks, version), parity cases in `herdr-agent-state.test.ts`.
- AC-2.9 — live smoke via `herdr-throwaway-repro`, recorded in the evidence.

### Screens
None (TUI). M3 captures `herdr` screen text of the sidebar with tasks for the review document.

## 5. Evidence

### Claude subagent payloads (M1 gate)
A live capture (`claude -p` in a scratch project with logging hooks) was blocked by the run's auto-mode permission classifier ("Create Unsafe Agents"). The shapes below come from Claude Code's hook and subagent docs (code.claude.com/docs/en/hooks, /sub-agents) and still need live confirmation in AC-2.9:
- `PostToolUse` for tool `Agent`: `tool_input` = `{prompt, description, subagent_type, model?}`. For a background launch, `tool_response` = `{status: "async_launched", agentId, description, prompt, outputFile, resolvedModel}`. Foreground: `status: "completed"`.
- `SubagentStart`: common fields plus `agent_id`, `agent_type`; no description, no background flag.
- `SubagentStop`: common fields plus `agent_id`, `agent_type`, `agent_transcript_path` (`…/subagents/agent-<agentId>.jsonl`), `last_assistant_message`, `stop_hook_active`. Fires "when a subagent completes", for foreground and background alike.
- Decision: the start report comes from `PostToolUse(Agent)` with `status == "async_launched"` (task_id = `agentId`, label = `description`, type = `subagent_type`). Done comes from `SubagentStop` (task_id = `agent_id`); the server ignores a finish for a task it doesn't know. There is no documented failure signal, so the hook never sends `failed`. No `branch`: at launch the `cwd` is the parent's.

### M1 — feat/background-agent-tasks-p1-data
- `just ci` green on `be4db568`: fmt, clippy `-D warnings`, 3919 nextest tests passed (14 skipped), maintenance tests; `just docs-contract-test` green. `windows-lint` can't run locally (no Windows SDK); CI's `windows-2022` job covers it.
- The QA script's AC scanner only reads JS/TS tests; AC-2.1–2.4 were mapped by hand to `ac_2_*` Rust tests (see `docs/reviews/2-m1.md`). App and browser stages don't apply (TUI).
- Reviewer: approve with fixes. The 2 High process findings were answered as above; Medium `source` and Low bidi stripping go to M2; Medium focus redraw goes to M3.
- Size: 954 counted lines, including about 400 lines of inline Rust tests and 174 lines of generated schema; about 250 lines of non-test code.

### M2 — feat/background-agent-tasks-p2-logic
- Server: tasks are owned by `source`; finishes are final (a finish applies whatever its seq, because hook processes stamp seq at process start, not at event time); a finish that arrives before its start is remembered (FIFO, max 20) and applied to that start; bidi and zero-width format characters are stripped.
- Hook `task` action (`.sh` over the socket, `.ps1` through `herdr pane report-task` with CommandLineToArgvW quoting for Windows PowerShell 5.1); installer adds `PostToolUse` (`^(Agent|Task)$`) and `SubagentStop` once and removes them on uninstall; integration version 11 → 12.
- `just ci` green on `ebc044c8` (3931 passed, 14 skipped); `just docs-contract-test` green. The `.ps1` test is `#[cfg(windows)]` and was not run (no Windows or pwsh here; this fork runs no CI).
- Scripted smoke against a debug server (documented payloads piped into the `.sh` hook; no Claude launched): background launch → 1 working task "Review the diff"; foreground launch → unchanged; SubagentStop → done; focus away and back → empty.
- Reviewer round 1: request changes. 2 High fixed (finishes final; `.ps1` argument quoting), Medium explicit uninstall flag fixed, Lows fixed. Remaining High (no live capture) recorded as a deferral below.
- Deviation: the parity tests live in Rust (`src/integration/tests.rs`, shared `claude_task_cases()` table), not in `herdr-agent-state.test.ts`, which tests only the omp plugin.
- Size: about 1060 added lines, roughly half of them tests, against ~200 planned.
- **AC-2.9 deferred to a human:** a live Claude Code 2.1.304 run with real background Agent tasks could not be done in this run (the permission classifier blocks spawning agents). See Check yourself on the final PR.

### M3 — feat/background-agent-tasks-p3-ui
- Companion `endpoint.agent-tasks.v1` behind capability `agent_tasks`. The client opts in with a serde-default `EndpointClientHello.agent_tasks` flag (the `surface_scroll` precedent). The status enum has a `#[serde(other)] Unknown` fallback, which clients skip. Nothing frozen was touched.
- The projection is built at most once per render pass, only when the task revision or the snapshot's pane set changes, and is sent only on change. A capable client gets a baseline companion on connect. Acknowledge-on-focus requests a render.
- Sidebar: `├─`/`└─` child rows (◐ ✓ ×), `▾`/`▸` toggle, collapse state TUI-only and pruned for panes that leave. Below the toggle width, children always render. A child click focuses the parent, in single- and multi-endpoint sidebars.
- Tests: AC-2.5 (tree, rollup, no-task parent unchanged, unknown status, narrow width), AC-2.6 (child click, toggle, two endpoints, pruning), AC-2.7 (opt-in, sent only on change, one build per pass, no rebuild on title change, baseline for a late joiner; deterministic barrier drain). New AC tests passed 8 of 8 repeated runs.
- `just ci` green on `5e2e417a` (3944 passed, 14 skipped); `just docs-contract-test` green.
- `just bench-render-scale`, base vs M3 run under the same conditions. Client-shell composition median at 15 panes across 6 scenarios: base 165–195 µs, M3 179–194 µs; at 1 pane both 159–172 µs. No measurable scaling change; the fixtures hold no tasks, so this measures the always-on path. `native_file_render_scale_profile` fails on both (pre-existing macOS CoW-retention issue).
- Reviewer round 1: request changes. The 3 High were process items (bench, evidence, context), answered here. Mediums fixed: projection cost, per-frame label clones, pairing robustness, test determinism, multi-endpoint tests. Lows fixed.
- **Deviation, AC-2.5:** when collapsed, the parent keeps its own agent status icon and the rolled-up child status (failed > working > done) is drawn beside `▸`. Replacing the icon would hide the agent's own blocked state.
- **Deviation, Context:** this repo has no `docs/context/`, and `/docs/*` is gitignored, so the product.md and domain.md rows were not added. They are covered by `integrations.mdx` and `socket-api.mdx`.
- Sidebar capture (test render, 26 columns, 2 tasks):
  ```
  expanded                    collapsed
   ● client-shell         ▾│   ● client-shell       ◐ ▸│
     claude                │     claude                │
   ├─ ◐ Review the diff    │
   └─ ✓ Write the tests    │
  ```

## 6. Retro
- What the plan got wrong: assumed the run could launch Claude itself to capture payloads and run the live smoke. Auto mode blocks spawning nested agents.
- What slowed the work: the qa tooling assumes a web app: the AC scanner reads only JS/TS, UI audit needs a URL, and the size script counts inline Rust tests.
- Rule, skill or fixture that would have prevented it: an ac-coverage pattern for Rust test names (`ac_<issue>_<n>`), plus a `project.json` flag to skip the web stages for a TUI. Proposed, not applied.

## 7. Hand-off
