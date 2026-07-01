# Custom Agent Command Aliases Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users register custom command names (e.g. `claude-xebia`) as a known agent so herdr recognizes those panes as that agent, and resume native sessions with the same command, plus install the integration hook into multiple account config dirs.

**Architecture:** A process-global, config-driven command registry in the detect layer (mirroring the existing manifest cache) makes `identify_agent` recognize custom command names. The actual launched command is captured from process detection, stored on the terminal, persisted with the agent session, and used as argv[0] when planning resume. The Claude installer is refactored to install into the default config dir plus any configured/`--config-dir` account dirs.

**Tech Stack:** Rust, serde/TOML config, `OnceLock<RwLock<_>>` global, tokio mpsc events, `just` test recipes.

## Global Constraints

- No `unwrap()` in production code; use `tracing` for logging; `#[allow]` only with a justifying comment. (`CLAUDE.md`)
- Platform-specific code must be compile-gated under `src/platform/`. This feature is cross-platform; no `#[cfg(target_os)]` is expected in core modules.
- Detection is decoupled: the registry must not reach into parser/viewport state; `identify_agent` stays a pure-ish lookup over its input plus the global registry.
- Runtime/client boundary: agent identity, detection, resume, and config are server/runtime concerns. Use neutral naming; do not add TUI-only coupling.
- `AppState`/`TerminalState` remain pure data, testable via `AppState::test_new()` / `TerminalState::new(...)` without PTYs. No syscalls in `AppState`.
- Resume argv[0] override must be a basename: reject values containing path separators (`/`, `\`) or control characters.
- Snapshot additions must be backward compatible: `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- Use `just check` before committing. When a commit relates to a GitHub issue, add a `refs #<n>` body line. Lowercase conventional commit subjects, no emojis, no AI co-author lines. Propose the commit message and get alignment before committing (per `CLAUDE.md`).
- Do not edit root `README.md`/`CHANGELOG.md` or stable `website/` docs; user-facing docs go under `docs/next/`.

**Testing commands:**
- Single test: `cargo nextest run <test_name>`
- Module: `cargo nextest run --lib <module>`
- Full gate: `just check`

---

## File Structure

- `src/detect/agent_commands.rs` — **new**: the global command registry (`set_agent_command_registry`, `registered_agent`) + unit tests.
- `src/detect/mod.rs` — modify: `identify_agent` consults the registry; export the new module.
- `src/config/model.rs` — modify: add `AgentsConfig` / `AgentEntryConfig`, add `agents` to `Config`, `command_entries()` and `config_dirs_for()` helpers.
- `src/app/mod.rs` — modify: `apply_live_config` populates the registry and surfaces unknown-agent diagnostics.
- `src/agent_resume.rs` — modify: `PersistedAgentSession.command`, `plan()` argv[0] override, basename validation.
- `src/terminal/state.rs` — modify: `detected_command` field + setter; populate `PersistedAgentSession.command` in `set_agent_session_ref_for_session_start`.
- `src/events.rs` — modify: add `AgentCommandDetected` event.
- `src/pane.rs` — modify: emit `AgentCommandDetected` from the agent-change detection site.
- `src/app/actions.rs` — modify: handle `AgentCommandDetected`.
- `src/persist/snapshot.rs` — modify: `PaneAgentSessionSnapshot.command` capture.
- `src/persist/restore.rs` — modify: thread `command` into the restore resume plan.
- `src/integration/targets.rs` — modify: `install_claude_into(dir)` / multi-dir `install_claude`; uninstall mirror.
- `src/integration/types.rs` — modify: install/uninstall result types carry the per-dir paths.
- `src/integration/actions.rs` — modify: report every installed/removed path.
- `src/cli/integration.rs` — modify: parse repeatable `--config-dir <path>`.
- `docs/next/website/src/content/docs/...` — **new/modify**: document the config and per-account install.

---

## Phase 1 — Recognition (independently shippable)

### Task 1: Agent command registry in the detect layer

**Files:**
- Create: `src/detect/agent_commands.rs`
- Modify: `src/detect/mod.rs` (add `mod agent_commands;` + re-export; route `identify_agent` fallthrough)
- Test: in `src/detect/agent_commands.rs` (`#[cfg(test)]`) and `src/detect/mod.rs` tests

**Interfaces:**
- Produces:
  - `pub fn set_agent_command_registry(entries: impl IntoIterator<Item = (String, crate::detect::Agent)>)`
  - `pub(crate) fn registered_agent(normalized_name: &str) -> Option<crate::detect::Agent>`
  - `identify_agent` now returns registry matches for unknown built-in names.
- Consumes: `normalized_agent_lookup_name` (private in `mod.rs`) — expose it to the submodule via `pub(super)`.

- [ ] **Step 1: Write the failing test**

In `src/detect/mod.rs` tests module, add:

```rust
#[test]
fn identify_agent_uses_registered_commands() {
    super::agent_commands::set_agent_command_registry([
        ("claude-xebia".to_string(), Agent::Claude),
        ("Claude-MTV".to_string(), Agent::Claude),
    ]);
    assert_eq!(identify_agent("claude-xebia"), Some(Agent::Claude));
    // normalization: case + .exe suffix
    assert_eq!(identify_agent("claude-mtv.exe"), Some(Agent::Claude));
    // built-ins still win and unknown still None after clearing
    super::agent_commands::set_agent_command_registry([]);
    assert_eq!(identify_agent("claude"), Some(Agent::Claude));
    assert_eq!(identify_agent("claude-xebia"), None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run identify_agent_uses_registered_commands`
Expected: FAIL — `agent_commands` module / `set_agent_command_registry` not found.

- [ ] **Step 3: Implement the registry module**

Create `src/detect/agent_commands.rs`:

```rust
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use super::{normalized_agent_lookup_name, Agent};

static AGENT_COMMAND_REGISTRY: OnceLock<RwLock<HashMap<String, Agent>>> = OnceLock::new();

fn registry() -> &'static RwLock<HashMap<String, Agent>> {
    AGENT_COMMAND_REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Replace the custom command registry with `entries`. Command names are
/// normalized the same way `identify_agent` normalizes its input; blank names
/// are dropped. Built-in agent names always take precedence in `identify_agent`,
/// so a registry entry that collides with a built-in name has no effect.
pub fn set_agent_command_registry(entries: impl IntoIterator<Item = (String, Agent)>) {
    let map: HashMap<String, Agent> = entries
        .into_iter()
        .map(|(name, agent)| (normalized_agent_lookup_name(&name), agent))
        .filter(|(name, _)| !name.is_empty())
        .collect();
    match registry().write() {
        Ok(mut guard) => *guard = map,
        Err(poisoned) => *poisoned.into_inner() = map,
    }
}

/// Look up a registered agent by an already-normalized command name.
pub(crate) fn registered_agent(normalized_name: &str) -> Option<Agent> {
    let guard = registry().read().ok()?;
    guard.get(normalized_name).copied()
}
```

In `src/detect/mod.rs`:
- Add near the other `mod` declarations: `mod agent_commands;`
- Re-export: `pub use agent_commands::set_agent_command_registry;`
- Change the visibility of `normalized_agent_lookup_name` from `fn` to `pub(super) fn`.
- In `identify_agent`, change the final arm `_ => None,` to:

```rust
        _ => agent_commands::registered_agent(&name),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run identify_agent_uses_registered_commands`
Expected: PASS

Also run the existing detect tests to confirm no regression:
Run: `cargo nextest run --lib detect`
Expected: PASS (existing `identify_agent` built-in tests unchanged).

- [ ] **Step 5: Commit**

```bash
git add src/detect/agent_commands.rs src/detect/mod.rs
git commit -m "feat: add custom agent command registry"
```

---

### Task 2: Config `[agents.*]` parsing

**Files:**
- Modify: `src/config/model.rs`
- Test: `src/config/model.rs` (`#[cfg(test)]`) or existing config tests module

**Interfaces:**
- Produces:
  - `pub struct AgentsConfig { entries: BTreeMap<String, AgentEntryConfig> }` with `#[serde(flatten)]`.
  - `pub struct AgentEntryConfig { pub commands: Vec<String>, pub config_dirs: Vec<String> }`.
  - `AgentsConfig::command_entries(&self) -> (Vec<(String, crate::detect::Agent)>, Vec<String>)` — resolved `(command, agent)` pairs plus warnings for unknown agent ids.
  - `AgentsConfig::config_dirs_for(&self, agent_id: &str) -> Vec<String>` — raw config-dir strings for one agent id.
  - `Config.agents: AgentsConfig`.
- Consumes: `crate::detect::parse_agent_label` (already `pub`).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn agents_config_resolves_commands_and_warns_on_unknown() {
    let toml = r#"
[agents.claude]
commands = ["claude-xebia", "  ", "claude-mtv"]
config_dirs = ["~/.claude-xebia"]

[agents.notanagent]
commands = ["whatever"]
"#;
    let cfg: Config = toml::from_str(toml).expect("parse");
    let (entries, warnings) = cfg.agents.command_entries();
    assert!(entries.contains(&("claude-xebia".to_string(), crate::detect::Agent::Claude)));
    assert!(entries.contains(&("claude-mtv".to_string(), crate::detect::Agent::Claude)));
    // blank command dropped
    assert_eq!(entries.iter().filter(|(c, _)| c.trim().is_empty()).count(), 0);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("notanagent"));
    assert_eq!(cfg.agents.config_dirs_for("claude"), vec!["~/.claude-xebia".to_string()]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run agents_config_resolves_commands_and_warns_on_unknown`
Expected: FAIL — `Config` has no `agents` field.

- [ ] **Step 3: Implement config types**

In `src/config/model.rs`:

```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AgentsConfig {
    #[serde(flatten)]
    pub entries: std::collections::BTreeMap<String, AgentEntryConfig>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AgentEntryConfig {
    /// Extra foreground command names that mean this agent.
    pub commands: Vec<String>,
    /// Account config dirs the integration installer should also target.
    pub config_dirs: Vec<String>,
}

impl AgentsConfig {
    /// Resolved `(command, agent)` pairs for the command registry, plus a
    /// warning per `[agents.<id>]` whose id is not a known agent.
    pub fn command_entries(&self) -> (Vec<(String, crate::detect::Agent)>, Vec<String>) {
        let mut entries = Vec::new();
        let mut warnings = Vec::new();
        for (agent_id, entry) in &self.entries {
            let Some(agent) = crate::detect::parse_agent_label(agent_id) else {
                warnings.push(format!("[agents.{agent_id}]: unknown agent id, ignoring"));
                continue;
            };
            for command in &entry.commands {
                if command.trim().is_empty() {
                    continue;
                }
                entries.push((command.trim().to_string(), agent));
            }
        }
        (entries, warnings)
    }

    /// Raw (unexpanded) config-dir strings configured for `agent_id`.
    pub fn config_dirs_for(&self, agent_id: &str) -> Vec<String> {
        self.entries
            .get(agent_id)
            .map(|entry| entry.config_dirs.clone())
            .unwrap_or_default()
    }
}
```

Add the field to `Config` (after `remote`):

```rust
    pub remote: RemoteConfig,
    pub agents: AgentsConfig,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run agents_config_resolves_commands_and_warns_on_unknown`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/config/model.rs
git commit -m "feat: parse [agents] command alias config"
```

---

### Task 3: Populate the registry when config is applied

**Files:**
- Modify: `src/app/mod.rs` (`apply_live_config`)
- Test: `src/app/mod.rs` tests (uses `AppState`/`App` test helpers) or an integration-style test that calls `apply_live_config`

**Interfaces:**
- Consumes: `config.agents.command_entries()` (Task 2), `crate::detect::set_agent_command_registry` (Task 1).
- Produces: registry reflects config after every `apply_live_config`; unknown-agent warnings appended to `diagnostics`.

- [ ] **Step 1: Write the failing test**

Add to `src/app/mod.rs` tests:

```rust
#[test]
fn applying_config_populates_agent_command_registry() {
    let mut config = crate::config::Config::default();
    config.agents.entries.insert(
        "claude".to_string(),
        crate::config::model::AgentEntryConfig {
            commands: vec!["claude-xebia".to_string()],
            config_dirs: vec![],
        },
    );
    let mut app = App::test_new();
    let _ = app.apply_live_config(&config, &[], &[], false);
    assert_eq!(
        crate::detect::identify_agent("claude-xebia"),
        Some(crate::detect::Agent::Claude)
    );
    // reset global for other tests
    crate::detect::set_agent_command_registry([]);
}
```

(If `App::test_new()` is not the exact constructor, use the existing app test constructor in that module — match the surrounding tests. `AgentEntryConfig` may need `pub use` from `crate::config`; re-export it next to `Config` in `src/config/mod.rs` if not already public.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run applying_config_populates_agent_command_registry`
Expected: FAIL — `identify_agent("claude-xebia")` returns `None` (registry never populated).

- [ ] **Step 3: Wire registry population in `apply_live_config`**

In `src/app/mod.rs`, inside `apply_live_config`, after `let mut diagnostics = load_diagnostics.to_vec();` and before the `keys` block, add:

```rust
        if !invalid_section("agents") {
            let (agent_commands, agent_warnings) = config.agents.command_entries();
            crate::detect::set_agent_command_registry(agent_commands);
            diagnostics.extend(agent_warnings);
        }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run applying_config_populates_agent_command_registry`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/app/mod.rs src/config/mod.rs
git commit -m "feat: apply agent command aliases from config"
```

---

## Phase 2 — Resume with the alias

### Task 4: `PersistedAgentSession.command` + `plan()` argv[0] override

**Files:**
- Modify: `src/agent_resume.rs`
- Test: `src/agent_resume.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces:
  - `PersistedAgentSession` gains `pub command: Option<String>`.
  - New `fn valid_resume_command(value: &str) -> bool` (basename, no control chars, non-empty, bounded).
  - `plan` signature becomes `pub fn plan(source: &str, agent: &str, session_ref: &AgentSessionRef, command: Option<&str>) -> Option<AgentResumePlan>`; when `command` is `Some(valid)` it replaces argv[0], else today's hardcoded binary is used.
- Consumes: nothing new.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn plan_uses_command_override_for_argv0() {
    let session = AgentSessionRef::id("xebia-session").unwrap();
    let plan = plan("herdr:claude", "claude", &session, Some("claude-xebia")).unwrap();
    assert_eq!(plan.argv, vec!["claude-xebia", "--resume", "xebia-session"]);

    // invalid override (path separator) falls back to the default binary
    let plan = plan("herdr:claude", "claude", &session, Some("/usr/bin/claude-xebia")).unwrap();
    assert_eq!(plan.argv, vec!["claude", "--resume", "xebia-session"]);

    // no override preserves existing behavior
    let plan = plan("herdr:claude", "claude", &session, None).unwrap();
    assert_eq!(plan.argv, vec!["claude", "--resume", "xebia-session"]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run plan_uses_command_override_for_argv0`
Expected: FAIL — `plan` takes 3 args, not 4.

- [ ] **Step 3: Implement override + validation + field**

In `src/agent_resume.rs`:

Add to `PersistedAgentSession`:

```rust
pub struct PersistedAgentSession {
    pub source: String,
    pub agent: String,
    pub session_ref: AgentSessionRef,
    pub command: Option<String>,
}
```

Add the validator near `valid_session_id`:

```rust
fn valid_resume_command(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SESSION_ID_LEN
        && !value.chars().any(char::is_control)
        && !value.contains(['/', '\\'])
}
```

Change `plan` to accept and apply the override. Replace the signature and, after building `argv`, swap argv[0] when valid:

```rust
pub fn plan(
    source: &str,
    agent: &str,
    session_ref: &AgentSessionRef,
    command: Option<&str>,
) -> Option<AgentResumePlan> {
    if !is_official_agent_source(source, agent) {
        return None;
    }

    let mut argv = match (source, agent, session_ref.kind) {
        // ... unchanged arms ...
        _ => return None,
    };

    if let Some(command) = command.filter(|command| valid_resume_command(command)) {
        if let Some(first) = argv.first_mut() {
            *first = command.to_string();
        }
    }

    Some(AgentResumePlan {
        agent: agent.to_string(),
        argv,
        dedupe_key: dedupe_key(source, agent, session_ref),
    })
}
```

Update every existing `PersistedAgentSession { ... }` literal in this file's tests and in `session_ref_from_snapshot` to set `command: None` (the snapshot path fills it in Task 5). Update existing `plan(...)` test calls to pass `None` as the 4th argument.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run --lib agent_resume`
Expected: PASS (new test + existing `planner_allows_supported_agents` etc., now passing `None`).

- [ ] **Step 5: Commit**

```bash
git add src/agent_resume.rs
git commit -m "feat: support resume command override in agent resume planner"
```

---

### Task 5: Persist and restore the resume command

**Files:**
- Modify: `src/persist/snapshot.rs` (struct + capture)
- Modify: `src/persist/restore.rs` (`restore_plan_for_snapshot`, `persisted_agent_session_from_snapshot`)
- Modify: `src/agent_resume.rs` (`session_ref_from_snapshot` keeps `command: None`; command comes from snapshot field)
- Test: `src/persist/snapshot.rs` tests

**Interfaces:**
- Consumes: `TerminalState.persisted_agent_session` now carries `command` (set in Task 6); `agent_resume::plan(.., command)` (Task 4).
- Produces: `PaneAgentSessionSnapshot.command: Option<String>` survives capture/restore and reaches `plan`.

- [ ] **Step 1: Write the failing test**

In `src/persist/snapshot.rs` tests, extend the existing
`capture_contract_preserves_restored_agent_session` style test (add a new one):

```rust
#[test]
fn capture_contract_preserves_resume_command() {
    let (mut state, root) = AppState::test_with_single_pane();
    let terminal = state.terminal_for_pane_mut(root).expect("terminal");
    terminal.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
        source: "herdr:claude".into(),
        agent: "claude".into(),
        session_ref: crate::agent_resume::AgentSessionRef::id("xebia-session").unwrap(),
        command: Some("claude-xebia".into()),
    });
    let snapshot = capture(&state);
    let agent_session = snapshot.workspaces[0].tabs[0].panes[&root.raw()]
        .agent_session
        .as_ref()
        .expect("agent session");
    assert_eq!(agent_session.command.as_deref(), Some("claude-xebia"));
}
```

(Match the actual test helpers used by the neighboring snapshot tests — e.g. how they obtain `state`, `root`, and the terminal. Reuse the same accessors those tests use.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run capture_contract_preserves_resume_command`
Expected: FAIL — `PaneAgentSessionSnapshot` has no `command` field.

- [ ] **Step 3: Implement snapshot field + capture + restore**

In `src/persist/snapshot.rs`, add to `PaneAgentSessionSnapshot`:

```rust
pub struct PaneAgentSessionSnapshot {
    pub source: String,
    pub agent: String,
    pub kind: crate::agent_resume::AgentSessionRefKind,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}
```

In the capture sites (~lines 343-360) set `command`:
- For the hook-authority branch: `command: authority.session_ref... ` — hook authority does not carry a command, set `command: None`.
- For the `terminal.persisted_agent_session` branch: `command: session.command.clone()`.

In `src/persist/restore.rs`, update `restore_plan_for_snapshot`:

```rust
fn restore_plan_for_snapshot(
    session: &PaneAgentSessionSnapshot,
    resume_agents_on_restore: bool,
) -> Option<crate::agent_resume::AgentResumePlan> {
    if !resume_agents_on_restore {
        return None;
    }
    let persisted = persisted_agent_session_from_snapshot(session)?;
    crate::agent_resume::plan(
        &session.source,
        &session.agent,
        &persisted.session_ref,
        session.command.as_deref(),
    )
}
```

In `persisted_agent_session_from_snapshot`, set the restored `command`. Since
`session_ref_from_snapshot` returns a `PersistedAgentSession` with `command: None`,
copy it over after construction:

```rust
fn persisted_agent_session_from_snapshot(
    session: &PaneAgentSessionSnapshot,
) -> Option<crate::agent_resume::PersistedAgentSession> {
    let mut persisted = crate::agent_resume::session_ref_from_snapshot(
        &session.source,
        &session.agent,
        session.kind,
        &session.value,
    )?;
    persisted.command = session.command.clone();
    Some(persisted)
}
```

Set `command: None` in every other `PaneAgentSessionSnapshot { ... }` literal (the
`None`-agent_session test fixtures already use `None` for the whole struct, so only
real-session literals need the field).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run --lib persist`
Expected: PASS (new test + existing roundtrip tests; old snapshots without `command` still deserialize via `#[serde(default)]`).

- [ ] **Step 5: Commit**

```bash
git add src/persist/snapshot.rs src/persist/restore.rs src/agent_resume.rs
git commit -m "feat: persist resume command with agent sessions"
```

---

### Task 6: Capture the detected command and attach it to the session

**Files:**
- Modify: `src/terminal/state.rs` (`detected_command` field + setter; fill `PersistedAgentSession.command`)
- Modify: `src/events.rs` (new `AgentCommandDetected` event)
- Modify: `src/pane.rs` (emit the event at the agent-change site)
- Modify: `src/app/actions.rs` (handle the event)
- Test: `src/terminal/state.rs` tests

**Interfaces:**
- Produces:
  - `TerminalState.detected_command: Option<String>` + `pub fn set_detected_command(&mut self, command: Option<String>)`.
  - `AppEvent::AgentCommandDetected { pane_id: PaneId, command: Option<String> }`.
  - `set_agent_session_ref_for_session_start` writes `command: self.detected_command.clone()` into the new `PersistedAgentSession`.
- Consumes: `probe.process_name` (already computed in the detection task).

- [ ] **Step 1: Write the failing test**

In `src/terminal/state.rs` tests:

```rust
#[test]
fn session_report_captures_detected_command() {
    let mut terminal = TerminalState::new(TerminalId::new_for_test(), std::env::temp_dir());
    terminal.set_detected_command(Some("claude-xebia".into()));
    let _ = terminal.set_agent_session_ref_for_session_start(
        "herdr:claude".into(),
        "claude".into(),
        crate::agent_resume::AgentSessionRef::id("xebia-session"),
        Some(1),
        Some("startup".into()),
    );
    let session = terminal.persisted_agent_session.as_ref().expect("session");
    assert_eq!(session.command.as_deref(), Some("claude-xebia"));
}
```

(Use the same `TerminalId` test constructor and arg shapes the neighboring tests at
lines ~3316-3433 use; mirror one of those calls exactly and add the
`set_detected_command` line.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run session_report_captures_detected_command`
Expected: FAIL — `set_detected_command` does not exist / `command` field absent.

- [ ] **Step 3: Implement the field, setter, and capture**

In `src/terminal/state.rs`:
- Add field to `TerminalState`: `detected_command: Option<String>,` (after `agent_name`).
- Initialize `detected_command: None,` in `TerminalState::new`.
- Add setter:

```rust
pub fn set_detected_command(&mut self, command: Option<String>) {
    self.detected_command = command.filter(|command| !command.trim().is_empty());
}
```

- In `set_agent_session_ref_for_session_start`, set the new field when building the
  session:

```rust
        self.persisted_agent_session = Some(crate::agent_resume::PersistedAgentSession {
            source,
            agent: agent_label,
            session_ref,
            command: self.detected_command.clone(),
        });
```

(Apply the same `command: self.detected_command.clone()` to any other
`PersistedAgentSession { ... }` literal this file constructs from live state. Test-only
literals that assert specific values set `command` explicitly.)

In `src/events.rs`, add a variant to `AppEvent`:

```rust
    /// The foreground command name backing a pane's agent was detected.
    AgentCommandDetected {
        pane_id: PaneId,
        command: Option<String>,
    },
```

In `src/pane.rs`, at the agent-change block (around line 2049, inside `if changed {`
where `agent` and `process_name` are in scope), after the existing logging, emit the
event when an agent is present. `process_name` is currently consumed by the log arm;
capture it first:

```rust
                                let detected_command =
                                    agent.is_some().then(|| process_name.clone()).flatten();
                                let _ = state_events
                                    .send(AppEvent::AgentCommandDetected {
                                        pane_id,
                                        command: detected_command,
                                    })
                                    .await;
```

Place this just before the `if let Some(process_name) = process_name { info!... }`
block so `process_name` is still owned there (use `.clone()` above, or move the emit
after the log and reuse the value — keep one owner). The send is best-effort; ignore
the error like other non-critical emits.

In `src/app/actions.rs`, handle the event (next to `StateChanged`):

```rust
            AppEvent::AgentCommandDetected { pane_id, command } => self
                .update_terminal_state(pane_id, |terminal| {
                    terminal.set_detected_command(command);
                    None
                })
                .into_iter()
                .collect(),
```

(Match the exact return shape `update_terminal_state` expects in this match — mirror a
neighboring arm that returns no mutation, e.g. one returning `Vec::new()` if the
closure yields `Option<TerminalStateMutation>`.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run session_report_captures_detected_command`
Expected: PASS
Run: `cargo nextest run --lib terminal::state`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/terminal/state.rs src/events.rs src/pane.rs src/app/actions.rs
git commit -m "feat: capture launched command for agent session resume"
```

---

## Phase 3 — Installer support for multiple config dirs

### Task 7: Refactor `install_claude` to install into a given dir, then multiple

**Files:**
- Modify: `src/integration/targets.rs` (`install_claude_into`, `install_claude`)
- Modify: `src/integration/types.rs` (result carries per-dir paths)
- Modify: `src/integration/actions.rs` (report each path)
- Test: `src/integration/tests.rs`

**Interfaces:**
- Produces:
  - `fn install_claude_into(dir: &std::path::Path) -> io::Result<ClaudeInstallPaths>` (one dir; current body).
  - `install_claude() -> io::Result<Vec<ClaudeInstallPaths>>` installs into the default `claude_dir()` plus expanded, deduped `config_dirs` from config; a missing dir is a warning, not a failure.
  - `ClaudeInstallPaths` unchanged (per-dir); callers iterate.
- Consumes: `crate::config::Config::load()` to read `agents.config_dirs_for("claude")`; a shared tilde/env-expansion helper.

- [ ] **Step 1: Write the failing test**

In `src/integration/tests.rs` (these tests already set `HOME`/`CLAUDE_CONFIG_DIR` via the
existing harness — reuse it):

```rust
#[test]
fn install_claude_into_targets_explicit_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("acct");
    std::fs::create_dir_all(&dir).unwrap();
    let paths = super::targets::install_claude_into(&dir).unwrap();
    assert!(paths.hook_path.starts_with(&dir));
    assert!(dir.join("settings.json").is_file());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run install_claude_into_targets_explicit_dir`
Expected: FAIL — `install_claude_into` does not exist.

- [ ] **Step 3: Refactor the installer**

In `src/integration/targets.rs`:
- Rename the current `install_claude` body to `pub(crate) fn install_claude_into(dir: &Path) -> io::Result<ClaudeInstallPaths>`, replacing `let dir = claude_dir()?;` with the `dir` parameter (keep the `is_dir()` guard, using `dir`).
- Add a new `install_claude`:

```rust
pub(crate) fn install_claude() -> io::Result<ClaudeInstallResult> {
    let mut dirs: Vec<PathBuf> = vec![claude_dir()?];
    let loaded = crate::config::Config::load();
    for raw in loaded.config.agents.config_dirs_for("claude") {
        let expanded = expand_config_dir(&raw);
        if !dirs.iter().any(|existing| existing == &expanded) {
            dirs.push(expanded);
        }
    }
    install_claude_dirs(&dirs)
}

pub(crate) fn install_claude_dirs(dirs: &[PathBuf]) -> io::Result<ClaudeInstallResult> {
    let mut installed = Vec::new();
    let mut warnings = Vec::new();
    for dir in dirs {
        if !dir.is_dir() {
            warnings.push(format!("skipped {}: directory not found", dir.display()));
            continue;
        }
        installed.push(install_claude_into(dir)?);
    }
    Ok(ClaudeInstallResult { installed, warnings })
}
```

Add an expansion helper (reuse one if `src/integration/env.rs` already expands `~`;
otherwise add):

```rust
fn expand_config_dir(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(raw)
}
```

In `src/integration/types.rs` add:

```rust
pub(crate) struct ClaudeInstallResult {
    pub installed: Vec<ClaudeInstallPaths>,
    pub warnings: Vec<String>,
}
```

In `src/integration/actions.rs` (the `IntegrationTarget::Claude` arm, ~line 59), iterate:

```rust
        crate::api::schema::IntegrationTarget::Claude => {
            let result = install_claude()?;
            let mut messages = Vec::new();
            for installed in &result.installed {
                messages.push(format!(
                    "installed claude integration hook to {}",
                    installed.hook_path.display()
                ));
                messages.push(format!(
                    "ensured claude settings at {}",
                    installed.settings_path.display()
                ));
            }
            messages.extend(result.warnings);
            messages
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run install_claude_into_targets_explicit_dir`
Expected: PASS
Run: `cargo nextest run --lib integration`
Expected: PASS (existing `install_claude_*` tests still pass: default-dir install is the
first element of `installed`).

Note: existing tests calling `install_claude()` and reading `.hook_path` must be
updated to read `result.installed[0].hook_path`. Update them in this step.

- [ ] **Step 5: Commit**

```bash
git add src/integration/targets.rs src/integration/types.rs src/integration/actions.rs src/integration/tests.rs
git commit -m "feat: install claude hook into configured account dirs"
```

---

### Task 8: Uninstall mirror + `--config-dir` CLI flag

**Files:**
- Modify: `src/integration/targets.rs` (`uninstall_claude_from` / multi-dir `uninstall_claude`)
- Modify: `src/integration/actions.rs` (uninstall Claude arm reports each path)
- Modify: `src/cli/integration.rs` (`parse_integration_target` accepts repeatable `--config-dir`)
- Test: `src/cli/integration.rs` tests + `src/integration/tests.rs`

**Interfaces:**
- Produces:
  - `uninstall_claude` removes from default + configured dirs, reporting each.
  - CLI: `herdr integration install claude --config-dir <path>` (repeatable) installs into exactly the given dirs plus default; same for `uninstall`.
  - `parse_integration_target` returns `(IntegrationTarget, Vec<PathBuf> /* extra dirs */)`.
- Consumes: `install_claude_dirs` (Task 7).

- [ ] **Step 1: Write the failing test**

In `src/cli/integration.rs` tests:

```rust
#[test]
fn parse_target_collects_config_dir_flags() {
    let args = vec![
        "claude".to_string(),
        "--config-dir".to_string(),
        "/a".to_string(),
        "--config-dir".to_string(),
        "/b".to_string(),
    ];
    let parsed = parse_integration_target(&args, "install").unwrap().unwrap();
    assert_eq!(parsed.0, IntegrationTarget::Claude);
    assert_eq!(parsed.1, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run parse_target_collects_config_dir_flags`
Expected: FAIL — `parse_integration_target` returns `Option<IntegrationTarget>`, not a tuple.

- [ ] **Step 3: Implement the flag + uninstall mirror**

In `src/cli/integration.rs`:
- Change `parse_integration_target` to return `std::io::Result<Option<(IntegrationTarget, Vec<PathBuf>)>>`. Parse the first positional as the target, then walk remaining args collecting `--config-dir <path>` pairs; reject unknown flags / `--config-dir` for non-Claude targets with the existing usage error.
- Update `integration_install` / `integration_uninstall` to destructure `(target, extra_dirs)`. When `extra_dirs` is non-empty for Claude, call a new path that installs into `default + extra_dirs` (build the `Vec<PathBuf>` and call `install_claude_dirs`); otherwise call the config-driven `install_target(target)` as today. Print returned messages.

In `src/integration/targets.rs`:
- Add `pub(crate) fn uninstall_claude_from(dir: &Path) -> io::Result<ClaudeUninstallResult>` (current `uninstall_claude` body parameterized by dir) and a multi-dir `uninstall_claude()` that mirrors `install_claude()` (default + configured dirs, per-dir warnings).

In `src/integration/actions.rs`: update the Claude uninstall arm (~line 234) to iterate
results and report each removed/absent path, like Task 7's install arm.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run parse_target_collects_config_dir_flags`
Expected: PASS
Run: `cargo nextest run --lib integration cli::integration`
Expected: PASS (update any existing `parse_integration_target` callers/tests to the new
tuple return).

- [ ] **Step 5: Commit**

```bash
git add src/cli/integration.rs src/integration/targets.rs src/integration/actions.rs
git commit -m "feat: add --config-dir flag and multi-dir claude uninstall"
```

---

### Task 9: Documentation (docs/next)

**Files:**
- Create/Modify: `docs/next/website/src/content/docs/` — the integrations/config page covering agents.
- Modify: `docs/next/CHANGELOG.md` if present (stage the changelog line).

**Interfaces:** none (docs only).

- [ ] **Step 1: Find the docs target**

Run: `ls docs/next/website/src/content/docs/`
Identify the existing config/integration page (follow the existing structure; do not edit
generated `website/src/content/docs/preview/` or stable `website/src/content/docs/`).

- [ ] **Step 2: Write the docs**

Document, in the existing voice:
- The `[agents.<id>]` config block with `commands` and `config_dirs`:

  ```toml
  [agents.claude]
  commands    = ["claude-xebia", "claude-mtv", "claude-personal"]
  config_dirs = ["~/.claude-xebia", "~/.claude-mtv", "~/.claude-personal"]
  ```

- That `commands` makes herdr recognize those panes as the agent and resume them with the
  same command (the command must be a real executable on `PATH`, not a shell alias).
- That `config_dirs` (and `herdr integration install claude --config-dir <path>`) install
  the session hook into each account so resume data is captured.

- [ ] **Step 3: Stage changelog (if applicable)**

Add a line to `docs/next/CHANGELOG.md` such as:
`- Recognize and resume custom per-account agent commands via [agents] config.`

- [ ] **Step 4: Commit**

```bash
git add docs/next
git commit -m "docs: document custom agent command aliases"
```

---

## Final validation

- [ ] **Run the full gate**

Run: `just check`
Expected: formatting clean, `cargo nextest` green, maintenance script tests green.

- [ ] **Manual smoke (optional, from inside herdr)**

Per `CLAUDE.md`, test the debug build without inherited socket overrides:

```bash
env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH cargo run -- <command>
```

Add `[agents.claude] commands = ["claude-xebia"]`, launch a `claude-xebia` wrapper in a
pane, confirm the pane is recognized as Claude (state detection engages), then restart and
confirm resume relaunches `claude-xebia --resume <id>`.

---

## Self-Review

**Spec coverage:**
- Config surface (spec §1) → Task 2 (+ Task 3 wiring). ✓
- Recognition / global registry (spec §2) → Tasks 1, 3. ✓
- Resume with captured command (spec §3) → Tasks 4 (planner), 5 (persistence), 6 (capture). ✓
- Installer multi-dir + `--config-dir` (spec §4) → Tasks 7, 8. ✓
- Runtime/client boundary (spec §5) → registry/config/resume all server-side; reflected in Global Constraints. ✓
- Testing (spec §Testing) → every task is TDD; Final validation runs `just check`. ✓
- Risk classification (spec) → identity authority (Task 1), persisted state (Task 5), resume planner (Task 4) each keep existing characterization tests green; new tests extend them. ✓
- Setup docs note (spec Open setup note) → Task 9. ✓

**Placeholder scan:** No TBD/TODO; each code step shows code. Sites described as "mirror a
neighboring arm / match the existing helper" point at concrete, named anchors
(`update_terminal_state`, lines ~2049/234/3316) rather than vague hand-waving.

**Type consistency:** `set_agent_command_registry`, `registered_agent`,
`AgentsConfig::command_entries`/`config_dirs_for`, `PersistedAgentSession.command`,
`plan(.., command: Option<&str>)`, `PaneAgentSessionSnapshot.command`,
`TerminalState::set_detected_command`, `AppEvent::AgentCommandDetected`,
`install_claude_into`/`install_claude_dirs`/`ClaudeInstallResult`,
`parse_integration_target -> (IntegrationTarget, Vec<PathBuf>)` are used consistently across
the tasks that produce and consume them.
