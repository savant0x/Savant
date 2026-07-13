use chrono::{DateTime, Utc};
use std::collections::HashMap;

use super::facets::{FacetCategory, PreferenceFacet};

/// In-memory cache for user preference facets with observation counting
/// and stability filtering. Periodically flushes to a workspace file.
pub struct FacetCache {
    facets: HashMap<(FacetCategory, String), PreferenceFacet>,
    /// Minimum observations before a facet is considered stable.
    min_observations: u32,
    /// Minimum age (in days) before a facet is considered stable.
    min_age_days: i64,
    /// Facets not seen in this many days are pruned.
    expire_days: i64,
}

impl Default for FacetCache {
    fn default() -> Self {
        Self::new()
    }
}

impl FacetCache {
    pub fn new() -> Self {
        Self {
            facets: HashMap::new(),
            min_observations: 3,
            min_age_days: 0,
            expire_days: 90,
        }
    }

    /// Observe a facet — merge with existing (increment count, update last_seen).
    pub fn observe(&mut self, facet: PreferenceFacet) {
        let key = (facet.category.clone(), facet.key.clone());
        if let Some(existing) = self.facets.get_mut(&key) {
            existing.observation_count += 1;
            existing.last_seen = facet.last_seen;
            // Update value if newer observation has different value
            existing.value = facet.value;
        } else {
            self.facets.insert(key, facet);
        }
    }

    /// Return facets meeting the min_observations threshold.
    pub fn stable_facets(&self) -> Vec<&PreferenceFacet> {
        let now = Utc::now();
        self.facets
            .values()
            .filter(|f| {
                f.observation_count >= self.min_observations
                    && (now - f.first_seen).num_days() >= self.min_age_days
            })
            .collect()
    }

    /// Remove facets not seen in expire_days.
    pub fn prune_expired(&mut self) {
        let now = Utc::now();
        self.facets
            .retain(|_, f| (now - f.last_seen).num_days() < self.expire_days);
    }

    /// Total facets in cache (including unstable).
    pub fn len(&self) -> usize {
        self.facets.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.facets.is_empty()
    }

    /// Save facets to a workspace file for persistence across restarts.
    pub fn save_to_file(&self, path: &std::path::Path) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(&self.serializable_facets())
            .unwrap_or_else(|_| "[]".to_string());
        std::fs::write(path, json)
    }

    /// Load facets from a workspace file.
    pub fn load_from_file(&mut self, path: &std::path::Path) -> Result<(), std::io::Error> {
        if !path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(path)?;
        if let Ok(facets) = serde_json::from_str::<Vec<SerializableFacet>>(&content) {
            for sf in facets {
                if let Some(facet) = sf.as_facet() {
                    let key = (facet.category.clone(), facet.key.clone());
                    self.facets.insert(key, facet);
                }
            }
        }
        Ok(())
    }

    fn serializable_facets(&self) -> Vec<SerializableFacet> {
        self.facets
            .values()
            .map(|f| SerializableFacet {
                category: format!("{:?}", f.category),
                key: f.key.clone(),
                value: f.value.clone(),
                observation_count: f.observation_count,
                first_seen: f.first_seen.to_rfc3339(),
                last_seen: f.last_seen.to_rfc3339(),
            })
            .collect()
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SerializableFacet {
    category: String,
    key: String,
    value: String,
    observation_count: u32,
    first_seen: String,
    last_seen: String,
}

impl SerializableFacet {
    fn as_facet(&self) -> Option<PreferenceFacet> {
        let category = match self.category.as_str() {
            "Style" => FacetCategory::Style,
            "Identity" => FacetCategory::Identity,
            "Tooling" => FacetCategory::Tooling,
            "Veto" => FacetCategory::Veto,
            "Goal" => FacetCategory::Goal,
            _ => return None,
        };
        Some(PreferenceFacet {
            category,
            key: self.key.clone(),
            value: self.value.clone(),
            observation_count: self.observation_count,
            first_seen: DateTime::parse_from_rfc3339(&self.first_seen)
                .ok()?
                .with_timezone(&Utc),
            last_seen: DateTime::parse_from_rfc3339(&self.last_seen)
                .ok()?
                .with_timezone(&Utc),
        })
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn sample_facet(category: FacetCategory, key: &str, value: &str) -> PreferenceFacet {
        PreferenceFacet {
            category,
            key: key.to_string(),
            value: value.to_string(),
            observation_count: 1,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
        }
    }

    #[test]
    fn test_observe_merges() {
        let mut cache = FacetCache::new();
        cache.observe(sample_facet(FacetCategory::Style, "preference", "terse"));
        cache.observe(sample_facet(FacetCategory::Style, "preference", "terse"));
        cache.observe(sample_facet(FacetCategory::Style, "preference", "terse"));
        assert_eq!(cache.len(), 1);
        let facets: Vec<_> = cache.facets.values().collect();
        assert_eq!(facets[0].observation_count, 3);
    }

    #[test]
    fn test_stable_facets_filtering() {
        let mut cache = FacetCache::new();
        cache.min_observations = 2;
        cache.min_age_days = 0; // no age requirement for test
        cache.observe(sample_facet(FacetCategory::Tooling, "lang", "rust"));
        assert!(cache.stable_facets().is_empty()); // only 1 observation
        cache.observe(sample_facet(FacetCategory::Tooling, "lang", "rust"));
        assert_eq!(cache.stable_facets().len(), 1); // 2 observations
    }

    #[test]
    fn test_prune_expired() {
        let mut cache = FacetCache::new();
        cache.expire_days = 0; // immediate expiry
        cache.observe(sample_facet(FacetCategory::Veto, "action", "push"));
        cache.prune_expired();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_save_and_load() {
        let mut cache = FacetCache::new();
        cache.observe(sample_facet(FacetCategory::Style, "preference", "terse"));
        let path = std::path::Path::new("test_facets.json");
        cache.save_to_file(path).unwrap();

        let mut cache2 = FacetCache::new();
        cache2.load_from_file(path).unwrap();
        assert_eq!(cache2.len(), 1);

        // Cleanup
        let _ = std::fs::remove_file(path);
    }
}
