//! cgroup v2 CPU sampling for the app container (`NFR-0030`,
//! `docs/adrs/0018`'s AX4h addendum).
//!
//! The app container runs under a CPU cap, so "how many CPU-milliseconds
//! did one request cost" is the metric that survives noisy neighbours
//! better than latency does. The kernel keeps a cumulative counter per
//! cgroup in `cpu.stat`; this module samples it at a fixed interval during
//! the whole run and, afterwards, reads the delta over the *measured
//! window* (the warm-up ramp excluded) off a linear interpolation between
//! the two samples that bracket each window edge.
//!
//! Reading a file (rather than shelling out to `docker exec`) keeps the
//! sampler dependency-free: `run-real-stack.sh` bind-mounts the app
//! container's cgroup directory read-only into the goose container.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// The `cpu.stat` counters this tier reports (all cumulative since the
/// cgroup was created).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuStat {
    pub usage_usec: u64,
    pub user_usec: u64,
    pub system_usec: u64,
    pub nr_throttled: u64,
    pub throttled_usec: u64,
}

/// One timestamped reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub unix_ms: u64,
    pub stat: CpuStat,
}

/// Parse the text of a cgroup v2 `cpu.stat` file. `usage_usec`,
/// `user_usec` and `system_usec` must be present; the throttling counters
/// are absent when the `cpu` controller is not enabled for the cgroup and
/// then read as 0.
pub fn parse_cpu_stat(text: &str) -> Result<CpuStat, String> {
    let field = |name: &str| -> Result<Option<u64>, String> {
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            if parts.next() == Some(name) {
                let value = parts
                    .next()
                    .ok_or_else(|| format!("cpu.stat field {name} has no value"))?;
                return value
                    .parse()
                    .map(Some)
                    .map_err(|_| format!("cpu.stat field {name} is not an integer: {value:?}"));
            }
        }
        Ok(None)
    };
    let required = |name: &str| -> Result<u64, String> {
        field(name)?.ok_or_else(|| format!("cpu.stat has no {name} field"))
    };
    Ok(CpuStat {
        usage_usec: required("usage_usec")?,
        user_usec: required("user_usec")?,
        system_usec: required("system_usec")?,
        nr_throttled: field("nr_throttled")?.unwrap_or(0),
        throttled_usec: field("throttled_usec")?.unwrap_or(0),
    })
}

/// CPU consumed over a window, in the units the summary reports.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CpuUsage {
    pub cpu_s: f64,
    pub user_s: f64,
    pub system_s: f64,
    pub nr_throttled: u64,
    pub throttled_s: f64,
    /// Samples taken over the whole run (a sanity figure, not the window).
    pub samples: usize,
}

/// The value of one counter at `at_ms`, linearly interpolated between the
/// two samples that bracket it. `None` when `at_ms` lies outside the
/// sampled range (sampling started late or stopped early).
fn counter_at(samples: &[Sample], at_ms: u64, pick: impl Fn(&CpuStat) -> u64) -> Option<f64> {
    let first = samples.first()?;
    let last = samples.last()?;
    if at_ms < first.unix_ms || at_ms > last.unix_ms {
        return None;
    }
    // First sample at or after `at_ms`; the one before it is the lower bracket.
    let hi_idx = samples.partition_point(|s| s.unix_ms < at_ms);
    let hi = &samples[hi_idx];
    if hi.unix_ms == at_ms || hi_idx == 0 {
        return Some(pick(&hi.stat) as f64);
    }
    let lo = &samples[hi_idx - 1];
    let fraction = (at_ms - lo.unix_ms) as f64 / (hi.unix_ms - lo.unix_ms) as f64;
    let (a, b) = (pick(&lo.stat) as f64, pick(&hi.stat) as f64);
    Some(a + (b - a) * fraction)
}

/// CPU used between `start_ms` and `end_ms` (unix milliseconds). `None`
/// when the samples do not cover the whole window.
pub fn window_usage(samples: &[Sample], start_ms: u64, end_ms: u64) -> Option<CpuUsage> {
    if end_ms < start_ms {
        return None;
    }
    let delta = |pick: fn(&CpuStat) -> u64| -> Option<f64> {
        let start = counter_at(samples, start_ms, pick)?;
        let end = counter_at(samples, end_ms, pick)?;
        Some((end - start).max(0.0))
    };
    Some(CpuUsage {
        cpu_s: delta(|s| s.usage_usec)? / 1e6,
        user_s: delta(|s| s.user_usec)? / 1e6,
        system_s: delta(|s| s.system_usec)? / 1e6,
        nr_throttled: delta(|s| s.nr_throttled)?.round() as u64,
        throttled_s: delta(|s| s.throttled_usec)? / 1e6,
        samples: samples.len(),
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

async fn read_sample(path: &Path) -> Option<Sample> {
    let timestamp = now_ms();
    let text = tokio::fs::read_to_string(path).await.ok()?;
    Some(Sample {
        unix_ms: timestamp,
        stat: parse_cpu_stat(&text).ok()?,
    })
}

/// A background task sampling `cpu.stat` every `interval` until stopped.
pub struct Sampler {
    stop: oneshot::Sender<()>,
    task: JoinHandle<Vec<Sample>>,
}

impl Sampler {
    /// Take a first reading now (so the run is bracketed from its very
    /// start) and keep sampling in the background.
    pub fn start(path: PathBuf, interval: Duration) -> Self {
        let (stop, mut stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut samples = Vec::new();
            let mut ticker = tokio::time::interval(interval);
            loop {
                // The first tick completes immediately.
                tokio::select! {
                    _ = ticker.tick() => samples.extend(read_sample(&path).await),
                    _ = &mut stopped => break,
                }
            }
            // One last reading so the end of the run is bracketed too.
            samples.extend(read_sample(&path).await);
            samples
        });
        Self { stop, task }
    }

    /// Stop sampling and return every reading, oldest first.
    pub async fn stop(self) -> Vec<Sample> {
        let _ = self.stop.send(());
        self.task.await.unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CPU_STAT: &str = "usage_usec 3000000\nuser_usec 2000000\nsystem_usec 1000000\n\
        nice_usec 0\nnr_periods 100\nnr_throttled 4\nthrottled_usec 250000\n\
        nr_bursts 0\nburst_usec 0\n";

    fn sample(unix_ms: u64, usage_usec: u64, nr_throttled: u64) -> Sample {
        Sample {
            unix_ms,
            stat: CpuStat {
                usage_usec,
                user_usec: usage_usec / 2,
                system_usec: usage_usec / 2,
                nr_throttled,
                throttled_usec: nr_throttled * 1000,
            },
        }
    }

    #[test]
    fn parses_a_full_cpu_stat() {
        let stat = parse_cpu_stat(CPU_STAT).unwrap();
        assert_eq!(
            stat,
            CpuStat {
                usage_usec: 3_000_000,
                user_usec: 2_000_000,
                system_usec: 1_000_000,
                nr_throttled: 4,
                throttled_usec: 250_000,
            }
        );
    }

    #[test]
    fn throttling_fields_default_to_zero_without_the_cpu_controller() {
        let stat = parse_cpu_stat("usage_usec 5\nuser_usec 3\nsystem_usec 2\n").unwrap();
        assert_eq!((stat.nr_throttled, stat.throttled_usec), (0, 0));
    }

    #[test]
    fn rejects_missing_and_malformed_fields() {
        assert!(parse_cpu_stat("user_usec 3\nsystem_usec 2\n")
            .unwrap_err()
            .contains("usage_usec"));
        assert!(
            parse_cpu_stat("usage_usec abc\nuser_usec 3\nsystem_usec 2\n")
                .unwrap_err()
                .contains("not an integer")
        );
        assert!(parse_cpu_stat("usage_usec\n")
            .unwrap_err()
            .contains("no value"));
        assert!(parse_cpu_stat("").is_err());
    }

    #[test]
    fn window_usage_is_the_delta_between_exact_samples() {
        let samples = [
            sample(1_000, 1_000_000, 0),
            sample(2_000, 2_500_000, 1),
            sample(3_000, 4_000_000, 3),
        ];
        let usage = window_usage(&samples, 1_000, 3_000).unwrap();
        assert!((usage.cpu_s - 3.0).abs() < 1e-9);
        assert!((usage.user_s - 1.5).abs() < 1e-9);
        assert!((usage.system_s - 1.5).abs() < 1e-9);
        assert_eq!(usage.nr_throttled, 3);
        assert!((usage.throttled_s - 0.003).abs() < 1e-9);
        assert_eq!(usage.samples, 3);
    }

    #[test]
    fn window_edges_are_interpolated_between_samples() {
        // 1.5 CPU-seconds per wall second between 1_000 and 2_000.
        let samples = [sample(1_000, 0, 0), sample(2_000, 1_500_000, 0)];
        let usage = window_usage(&samples, 1_500, 2_000).unwrap();
        assert!((usage.cpu_s - 0.75).abs() < 1e-9);
        let usage = window_usage(&samples, 1_250, 1_750).unwrap();
        assert!((usage.cpu_s - 0.75).abs() < 1e-9);
    }

    #[test]
    fn window_outside_the_sampled_range_is_unknown() {
        let samples = [sample(1_000, 0, 0), sample(2_000, 10, 0)];
        assert!(window_usage(&samples, 500, 1_500).is_none());
        assert!(window_usage(&samples, 1_500, 2_500).is_none());
        assert!(window_usage(&[], 0, 1).is_none());
        // An inverted window is a caller bug, not a negative usage.
        assert!(window_usage(&samples, 1_800, 1_200).is_none());
    }

    #[test]
    fn a_counter_that_went_backwards_never_reports_negative_cpu() {
        // A restarted container resets its cgroup counters.
        let samples = [sample(1_000, 9_000_000, 5), sample(2_000, 1_000, 0)];
        let usage = window_usage(&samples, 1_000, 2_000).unwrap();
        assert_eq!((usage.cpu_s, usage.nr_throttled), (0.0, 0));
    }

    #[tokio::test]
    async fn the_sampler_collects_readings_and_a_final_one() {
        let dir = std::env::temp_dir().join(format!("perf-cpu-sampler-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cpu.stat");
        std::fs::write(&path, CPU_STAT).unwrap();

        let sampler = Sampler::start(path.clone(), Duration::from_millis(20));
        tokio::time::sleep(Duration::from_millis(110)).await;
        let samples = sampler.stop().await;

        assert!(samples.len() >= 3, "got {} samples", samples.len());
        assert!(samples.windows(2).all(|w| w[0].unix_ms <= w[1].unix_ms));
        assert_eq!(samples[0].stat.usage_usec, 3_000_000);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn an_unreadable_file_yields_no_samples() {
        let sampler = Sampler::start(
            PathBuf::from("/nonexistent/cpu.stat"),
            Duration::from_millis(5),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(sampler.stop().await.is_empty());
    }
}
