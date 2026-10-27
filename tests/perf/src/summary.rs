//! The machine-readable run summary (`NFR-0030`; schema documented in this
//! crate's `README.md`). Pure functions over plain data -- `main.rs` maps
//! goose's metrics onto these types -- so the arithmetic is unit-testable
//! without a load test. Field names carry their unit and are stable:
//! level-2 A/B comparisons read these files across commits.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::cpu::CpuUsage;

/// Bumped on any rename, removal or change of meaning of a field.
pub const SCHEMA_VERSION: u32 = 1;

/// One goose phase, reduced to what the window needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Increasing,
    Maintaining,
    Other,
}

/// The measured window: from "all users running" (goose resets its
/// metrics then, which is also where warm-up ends) to the end of the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub start_ms: u64,
    pub end_ms: u64,
    pub warmup_ms: u64,
}

impl Window {
    pub fn duration_s(&self) -> f64 {
        (self.end_ms - self.start_ms) as f64 / 1000.0
    }

    /// Derive the window from goose's step history (`(phase, unix_ms)`,
    /// oldest first, the last entry being the finish). Without a
    /// `Maintaining` step (an `--iterations` run, say) the whole run counts.
    pub fn from_history(history: &[(Phase, u64)]) -> Option<Self> {
        let first = history.first()?.1;
        let last = history.last()?.1;
        let start = history
            .iter()
            .find(|(phase, _)| *phase == Phase::Maintaining)
            .map_or(first, |(_, ms)| *ms);
        Some(Self {
            start_ms: start.min(last),
            end_ms: last,
            warmup_ms: start.saturating_sub(first),
        })
    }
}

/// Per method+path request statistics for the measured window.
#[derive(Clone, Debug, Default)]
pub struct EndpointStats {
    pub method: String,
    pub path: String,
    pub success: u64,
    pub failed: u64,
    /// Response time (ms, goose's rounded buckets) -> request count.
    pub times: BTreeMap<u64, u64>,
    pub max_ms: u64,
    pub total_ms: u64,
}

impl EndpointStats {
    pub fn requests(&self) -> u64 {
        self.success + self.failed
    }
}

/// What was run, and under which caps.
#[derive(Clone, Debug)]
pub struct RunInfo {
    pub scenario: String,
    pub mode: String,
    pub users: u64,
    pub git_sha: String,
    pub image_tag: String,
    pub cpu_limit_cores: Option<f64>,
    pub memory_limit_mib: Option<f64>,
    pub cpu_sample_interval_ms: u64,
}

/// Nearest-rank percentile (`q` in 0..=1) of a response-time histogram.
pub fn percentile_ms(times: &BTreeMap<u64, u64>, q: f64) -> Option<u64> {
    let total: u64 = times.values().sum();
    if total == 0 {
        return None;
    }
    let rank = ((q * total as f64).ceil() as u64).clamp(1, total);
    let mut seen = 0;
    for (ms, count) in times {
        seen += count;
        if seen >= rank {
            return Some(*ms);
        }
    }
    None
}

/// Docker-style memory size (`2g`, `512m`, `1536M`, `2048k`, `1073741824`)
/// in MiB.
pub fn parse_memory_mib(text: &str) -> Option<f64> {
    let text = text.trim().to_ascii_lowercase();
    let (digits, factor) = match text.chars().last()? {
        'b' => (&text[..text.len() - 1], 1.0),
        'k' => (&text[..text.len() - 1], 1024.0),
        'm' => (&text[..text.len() - 1], 1024.0 * 1024.0),
        'g' => (&text[..text.len() - 1], 1024.0 * 1024.0 * 1024.0),
        _ => (text.as_str(), 1.0),
    };
    let bytes = digits.trim().parse::<f64>().ok()? * factor;
    (bytes > 0.0).then_some(bytes / (1024.0 * 1024.0))
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// Latency block shared by the overall figures and each endpoint.
fn latency(times: &BTreeMap<u64, u64>) -> Value {
    json!({
        "p50_ms": percentile_ms(times, 0.50),
        "p95_ms": percentile_ms(times, 0.95),
        "p99_ms": percentile_ms(times, 0.99),
    })
}

/// Assemble the summary document.
pub fn build_summary(
    run: &RunInfo,
    window: &Window,
    endpoints: &[EndpointStats],
    cpu: Option<&CpuUsage>,
) -> Value {
    let success: u64 = endpoints.iter().map(|e| e.success).sum();
    let failed: u64 = endpoints.iter().map(|e| e.failed).sum();
    let total = success + failed;
    let duration_s = window.duration_s();
    let mut times: BTreeMap<u64, u64> = BTreeMap::new();
    for endpoint in endpoints {
        for (ms, count) in &endpoint.times {
            *times.entry(*ms).or_default() += count;
        }
    }
    let total_ms: u64 = endpoints.iter().map(|e| e.total_ms).sum();
    let max_ms = endpoints.iter().map(|e| e.max_ms).max().unwrap_or(0);

    let per_second = |count: u64| (duration_s > 0.0).then(|| round3(count as f64 / duration_s));
    let cpu_json = cpu.map(|c| {
        json!({
            "cpu_s": round3(c.cpu_s),
            "user_s": round3(c.user_s),
            "system_s": round3(c.system_s),
            "cpu_ms_per_request": (success > 0).then(|| round3(c.cpu_s * 1000.0 / success as f64)),
            "avg_cores": (duration_s > 0.0).then(|| round3(c.cpu_s / duration_s)),
            "nr_throttled": c.nr_throttled,
            "throttled_s": round3(c.throttled_s),
            "samples": c.samples,
            "sample_interval_ms": run.cpu_sample_interval_ms,
        })
    });

    let mut sorted: Vec<&EndpointStats> = endpoints.iter().collect();
    sorted.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));
    let endpoint_json: Vec<Value> = sorted
        .iter()
        .map(|e| {
            let mut value = json!({
                "method": e.method,
                "path": e.path,
                "requests": e.requests(),
                "failed": e.failed,
            });
            value["latency"] = latency(&e.times);
            value
        })
        .collect();

    let mut summary = json!({
        "schema_version": SCHEMA_VERSION,
        "scenario": run.scenario,
        "mode": run.mode,
        "git_sha": run.git_sha,
        "image_tag": run.image_tag,
        "users": run.users,
        "warmup_s": round3(window.warmup_ms as f64 / 1000.0),
        "duration_s": round3(duration_s),
        "requests": total,
        "requests_success": success,
        "errors": failed,
        "error_rate": (total > 0).then(|| failed as f64 / total as f64),
        "rps": per_second(total),
        "mean_ms": (total > 0).then(|| round3(total_ms as f64 / total as f64)),
        "max_ms": max_ms,
        "cpu": cpu_json,
        "limits": {
            "cpu_limit_cores": run.cpu_limit_cores,
            "memory_limit_mib": run.memory_limit_mib,
        },
        "endpoints": endpoint_json,
    });
    for (key, value) in latency(&times).as_object().into_iter().flatten() {
        summary[key] = value.clone();
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn histogram(pairs: &[(u64, u64)]) -> BTreeMap<u64, u64> {
        pairs.iter().copied().collect()
    }

    fn run() -> RunInfo {
        RunInfo {
            scenario: "HeroJourneyReal".into(),
            mode: "real".into(),
            users: 25,
            git_sha: "abc123".into(),
            image_tag: "template-axum:x".into(),
            cpu_limit_cores: Some(1.0),
            memory_limit_mib: Some(2048.0),
            cpu_sample_interval_ms: 500,
        }
    }

    fn endpoint(path: &str, success: u64, failed: u64, pairs: &[(u64, u64)]) -> EndpointStats {
        EndpointStats {
            method: "GET".into(),
            path: path.into(),
            success,
            failed,
            times: histogram(pairs),
            max_ms: pairs.last().map_or(0, |p| p.0),
            total_ms: pairs.iter().map(|(ms, n)| ms * n).sum(),
        }
    }

    #[test]
    fn percentiles_are_nearest_rank() {
        let times = histogram(&[(1, 90), (10, 5), (100, 4), (900, 1)]);
        assert_eq!(percentile_ms(&times, 0.50), Some(1));
        assert_eq!(percentile_ms(&times, 0.90), Some(1));
        assert_eq!(percentile_ms(&times, 0.95), Some(10));
        assert_eq!(percentile_ms(&times, 0.99), Some(100));
        assert_eq!(percentile_ms(&times, 1.0), Some(900));
        assert_eq!(percentile_ms(&times, 0.0), Some(1));
        assert_eq!(percentile_ms(&BTreeMap::new(), 0.5), None);
    }

    #[test]
    fn memory_sizes_parse_to_mib() {
        assert_eq!(parse_memory_mib("2g"), Some(2048.0));
        assert_eq!(parse_memory_mib("512M"), Some(512.0));
        assert_eq!(parse_memory_mib("2048k"), Some(2.0));
        assert_eq!(parse_memory_mib("1048576"), Some(1.0));
        assert_eq!(parse_memory_mib("1048576b"), Some(1.0));
        assert_eq!(parse_memory_mib("0"), None);
        assert_eq!(parse_memory_mib("lots"), None);
        assert_eq!(parse_memory_mib(""), None);
    }

    #[test]
    fn window_starts_when_all_users_run() {
        let history = [
            (Phase::Increasing, 1_000),
            (Phase::Maintaining, 6_000),
            (Phase::Other, 36_000),
            (Phase::Other, 36_100),
        ];
        let window = Window::from_history(&history).unwrap();
        assert_eq!(
            window,
            Window {
                start_ms: 6_000,
                end_ms: 36_100,
                warmup_ms: 5_000
            }
        );
        assert!((window.duration_s() - 30.1).abs() < 1e-9);
    }

    #[test]
    fn without_a_maintaining_step_the_whole_run_is_the_window() {
        let window =
            Window::from_history(&[(Phase::Increasing, 1_000), (Phase::Other, 4_000)]).unwrap();
        assert_eq!(
            (window.start_ms, window.end_ms, window.warmup_ms),
            (1_000, 4_000, 0)
        );
        assert!(Window::from_history(&[]).is_none());
    }

    #[test]
    fn summary_has_the_documented_fields() {
        let endpoints = [
            endpoint("/a", 90, 0, &[(2, 60), (4, 30)]),
            endpoint("/b", 8, 2, &[(10, 9), (200, 1)]),
        ];
        let window = Window {
            start_ms: 0,
            end_ms: 10_000,
            warmup_ms: 5_000,
        };
        let cpu = CpuUsage {
            cpu_s: 4.9,
            user_s: 3.0,
            system_s: 1.9,
            nr_throttled: 7,
            throttled_s: 0.25,
            samples: 21,
        };
        let s = build_summary(&run(), &window, &endpoints, Some(&cpu));

        assert_eq!(s["schema_version"], 1);
        assert_eq!(s["scenario"], "HeroJourneyReal");
        assert_eq!(s["mode"], "real");
        assert_eq!(s["users"], 25);
        assert_eq!(s["warmup_s"], 5.0);
        assert_eq!(s["duration_s"], 10.0);
        assert_eq!(s["requests"], 100);
        assert_eq!(s["requests_success"], 98);
        assert_eq!(s["errors"], 2);
        assert_eq!(s["error_rate"], 0.02);
        assert_eq!(s["rps"], 10.0);
        assert_eq!(s["p50_ms"], 2);
        assert_eq!(s["p95_ms"], 10);
        assert_eq!(s["p99_ms"], 10);
        assert_eq!(s["max_ms"], 200);
        assert_eq!(s["cpu"]["cpu_s"], 4.9);
        assert_eq!(s["cpu"]["cpu_ms_per_request"], 50.0);
        assert_eq!(s["cpu"]["avg_cores"], 0.49);
        assert_eq!(s["cpu"]["nr_throttled"], 7);
        assert_eq!(s["cpu"]["throttled_s"], 0.25);
        assert_eq!(s["limits"]["cpu_limit_cores"], 1.0);
        assert_eq!(s["limits"]["memory_limit_mib"], 2048.0);
        assert_eq!(s["git_sha"], "abc123");
        assert_eq!(s["image_tag"], "template-axum:x");
        assert_eq!(s["endpoints"][0]["path"], "/a");
        assert_eq!(s["endpoints"][1]["failed"], 2);
        assert_eq!(s["endpoints"][1]["latency"]["p95_ms"], 200);
    }

    #[test]
    fn missing_cpu_and_empty_runs_serialize_as_null() {
        let window = Window {
            start_ms: 0,
            end_ms: 0,
            warmup_ms: 0,
        };
        let mut info = run();
        info.cpu_limit_cores = None;
        info.memory_limit_mib = None;
        let s = build_summary(&info, &window, &[], None);
        assert!(s["cpu"].is_null());
        assert!(s["rps"].is_null());
        assert!(s["error_rate"].is_null());
        assert!(s["p95_ms"].is_null());
        assert!(s["mean_ms"].is_null());
        assert!(s["limits"]["cpu_limit_cores"].is_null());
        assert_eq!(s["requests"], 0);
    }

    #[test]
    fn cpu_per_request_is_null_when_nothing_succeeded() {
        let endpoints = [endpoint("/a", 0, 5, &[(3, 5)])];
        let window = Window {
            start_ms: 0,
            end_ms: 1_000,
            warmup_ms: 0,
        };
        let cpu = CpuUsage {
            cpu_s: 1.0,
            user_s: 1.0,
            system_s: 0.0,
            nr_throttled: 0,
            throttled_s: 0.0,
            samples: 3,
        };
        let s = build_summary(&run(), &window, &endpoints, Some(&cpu));
        assert!(s["cpu"]["cpu_ms_per_request"].is_null());
        assert_eq!(s["error_rate"], 1.0);
    }
}
