//! Three-layer rule overlay — project > user > builtin.

use crate::compact::rules::RuleRegistry;
use crate::compact::schema::*;
use std::path::PathBuf;
use std::sync::Arc;

/// Manages the three-layer rule overlay.
#[derive(Debug, Clone)]
pub struct ThreeLayerOverlay {
    registry: RuleRegistry,
}

impl ThreeLayerOverlay {
    /// Creates a new overlay with the given user and project rule directories.
    pub fn new(user_rules_dir: PathBuf, project_rules_dir: PathBuf) -> Self {
        let registry = RuleRegistry::new(user_rules_dir, project_rules_dir);
        Self { registry }
    }

    /// Returns all compiled rules.
    pub fn all_rules(&self) -> Vec<Arc<CompiledRule>> {
        self.registry.all_rules().to_vec()
    }

    /// Reloads rules from all layers.
    pub fn reload(&mut self) {
        self.registry.recompile();
    }

    /// Returns the number of rules.
    pub fn rule_count(&self) -> usize {
        self.registry.len()
    }
}
