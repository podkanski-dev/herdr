# Custom agent command aliases (recognition + resume)

Date: 2026-06-30
Status: Approved design, pending implementation plan
Branch: `claude-aliases-support`

## Problem

Herdr recognizes coding agents by foreground process name. `identify_agent()`
(`src/detect/mod.rs`) matches only literal binary names (`claude`,
`claude-code`, …). Users who launch Claude through per-account wrapper commands —
`claude-xebia`, `claude-mtv`, `claude-personal`, each pointing the `claude`
binary at a different `CLAUDE_CONFIG_DIR`/account — run a process whose visible
name is `claude-xebia`. `identify_agent("claude-xebia")` returns `None`, so:

1. The pane is never treated as a Claude agent, so manifest state detection
   (working/idle/blocked), the agent UI, and the resume affordance never engage.
   This is the user-visible symptom: **"tab not recognized as Claude."**
2. Even when a session id is captured via the hook, resume is hardcoded to argv
   `["claude", "--resume", <id>]` in `src/agent_resume.rs`, which would relaunch
   the wrong account.

The user launches these commands manually in a shell (not via herdr), and the
herdr Claude hook may not be installed in every account's config dir.

## Goal

Let a user register custom command names as a known agent so herdr:

- recognizes the alias-launched pane as that agent (engaging detection + UI), and
- resumes the native session using the *same* command the session was launched
  with (`claude-xebia --resume <id>`), hitting the correct account.

Plus an installer path to place the hook into each account's config dir.

## Non-goals

- No per-repository scoping. Recognition is process-name based and global; a
  single registry covers every repo.
- No prefix/heuristic auto-matching (`claude-*`). Recognition is explicit config
  only, to avoid false positives.
- No change to per-agent resume *argument* shapes; only argv[0] becomes
  configurable/captured.
- Shell aliases are out of scope. Custom commands must be real executables on
  `PATH` (wrapper script or symlink that selects the account), because both
  detection (process name) and resume (direct exec of argv) require a real
  process image.

## Approach

Chosen approach: **global command registry + auto-captured resume command**,
plus installer support for multiple config dirs.

### 1. Config surface

New agent-keyed table in herdr config (the `Config` struct in
`src/config/model.rs`, alongside `session`, `terminal`, etc.):

```toml
[agents.claude]
commands    = ["claude-xebia", "claude-mtv", "claude-personal"]
config_dirs = ["~/.claude-xebia", "~/.claude-mtv", "~/.claude-personal"]
```

- Keyed by built-in agent id, parsed through the existing
  `crate::detect::parse_agent_label`. An unknown agent id is a config warning and
  that entry is skipped (does not abort config load).
- `commands`: extra process names that mean this agent. Normalized the same way
  `identify_agent` normalizes input (lowercase, basename via the existing
  `normalized_agent_lookup_name` path) and stored as `command -> Agent`. A name
  that collides with a built-in is ignored (built-ins win). Empty/blank entries
  are dropped.
- `config_dirs`: account config dirs the integration installer should also target
  for this agent. Tilde/`$HOME` expanded and deduped. Independent from
  `commands` — herdr does not infer one from the other.
- Both keys are optional; an `[agents.<id>]` table may set either or both.

### 2. Recognition

- Add a process-global, reloadable registry in the detect layer, mirroring the
  existing manifest cache (`static MANIFEST_CACHE: OnceLock<RwLock<…>>` +
  `reload_manifests()` in `src/detect/manifest.rs`):

  ```rust
  static AGENT_COMMAND_REGISTRY: OnceLock<RwLock<HashMap<String, Agent>>>;
  ```

  Populated from config at server start; refreshed on config reload. Default
  (uninitialized / empty) preserves current behavior exactly.
- `identify_agent()` consults built-ins first, then the registry. No call-site
  signature changes — `identify_agent_in_job`, the pane probes in `src/pane.rs`
  (lines ~440/482/509), and the Windows path (`src/platform/windows.rs:93`) keep
  working unchanged.
- Result: a `claude-xebia` pane resolves to `Agent::Claude`, so the manifest
  state pipeline, agent UI, and resume affordance engage.

### 3. Resume with the captured command

- The session id already arrives via the hook (`source: herdr:claude`,
  `pane.report_agent_session`). The missing piece is argv[0].
- When herdr records the agent session for a pane, also capture the **detected
  foreground command name** (e.g. `claude-xebia`) from the same probe that
  identified the agent. `launch_argv` cannot be used: it is only populated when
  herdr itself launched the command (`SplitCommand::Argv`), not when the user
  types the alias in a shell.
- Persist it: add an optional `command: Option<String>` to
  `PaneAgentSessionSnapshot` (`src/persist/snapshot.rs`), `#[serde(default,
  skip_serializing_if = "Option::is_none")]` so existing snapshots remain
  compatible.
- Thread it into `agent_resume::plan()` as an optional argv[0] override. When
  present, plan emits `["claude-xebia", "--resume", <id>]`; when absent, it falls
  back to today's hardcoded binary name. The per-agent argument shape
  (`--resume <id>`) is unchanged. `plan()` continues to gate on
  `is_official_agent_source`; the override only replaces argv[0].
- The override must be a basename (no path separators) so it resolves via `PATH`
  like the built-ins; reject/ignore values containing path separators or control
  characters (reuse the existing `valid_session_id`-style guard style).

### 4. Installer support for multiple config dirs

- Refactor `install_claude()` (`src/integration/targets.rs`) into
  `install_claude_into(dir: &Path)`. `install_claude()` installs into the default
  `claude_dir()` **plus** each configured `config_dirs` entry, returning the set
  of paths touched. `uninstall_claude()` mirrors this.
- The install/uninstall action layer (`src/integration/actions.rs`) reports every
  path it installed to / removed from.
- Add a repeatable one-off CLI flag handled in `src/cli/integration.rs`:
  `herdr integration install claude --config-dir <path>` (and the matching
  uninstall flag) for installing without editing config.
- Per-dir robustness: a missing/var-unset config dir is a per-dir warning, not a
  hard failure, so one bad path does not abort the others. Dirs are deduped
  (including against the default).
- Bump the relevant `*_INTEGRATION_VERSION` only if the hook asset content
  changes; installing the same asset into more dirs does not by itself require a
  version bump.

### 5. Runtime/client boundary

Agent identity, detection, and resume are server/runtime concerns. The registry
lives in the detect/runtime layer with neutral naming; no TUI-only coupling is
introduced. Config is server config. Consistent with the runtime/client
guardrail in `CLAUDE.md`.

## Testing

- **Config parse**: valid agent ids accepted; unknown id warned and skipped;
  `commands` normalization; built-in collision ignored; `config_dirs`
  expansion/dedup; empty entries dropped.
- **Recognition**: `identify_agent` with a populated registry returns the mapped
  agent; empty registry preserves all current behavior (existing
  `identify_agent` tests stay green).
- **Resume planning**: `plan()` with a command override emits the alias as
  argv[0] for each supported source/agent; without override, output is unchanged
  (existing `planner_allows_supported_agents` stays green); override with path
  separators/control chars is rejected.
- **Persistence**: snapshot roundtrip carries `command` through save/restore; old
  snapshots without the field still deserialize.
- **Installer**: `install_claude_into` writes hook + settings into a given dir;
  multi-dir install touches default + configured dirs and reports them; missing
  dir produces a warning, not an abort; `--config-dir` flag parsed and repeatable.

These are unit tests next to the code. No full-screen agent fixture suites are
added (per `CLAUDE.md` agent-detection guidance).

## Risk classification

Touches detection identity authority, persisted snapshot state, and the resume
planner — refactor-risk surfaces under `CLAUDE.md`. Protected behavior:
identify_agent for built-ins, existing resume argv for non-aliased agents, and
snapshot backward compatibility. Characterization is covered by the existing
`identify_agent`, `planner_allows_supported_agents`, and snapshot roundtrip
tests, which must remain green; new tests extend rather than replace them.

## Open setup note (user-facing docs)

Resume data only exists when the Claude hook is present in each account's
`CLAUDE_CONFIG_DIR/settings.json`. With the installer change, users run
`herdr integration install claude` after listing `config_dirs`, or use
`--config-dir` per account. Document this in `docs/next` when the change lands.
