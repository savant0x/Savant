//! Pressure level classification for resource-aware agent spawning.

use savant_core::config::ResourceGovernorConfig;

/// System resource pressure level. Worst-case wins (max of CPU and memory).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PressureLevel {
    Low = 0,
    Medium = 1,
    High = 2,
    Critical = 3,
}

impl PressureLevel {
    /// Classify pressure from CPU and memory percentages.
    /// Worst-case wins: max(cpu_pressure, mem_pressure).
    pub fn from_metrics(cpu_pct: f64, mem_pct: f64, config: &ResourceGovernorConfig) -> Self {
        let cpu_level = if cpu_pct >= config.cpu_critical_pct {
            3
        } else if cpu_pct >= config.cpu_high_pct {
            2
        } else if cpu_pct >= config.cpu_medium_pct {
            1
        } else {
            0
        };

        let mem_level = if mem_pct >= config.memory_critical_pct {
            3
        } else if mem_pct >= config.memory_high_pct {
            2
        } else if mem_pct >= config.memory_medium_pct {
            1
        } else {
            0
        };

        match cpu_level.max(mem_level) {
            0 => Self::Low,
            1 => Self::Medium,
            2 => Self::High,
            _ => Self::Critical,
        }
    }

    /// Max concurrent agents at this pressure level.
    pub fn max_agents(&self, config: &ResourceGovernorConfig) -> usize {
        match self {
            Self::Low => config.max_agents_low,
            Self::Medium => config.max_agents_medium,
            Self::High => config.max_agents_high,
            Self::Critical => config.max_agents_critical,
        }
    }
}

impl std::fmt::Display for PressureLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "LOW"),
            Self::Medium => write!(f, "MEDIUM"),
            Self::High => write!(f, "HIGH"),
            Self::Critical => write!(f, "CRITICAL"),
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn test_config() -> ResourceGovernorConfig {
        ResourceGovernorConfig {
            enabled: true,
            monitor_interval_secs: 5,
            memory_medium_pct: 60.0,
            memory_high_pct: 80.0,
            memory_critical_pct: 92.0,
            cpu_medium_pct: 70.0,
            cpu_high_pct: 85.0,
            cpu_critical_pct: 95.0,
            max_agents_low: 128,
            max_agents_medium: 64,
            max_agents_high: 32,
            max_agents_critical: 8,
            max_deferral_retries: 60,
            smoothing_factor: 0.7,
        }
    }

    #[test]
    fn test_pressure_low() {
        let config = test_config();
        assert_eq!(
            PressureLevel::from_metrics(10.0, 20.0, &config),
            PressureLevel::Low
        );
    }

    #[test]
    fn test_pressure_medium() {
        let config = test_config();
        assert_eq!(
            PressureLevel::from_metrics(75.0, 20.0, &config),
            PressureLevel::Medium
        );
        assert_eq!(
            PressureLevel::from_metrics(10.0, 65.0, &config),
            PressureLevel::Medium
        );
    }

    #[test]
    fn test_pressure_high() {
        let config = test_config();
        assert_eq!(
            PressureLevel::from_metrics(90.0, 20.0, &config),
            PressureLevel::High
        );
        assert_eq!(
            PressureLevel::from_metrics(10.0, 85.0, &config),
            PressureLevel::High
        );
    }

    #[test]
    fn test_pressure_critical() {
        let config = test_config();
        assert_eq!(
            PressureLevel::from_metrics(96.0, 20.0, &config),
            PressureLevel::Critical
        );
        assert_eq!(
            PressureLevel::from_metrics(10.0, 95.0, &config),
            PressureLevel::Critical
        );
    }

    #[test]
    fn test_worst_case_wins() {
        let config = test_config();
        assert_eq!(
            PressureLevel::from_metrics(10.0, 85.0, &config),
            PressureLevel::High
        );
    }

    #[test]
    fn test_max_agents_per_level() {
        let config = test_config();
        assert_eq!(PressureLevel::Low.max_agents(&config), 128);
        assert_eq!(PressureLevel::Medium.max_agents(&config), 64);
        assert_eq!(PressureLevel::High.max_agents(&config), 32);
        assert_eq!(PressureLevel::Critical.max_agents(&config), 8);
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", PressureLevel::Low), "LOW");
        assert_eq!(format!("{}", PressureLevel::Critical), "CRITICAL");
    }
}
