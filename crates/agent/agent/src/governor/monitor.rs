//! Resource Monitor — polls CPU and memory usage, publishes pressure level.

use savant_core::config::ResourceGovernorConfig;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::pressure::PressureLevel;

/// Background resource monitor. Polls system metrics and publishes pressure level.
/// Uses EMA (Exponential Moving Average) smoothing to prevent transient CPU spikes
/// from triggering pressure changes.
pub struct ResourceMonitor {
    config: ResourceGovernorConfig,
    /// Current pressure level as u8 (0=Low, 1=Medium, 2=High, 3=Critical)
    pressure: Arc<AtomicU8>,
    /// Current CPU usage (f64 as u64 bits)
    cpu_pct: Arc<AtomicU64>,
    /// Current memory usage (f64 as u64 bits)
    mem_pct: Arc<AtomicU64>,
    /// Shutdown signal
    shutdown: CancellationToken,
    /// Pressure change notification
    pressure_tx: watch::Sender<PressureLevel>,
    pub pressure_rx: watch::Receiver<PressureLevel>,
}

impl ResourceMonitor {
    pub fn new(config: ResourceGovernorConfig, shutdown: CancellationToken) -> Arc<Self> {
        let (pressure_tx, pressure_rx) = watch::channel(PressureLevel::Low);
        Arc::new(Self {
            config,
            pressure: Arc::new(AtomicU8::new(0)),
            cpu_pct: Arc::new(AtomicU64::new(0)),
            mem_pct: Arc::new(AtomicU64::new(0)),
            shutdown,
            pressure_tx,
            pressure_rx,
        })
    }

    /// Start the background monitoring task.
    pub fn start(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let monitor = self.clone();
        tokio::spawn(async move { monitor.run().await })
    }

    async fn run(&self) {
        let mut sys = sysinfo::System::new();
        let interval = std::time::Duration::from_secs(self.config.monitor_interval_secs.max(1));
        let alpha = self.config.smoothing_factor.clamp(0.1, 0.99); // EMA weight for history
        let mut smoothed_cpu: f64 = 0.0;
        let mut smoothed_mem: f64 = 0.0;

        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => break,
                _ = tokio::time::sleep(interval) => {
                    // Refresh system metrics (same pattern as perception.rs)
                    sys.refresh_cpu_all();
                    sys.refresh_memory();

                    let cpu_pct = sys.global_cpu_usage() as f64;
                    let total_mem = sys.total_memory() as f64;
                    let used_mem = sys.used_memory() as f64;
                    let mem_pct = if total_mem > 0.0 { (used_mem / total_mem) * 100.0 } else { 0.0 };

                    self.cpu_pct.store(cpu_pct.to_bits(), Ordering::Relaxed);
                    self.mem_pct.store(mem_pct.to_bits(), Ordering::Relaxed);

                    // EMA smoothing — absorbs transient spikes, responds to sustained load
                    smoothed_cpu = smoothed_cpu * alpha + cpu_pct * (1.0 - alpha);
                    smoothed_mem = smoothed_mem * alpha + mem_pct * (1.0 - alpha);

                    let level = if self.config.enabled {
                        PressureLevel::from_metrics(smoothed_cpu, smoothed_mem, &self.config)
                    } else {
                        PressureLevel::Low
                    };

                    let old = self.pressure.swap(level as u8, Ordering::Relaxed);
                    if old != level as u8 {
                        tracing::info!(
                            "[governor] Pressure changed: {} → {} (CPU={:.1}%, MEM={:.1}%)",
                            PressureLevel::from_u8(old), level, cpu_pct, mem_pct
                        );
                        let _ = self.pressure_tx.send(level);
                    }
                }
            }
        }
    }

    pub fn current_pressure(&self) -> PressureLevel {
        PressureLevel::from_u8(self.pressure.load(Ordering::Relaxed))
    }

    pub fn current_metrics(&self) -> (f64, f64) {
        let cpu = f64::from_bits(self.cpu_pct.load(Ordering::Relaxed));
        let mem = f64::from_bits(self.mem_pct.load(Ordering::Relaxed));
        debug_assert!(cpu.is_finite(), "cpu_pct is not finite: {}", cpu);
        debug_assert!(mem.is_finite(), "mem_pct is not finite: {}", mem);
        (cpu.max(0.0), mem.max(0.0))
    }
}

impl PressureLevel {
    fn from_u8(val: u8) -> Self {
        match val {
            1 => Self::Medium,
            2 => Self::High,
            3 => Self::Critical,
            _ => Self::Low,
        }
    }
}
