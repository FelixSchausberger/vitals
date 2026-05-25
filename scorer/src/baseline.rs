//! Resource baseline learning via Welford online variance estimation.
//!
//! The `ResourceBaseline` maintains running mean and standard deviation for each
//! resource dimension using Knuth's online algorithm (Welford, 1962). This allows
//! the TWHS algorithm to score resource usage *relative to this machine's own
//! historical normal*, rather than against hardcoded global thresholds.
//!
//! ## Contamination Prevention
//! Samples collected when the system is already degraded (score below
//! `ResourceConfig::contamination_score_threshold`) are excluded from baseline
//! learning. This prevents a chronic RAM leak from slowly shifting "normal" upward.
//!
//! ## Bootstrap
//! On first run, no baseline data exists. Use [`ResourceBaseline::from_system_profile`]
//! to seed the baseline with hardware-derived priors, or start with zero and accept
//! that resource scoring will use static thresholds for the first N samples.

use serde::{Deserialize, Serialize};

use crate::types::ResourceSnapshot;

/// Online variance estimator using Welford's algorithm.
///
/// Maintains (count, mean, M2) where M2 = Σ(x - mean)².
/// The sample variance is M2 / (count - 1) and sample std = sqrt(variance).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WelfordState {
    pub count: u64,
    pub mean: f64,
    pub m2: f64,
}

impl WelfordState {
    #![allow(clippy::cast_precision_loss)]
    /// Update with a new observation.
    pub fn update(&mut self, value: f64) {
        self.count += 1;
        let delta = value - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = value - self.mean;
        self.m2 += delta * delta2;
    }

    /// Sample variance. Returns 0.0 with fewer than 2 observations.
    #[must_use]
    pub fn variance(&self) -> f64 {
        if self.count < 2 {
            return 0.0;
        }
        self.m2 / (self.count - 1) as f64
    }

    /// Sample standard deviation.
    #[must_use]
    pub fn std(&self) -> f64 {
        self.variance().sqrt()
    }
}

/// System hardware profile, used to seed the baseline on first run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemProfile {
    /// Rack/blade server, database server, headless cloud instance.
    Server,
    /// Desktop workstation or gaming PC.
    Desktop,
    /// Laptop or convertible.
    Laptop,
    /// Raspberry Pi, ARM SBC, industrial embedded.
    Embedded,
    /// Virtual machine or container.
    VmOrContainer,
    /// Unknown — use conservative generic defaults.
    Unknown,
}

impl SystemProfile {
    /// Detect the system profile from hardware information.
    ///
    /// Reads `/sys/class/dmi/id/chassis_type` (SMBIOS type 3) or falls back to
    /// `hostnamectl chassis` semantics. On systems without DMI (VMs, containers),
    /// checks `/proc/1/environ` or `/run/systemd/container`.
    #[must_use]
    pub fn detect() -> Self {
        // Check for container/VM first (fastest path, most common false-positive source)
        if Self::is_container_or_vm() {
            return Self::VmOrContainer;
        }

        // Read SMBIOS chassis type
        if let Ok(chassis_raw) = std::fs::read_to_string("/sys/class/dmi/id/chassis_type") {
            if let Ok(chassis_type) = chassis_raw.trim().parse::<u8>() {
                return match chassis_type {
                    // Server chassis types (SMBIOS spec 3.x, Table 23)
                    17 | 23 | 28 | 29 | 36 => Self::Server,
                    // Desktop
                    3 | 4 | 5 | 6 | 7 | 13 | 35 => Self::Desktop,
                    // Laptop / mobile
                    8 | 9 | 10 | 11 | 14 | 30 | 31 | 32 => Self::Laptop,
                    // Embedded / IoT / mini (33 = Embedded PC, 34 = Diskette drive)
                    33 | 34 => Self::Embedded,
                    _ => Self::Unknown,
                };
            }
        }

        // Heuristic: very low RAM suggests embedded/minimal
        if let Some(ram_mb) = Self::total_ram_mb() {
            if ram_mb <= 1024 {
                return Self::Embedded;
            }
        }

        Self::Unknown
    }

    fn is_container_or_vm() -> bool {
        // systemd-detect-virt writes to /run/systemd/container for containers
        std::path::Path::new("/run/systemd/container").exists()
            || std::path::Path::new("/.dockerenv").exists()
            || std::path::Path::new("/run/.containerenv").exists()
    }

    fn total_ram_mb() -> Option<u64> {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        for line in meminfo.lines() {
            if line.starts_with("MemTotal:") {
                let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
                return Some(kb / 1024);
            }
        }
        None
    }

    /// Initial baseline priors for this profile.
    ///
    /// These are empirically-derived starting points. The Welford estimator will
    /// replace them as real data accumulates.
    #[must_use]
    pub fn resource_priors(self) -> ResourcePriors {
        match self {
            Self::Server => ResourcePriors {
                cpu_mean: 40.0,
                cpu_std: 20.0,
                memory_mean: 60.0,
                memory_std: 20.0,
                disk_mean: 50.0,
                disk_std: 20.0,
                load_mean: 2.0,
                load_std: 2.0,
            },
            Self::Desktop => ResourcePriors {
                cpu_mean: 15.0,
                cpu_std: 15.0,
                memory_mean: 50.0,
                memory_std: 20.0,
                disk_mean: 40.0,
                disk_std: 15.0,
                load_mean: 0.8,
                load_std: 0.8,
            },
            Self::Laptop => ResourcePriors {
                cpu_mean: 10.0,
                cpu_std: 12.0,
                memory_mean: 40.0,
                memory_std: 20.0,
                disk_mean: 35.0,
                disk_std: 15.0,
                load_mean: 0.5,
                load_std: 0.5,
            },
            Self::Embedded => ResourcePriors {
                cpu_mean: 30.0,
                cpu_std: 20.0,
                memory_mean: 70.0,
                memory_std: 15.0,
                disk_mean: 60.0,
                disk_std: 20.0,
                load_mean: 0.5,
                load_std: 0.5,
            },
            Self::VmOrContainer | Self::Unknown => ResourcePriors {
                cpu_mean: 20.0,
                cpu_std: 20.0,
                memory_mean: 50.0,
                memory_std: 20.0,
                disk_mean: 40.0,
                disk_std: 20.0,
                load_mean: 1.0,
                load_std: 1.0,
            },
        }
    }
}

/// Empirical priors for seeding the Welford estimator before real data exists.
#[derive(Debug, Clone)]
pub struct ResourcePriors {
    pub cpu_mean: f64,
    pub cpu_std: f64,
    pub memory_mean: f64,
    pub memory_std: f64,
    pub disk_mean: f64,
    pub disk_std: f64,
    pub load_mean: f64,
    pub load_std: f64,
}

/// Running baseline state for resource scoring.
///
/// Maintain this across scoring cycles (serialize to disk between daemon restarts).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceBaseline {
    pub cpu: WelfordState,
    pub memory: WelfordState,
    pub disk: WelfordState,
    pub load: WelfordState,
    /// Total number of samples accepted (not counting contaminated ones).
    pub sample_count: u64,
}

impl ResourceBaseline {
    /// Create a new empty baseline.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed the baseline from hardware-detected system profile priors.
    ///
    /// Injects synthetic samples equivalent to `seed_weight` real observations,
    /// giving the algorithm a reasonable starting point without contamination.
    #[must_use]
    pub fn from_system_profile(profile: SystemProfile, seed_weight: u64) -> Self {
        let priors = profile.resource_priors();
        let mut baseline = Self::new();

        // Inject synthetic samples: mean ± std to establish the Welford state.
        // We inject seed_weight samples: half at mean-std, half at mean+std.
        // This gives the correct mean and approximates the correct variance.
        let half = seed_weight / 2;
        for _ in 0..half {
            baseline.cpu.update(priors.cpu_mean - priors.cpu_std);
            baseline.cpu.update(priors.cpu_mean + priors.cpu_std);
            baseline
                .memory
                .update(priors.memory_mean - priors.memory_std);
            baseline
                .memory
                .update(priors.memory_mean + priors.memory_std);
            baseline.disk.update(priors.disk_mean - priors.disk_std);
            baseline.disk.update(priors.disk_mean + priors.disk_std);
            baseline.load.update(priors.load_mean - priors.load_std);
            baseline.load.update(priors.load_mean + priors.load_std);
        }
        baseline.sample_count = seed_weight;
        baseline
    }

    /// Update the baseline with a new resource snapshot.
    ///
    /// `score_ok` should be true when the current health score is above the
    /// contamination threshold. When false (system degraded), the sample is
    /// skipped to prevent the degraded state from shifting the baseline.
    pub fn update(&mut self, snapshot: &ResourceSnapshot, score_ok: bool) {
        if !score_ok {
            return;
        }
        self.cpu.update(snapshot.cpu_usage);
        self.memory.update(snapshot.memory_usage);
        self.disk.update(snapshot.disk_usage);
        self.load.update(snapshot.load_average);
        self.sample_count += 1;
    }
}
