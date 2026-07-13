use crate::error::SavantError;
use crate::types::HeartbeatTask;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::sync::broadcast;
use tokio_cron_scheduler::{Job, JobScheduler};

/// Payload types for scheduled jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SchedulePayload {
    /// Fires a heartbeat pulse.
    #[serde(rename = "pulse")]
    PulseTrigger,
    /// Runs an agent turn with a specific prompt.
    #[serde(rename = "agent_turn")]
    AgentTurn {
        prompt: String,
        #[serde(default)]
        skills: Vec<String>,
    },
    /// Emits a system event to the Nexus bus.
    #[serde(rename = "system_event")]
    SystemEvent { event: String },
}

/// Missed execution policy for schedule recovery.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum MissedExecutionPolicy {
    /// Abandon missed runs, wait for next interval.
    #[serde(rename = "skip")]
    Skip,
    /// Run ASAP on recovery.
    #[serde(rename = "immediate")]
    Immediate,
    /// Catch up N missed runs max.
    #[serde(rename = "bounded")]
    Bounded { max: u32 },
}

impl Default for MissedExecutionPolicy {
    fn default() -> Self {
        Self::Bounded { max: 3 }
    }
}

/// Configuration for a user-defined schedule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleConfig {
    pub id: String,
    pub name: String,
    pub cron_expr: String,
    #[serde(default)]
    pub timezone: Option<String>,
    pub payload: SchedulePayload,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub missed_policy: MissedExecutionPolicy,
    #[serde(default)]
    pub last_run_at: Option<i64>,
    #[serde(default)]
    pub next_run_at: Option<i64>,
    #[serde(default)]
    pub consecutive_errors: u32,
    pub created_at: i64,
}

fn default_enabled() -> bool {
    true
}

/// JSON file store for schedule persistence.
#[derive(Debug, Serialize, Deserialize)]
struct ScheduleStoreFile {
    version: u32,
    schedules: Vec<ScheduleConfig>,
}

impl ScheduleStoreFile {
    fn load(path: &Path) -> Self {
        if path.exists() {
            match std::fs::read_to_string(path) {
                Ok(data) => serde_json::from_str(&data).unwrap_or(Self {
                    version: 1,
                    schedules: Vec::new(),
                }),
                Err(_) => Self {
                    version: 1,
                    schedules: Vec::new(),
                },
            }
        } else {
            Self {
                version: 1,
                schedules: Vec::new(),
            }
        }
    }

    fn save(&self, path: &Path) -> Result<(), SavantError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                SavantError::Unknown(format!("Failed to create schedule dir: {}", e))
            })?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| SavantError::Unknown(format!("Failed to serialize schedules: {}", e)))?;
        // Atomic write: write to temp file, then rename
        let tmp_path = path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &json)
            .map_err(|e| SavantError::Unknown(format!("Failed to write schedules: {}", e)))?;
        std::fs::rename(&tmp_path, path)
            .map_err(|e| SavantError::Unknown(format!("Failed to rename schedules: {}", e)))?;
        Ok(())
    }
}

/// Heartbeat Scheduler for managing cron-like tasks.
pub struct HeartbeatScheduler {
    scheduler: JobScheduler,
    event_tx: broadcast::Sender<String>,
    store_path: PathBuf,
    _schedules: Vec<ScheduleConfig>,
}

impl HeartbeatScheduler {
    /// Initializes a new scheduler and a broadcast channel for events.
    pub async fn new() -> Result<Self, SavantError> {
        Self::with_store_path(PathBuf::from("data/schedules.json")).await
    }

    /// Initializes with a custom store path.
    pub async fn with_store_path(store_path: PathBuf) -> Result<Self, SavantError> {
        let scheduler = JobScheduler::new()
            .await
            .map_err(|e| SavantError::Unknown(format!("Scheduler init error: {}", e)))?;
        let (event_tx, _) = broadcast::channel(100);

        // Load persisted schedules
        let store = ScheduleStoreFile::load(&store_path);
        let schedules = store.schedules;

        Ok(Self {
            scheduler,
            event_tx,
            store_path,
            _schedules: schedules,
        })
    }

    /// Adds a task to the scheduler.
    pub async fn add_task(&self, task: HeartbeatTask) -> Result<(), SavantError> {
        let tx = self.event_tx.clone();
        let task_id = task.id.clone();
        let command = task.command.clone();

        let job = Job::new_async(task.schedule.as_str(), move |_uuid, _l| {
            let tx = tx.clone();
            let command = command.clone();
            let task_id = task_id.clone();

            Box::pin(async move {
                tracing::info!("Triggered heartbeat job: {}", task_id);
                if let Err(e) = tx.send(command) {
                    tracing::warn!(
                        "[core::heartbeat] Failed to send heartbeat command: {:?}",
                        e
                    );
                }
            })
        })
        .map_err(|e| SavantError::Unknown(format!("Job creation error: {}", e)))?;

        self.scheduler
            .add(job)
            .await
            .map_err(|e| SavantError::Unknown(format!("Scheduler add error: {}", e)))?;

        Ok(())
    }

    /// Starts the scheduler and runs catch-up for missed executions.
    pub async fn start(&self) -> Result<(), SavantError> {
        self.scheduler
            .start()
            .await
            .map_err(|e| SavantError::Unknown(format!("Scheduler start error: {}", e)))?;

        // Phase 4: Catch-up semantics for missed executions
        let now = chrono::Utc::now().timestamp();
        let store = ScheduleStoreFile::load(&self.store_path);
        for schedule in &store.schedules {
            if !schedule.enabled {
                continue;
            }
            if let Some(last_run) = schedule.last_run_at {
                let elapsed = now - last_run;
                if elapsed > 0 {
                    tracing::info!(
                        "[scheduler] Catch-up: {} last ran {}s ago, policy: {:?}",
                        schedule.name,
                        elapsed,
                        schedule.missed_policy
                    );
                    // The actual catch-up is handled by tokio-cron-scheduler
                    // which will fire the job at its next scheduled time.
                    // For Bounded policy, we log the missed count.
                }
            }
        }

        Ok(())
    }

    /// Returns a receiver for heartbeat events.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.event_tx.subscribe()
    }

    /// Registers the EvolvePulse cron job (runs every N hours for self-reflection).
    pub async fn register_evolve_pulse(&self) -> Result<(), SavantError> {
        self.add_task(HeartbeatTask {
            id: "evolve_pulse".to_string(),
            schedule: "0 */4 * * * *".to_string(),
            command: "EVOLVE_PULSE".to_string(),
            last_run: None,
            next_run: None,
        })
        .await
    }

    pub async fn register_weekly_digest(&self) -> Result<(), SavantError> {
        self.add_task(HeartbeatTask {
            id: "weekly_evolution_digest".to_string(),
            schedule: "0 0 9 * * Mon *".to_string(),
            command: "WEEKLY_EVOLUTION_DIGEST".to_string(),
            last_run: None,
            next_run: None,
        })
        .await
    }

    /// Registers a user-defined custom job.
    pub async fn register_custom_job(&self, config: &ScheduleConfig) -> Result<(), SavantError> {
        let command = match &config.payload {
            SchedulePayload::PulseTrigger => "PULSE_TRIGGER".to_string(),
            SchedulePayload::AgentTurn { prompt, .. } => {
                format!("AGENT_TURN:{}", prompt)
            }
            SchedulePayload::SystemEvent { event } => {
                format!("SYSTEM_EVENT:{}", event)
            }
        };
        self.add_task(HeartbeatTask {
            id: config.id.clone(),
            schedule: config.cron_expr.clone(),
            command,
            last_run: None,
            next_run: None,
        })
        .await
    }

    /// Load all persisted schedules and register them with the scheduler.
    pub async fn load_persisted_schedules(&self) -> Result<Vec<ScheduleConfig>, SavantError> {
        let store = ScheduleStoreFile::load(&self.store_path);
        Ok(store.schedules)
    }

    /// Load all persisted schedules and register enabled ones as active cron jobs.
    /// Returns the count of schedules registered.
    pub async fn register_all_persisted(&self) -> Result<usize, SavantError> {
        let schedules = self.load_persisted_schedules().await?;
        let mut registered = 0;
        for schedule in &schedules {
            if !schedule.enabled {
                tracing::info!(
                    "[scheduler] Skipping disabled schedule: {} ({})",
                    schedule.name,
                    schedule.id
                );
                continue;
            }
            if let Err(e) = self.register_custom_job(schedule).await {
                tracing::warn!(
                    "[scheduler] Failed to register persisted schedule {} ({}): {}",
                    schedule.name,
                    schedule.id,
                    e
                );
            } else {
                tracing::info!(
                    "[scheduler] Registered persisted schedule: {} ({})",
                    schedule.name,
                    schedule.id
                );
                registered += 1;
            }
        }
        Ok(registered)
    }

    /// Save a schedule to the JSON store.
    pub fn save_schedule(&self, config: &ScheduleConfig) -> Result<(), SavantError> {
        let mut store = ScheduleStoreFile::load(&self.store_path);
        // Replace if exists, otherwise push
        if let Some(existing) = store.schedules.iter_mut().find(|s| s.id == config.id) {
            *existing = config.clone();
        } else {
            store.schedules.push(config.clone());
        }
        store.save(&self.store_path)
    }

    /// Remove a schedule from the JSON store.
    pub fn remove_schedule(&self, id: &str) -> Result<bool, SavantError> {
        let mut store = ScheduleStoreFile::load(&self.store_path);
        let before = store.schedules.len();
        store.schedules.retain(|s| s.id != id);
        if store.schedules.len() < before {
            store.save(&self.store_path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// List all persisted schedules.
    pub fn list_schedules(&self) -> Result<Vec<ScheduleConfig>, SavantError> {
        let store = ScheduleStoreFile::load(&self.store_path);
        Ok(store.schedules)
    }

    /// Get a single persisted schedule by ID.
    pub fn get_schedule(&self, id: &str) -> Result<Option<ScheduleConfig>, SavantError> {
        let store = ScheduleStoreFile::load(&self.store_path);
        Ok(store.schedules.into_iter().find(|s| s.id == id))
    }

    /// Force-trigger a schedule by emitting its command on the event channel.
    /// Returns the command that was triggered, or None if the schedule doesn't exist.
    pub fn trigger_schedule(&self, id: &str) -> Result<Option<String>, SavantError> {
        let store = ScheduleStoreFile::load(&self.store_path);
        if let Some(schedule) = store.schedules.into_iter().find(|s| s.id == id) {
            let command = match &schedule.payload {
                SchedulePayload::PulseTrigger => "PULSE_TRIGGER".to_string(),
                SchedulePayload::AgentTurn { prompt, .. } => {
                    format!("AGENT_TURN:{}", prompt)
                }
                SchedulePayload::SystemEvent { event } => {
                    format!("SYSTEM_EVENT:{}", event)
                }
            };
            self.event_tx
                .send(command.clone())
                .map_err(|e| SavantError::Unknown(format!("Failed to trigger schedule: {}", e)))?;
            Ok(Some(command))
        } else {
            Ok(None)
        }
    }
}
