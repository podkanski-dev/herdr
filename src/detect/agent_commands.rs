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
#[allow(dead_code)] // consumed by the config-loading layer in a later task
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
