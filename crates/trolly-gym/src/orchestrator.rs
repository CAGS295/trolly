//! Local weekday working-hours schedule for the GPU training orchestrator.
//!
//! Independent of libtorch so default `cargo test -p trolly-gym` covers the
//! window logic. The `gpu_train_orchestrator` binary (torch feature) fulfills
//! this schedule by running the existing WoLF-PPO training sessions on GPU.
//!
//! Job choice is directed by [`BROAD_GOAL`]: prefer stream-shaped
//! microstructure checkpoints (closer to live/demo trading) over matrix-game
//! NES drills when those are the open gap. Scheduling stays weekday
//! working hours; the goal does not replace the window.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// ISO weekday: Monday = 1 … Sunday = 7.
pub type IsoWeekday = u8;

/// Minutes since local midnight.
pub type Minutes = u16;

/// Durable north-star shared with the Daily workplan orchestrator (`WORKPLAN.md`).
///
/// Close the loop from GPU-trained WoLF-PPO policies to demo/live Binance
/// trading: a checkpointed policy consumes trolly-stream observations, selects
/// actions through trolly-strategy, and places/reconciles orders via the exec
/// crates. Matrix-game NES validation is a regression gate, not the destination.
pub const BROAD_GOAL: &str = "Close the loop from GPU-trained WoLF-PPO policies to \
demo/live Binance trading via trolly-stream observations, trolly-strategy actions, \
and exec-crate order placement/reconcile.";

/// Sidecar written after each GPU training window (evidence of daily progress).
pub const PROGRESS_LOG_NAME: &str = "progress.json";

/// Checkpoint filename used by the train/game sessions.
pub const LATEST_CHECKPOINT_NAME: &str = "latest.safetensors";

/// Microstructure completion marker (`train::microstructure_completion`).
pub const COMPLETED_MARKER_NAME: &str = "completed.json";

/// Jobs selected for this window and why they are the highest-leverage slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainGap {
    pub jobs: Vec<String>,
    pub reason: String,
}

/// Prompt fragment for “continue training locally” (daily orchestrator + `--continue`).
pub const CONTINUE_TRAINING_LOCALLY: &str = "\
When asked to continue training locally: (1) git fetch origin then rebase \
onto @{u} if the working tree is clean (fetch+warn and continue if rebase \
would destroy uncommitted work; never reset --hard, never rebase -i); \
(2) assess state vs BROAD_GOAL; \
(3) ensure ClickHouse is reachable and ingest sim ticks into trolly.ticks \
(bail if the DB is down; no in-memory tape); \
(4) train a time-boxed WoLF-PPO slice (gpu_train_orchestrator --continue, \
weekday window for --once); (5) write latest.safetensors plus \
latest.fingerprint.json (weights/data-window/config hashes); \
(6) leave evidence in progress.json — not a no-op health check.";

/// ClickHouse + tick tape prepared for a local training slice.
#[derive(Debug, Clone)]
pub struct LocalTrainPrep {
    pub session_id: String,
    pub ticks_written: usize,
    pub data_window: String,
    pub clickhouse_ok: bool,
    pub tape: Option<crate::ticks::TickTape>,
    pub note: String,
}

/// Fetch `origin` and rebase onto the tracked upstream (`@{u}`) so a local
/// train slice uses what cloud agents already pushed.
///
/// Skips rebase when the working tree has uncommitted changes (fetch + warn
/// only). Never resets, never interactive rebase. Not called from unit tests.
pub fn sync_local_git_upstream() -> String {
    let inside = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output();
    match inside {
        Ok(o) if o.status.success() => {}
        _ => return "warn: not a git work tree; skipping fetch".into(),
    }

    match Command::new("git").args(["fetch", "origin"]).output() {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            return format!(
                "warn: git fetch origin failed ({}); continuing with local tree",
                String::from_utf8_lossy(&o.stderr).trim()
            );
        }
        Err(err) => {
            return format!("warn: git fetch origin failed ({err}); continuing with local tree");
        }
    }

    let dirty = Command::new("git")
        .args(["diff", "--quiet"])
        .status()
        .map(|s| !s.success())
        .unwrap_or(true)
        || Command::new("git")
            .args(["diff", "--cached", "--quiet"])
            .status()
            .map(|s| !s.success())
            .unwrap_or(true);
    if dirty {
        return "warn: uncommitted changes; fetched but skipping rebase so local work is not destroyed"
            .into();
    }

    let has_upstream = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !has_upstream {
        return "warn: no upstream tracking branch; fetched but skipping rebase".into();
    }

    match Command::new("git").args(["rebase", "@{u}"]).output() {
        Ok(o) if o.status.success() => "fetched origin; rebased onto @{u}".into(),
        Ok(o) => {
            let _ = Command::new("git").args(["rebase", "--abort"]).status();
            format!(
                "warn: git rebase @{{u}} failed ({}); aborted rebase and continuing",
                String::from_utf8_lossy(&o.stderr).trim()
            )
        }
        Err(err) => {
            let _ = Command::new("git").args(["rebase", "--abort"]).status();
            format!("warn: git rebase @{{u}} failed ({err}); continuing with local tree")
        }
    }
}

/// Start (or reuse) ClickHouse, write sim ticks, return a training tape.
///
/// Fails if ClickHouse cannot be reached after a docker-compose attempt, or
/// if the insert does not land. The weekday trainer must not fall back to
/// an in-memory tape.
pub fn prepare_local_training() -> Result<LocalTrainPrep, String> {
    let session_id = format!("sim-SYNTHUSDT-{}", now_session_stamp());
    let rows = crate::sim::ingest_sim_ticks(
        &crate::sim::MicrostructureConfig::default(),
        &session_id,
        64,
    );
    let tape = crate::ticks::TickTape::new(rows.clone());
    let ch = crate::ticks::ensure_local_clickhouse().map_err(|err| {
        crate::ticks::clickhouse_unreachable_error(
            &crate::ticks::ClickHouseTicks::default().url,
            err,
        )
    })?;
    let n = ch.insert(&rows).map_err(|err| {
        format!(
            "training bails: ClickHouse is reachable at {} but insert failed ({err})",
            ch.url
        )
    })?;
    // Fulfill: train from the same rows the DB just stored.
    let tape = match ch.query_session(&session_id) {
        Ok(loaded) if !loaded.is_empty() => crate::ticks::TickTape::new(loaded),
        _ => tape,
    };
    let data_window = tape.window_hash();
    Ok(LocalTrainPrep {
        session_id: session_id.clone(),
        ticks_written: n,
        data_window,
        clickhouse_ok: true,
        tape: Some(tape),
        note: format!("inserted+reloaded {n} ticks session={session_id}"),
    })
}

fn now_session_stamp() -> String {
    Command::new("date")
        .args(["+%Y%m%dT%H%M%S"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "local".into())
}

/// Assess checkpoint tree vs [`BROAD_GOAL`] and pick today's training jobs.
///
/// `override_jobs` is `TROLLY_TRAIN_JOBS` (`matrix`, `microstructure`, comma-separated).
/// When unset, missing or incomplete stream-shaped (microstructure) checkpoints
/// outrank matrix NES drills. Always returns at least one job.
pub fn select_train_jobs(checkpoint_root: &Path, override_jobs: Option<&str>) -> TrainGap {
    if let Some(raw) = override_jobs {
        let jobs = parse_job_list(raw);
        if !jobs.is_empty() {
            return TrainGap {
                jobs,
                reason: "TROLLY_TRAIN_JOBS override".into(),
            };
        }
    }

    let micro_complete = microstructure_complete(checkpoint_root);
    let micro_started = microstructure_started(checkpoint_root);
    let matrix_started = matrix_started(checkpoint_root);

    if !micro_complete {
        if !micro_started {
            return TrainGap {
                jobs: vec!["microstructure".into(), "matrix".into()],
                reason: "no stream-shaped checkpoints; train microstructure first \
                         (closest to live policy), then matrix NES gate"
                    .into(),
            };
        }
        return TrainGap {
            jobs: vec!["microstructure".into()],
            reason: "microstructure policy not complete; highest-leverage gap \
                     toward stream-backed trading"
                .into(),
        };
    }
    if !matrix_started {
        return TrainGap {
            jobs: vec!["matrix".into()],
            reason: "stream-shaped models complete; improve matrix NES regression gate".into(),
        };
    }
    TrainGap {
        jobs: vec!["microstructure".into(), "matrix".into()],
        reason: "both tracks exist; keep improving stream-shaped checkpoints \
                 toward a live/demo policy"
            .into(),
    }
}

fn parse_job_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

fn microstructure_started(root: &Path) -> bool {
    ["mlp", "liquid"].iter().any(|arch| {
        root.join("microstructure")
            .join(arch)
            .join(LATEST_CHECKPOINT_NAME)
            .exists()
    })
}

fn microstructure_complete(root: &Path) -> bool {
    ["mlp", "liquid"].iter().all(|arch| {
        root.join("microstructure")
            .join(arch)
            .join(COMPLETED_MARKER_NAME)
            .exists()
    })
}

fn matrix_started(root: &Path) -> bool {
    let games = ["matching_pennies_weighted", "rps_weighted"];
    ["mlp", "liquid"].iter().any(|arch| {
        games.iter().any(|game| {
            root.join("matrix")
                .join(arch)
                .join(game)
                .join(LATEST_CHECKPOINT_NAME)
                .exists()
        })
    })
}

/// Write `progress.json` under `checkpoint_root` (daily evidence; not a no-op probe).
pub fn write_progress_log(
    checkpoint_root: &Path,
    date: &str,
    gap: &TrainGap,
    extras: &[(&str, &str)],
) -> std::io::Result<PathBuf> {
    fs::create_dir_all(checkpoint_root)?;
    let path = checkpoint_root.join(PROGRESS_LOG_NAME);
    let jobs = gap
        .jobs
        .iter()
        .map(|j| format!("\"{j}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let mut extra_json = String::new();
    for (k, v) in extras {
        extra_json.push_str(&format!(
            ",\n  \"{}\": {}",
            escape_json(k),
            json_string_or_raw(v)
        ));
    }
    let body = format!(
        "{{\n  \"goal\": \"{}\",\n  \"date\": \"{}\",\n  \"jobs\": [{jobs}],\n  \"reason\": \"{}\"{extra_json}\n}}\n",
        escape_json(BROAD_GOAL),
        escape_json(date),
        escape_json(&gap.reason),
    );
    fs::write(&path, body)?;
    Ok(path)
}

fn escape_json(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('"', "\\\"")
}

fn json_string_or_raw(raw: &str) -> String {
    let t = raw.trim();
    if t.starts_with('{')
        || t.starts_with('[')
        || t.parse::<f64>().is_ok()
        || t == "true"
        || t == "false"
    {
        t.to_string()
    } else {
        format!("\"{}\"", escape_json(raw))
    }
}

/// Compute device requested by the orchestrator (fulfilled by [`crate::device`]
/// when the `torch` feature is on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceSpec {
    /// Use GPU when libtorch reports one, otherwise CPU (examples/tests).
    /// The GPU orchestrator refuses CPU and non-RX HIP devices.
    Auto,
    Cpu,
    /// Libtorch CUDA index. ROCm/HIP builds expose HIP devices through this API.
    Cuda(usize),
}

impl DeviceSpec {
    /// Parse `TROLLY_TRAIN_DEVICE` (`auto`, `cpu`, `cuda`, `cuda:N`, `rocm`, `gpu`).
    pub fn from_env() -> Self {
        match std::env::var("TROLLY_TRAIN_DEVICE") {
            Ok(raw) => Self::parse(&raw).unwrap_or(Self::Auto),
            Err(_) => Self::Auto,
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let s = raw.trim().to_ascii_lowercase();
        match s.as_str() {
            "" | "auto" => Some(Self::Auto),
            "cpu" => Some(Self::Cpu),
            "cuda" | "gpu" | "rocm" | "hip" => Some(Self::Cuda(0)),
            _ => s
                .strip_prefix("cuda:")
                .and_then(|n| n.parse().ok().map(Self::Cuda)),
        }
    }
}

/// Default gap between `latest.safetensors` writes on a long GPU slice.
pub const DEFAULT_CHECKPOINT_INTERVAL_SECS: u64 = 60;

/// Parse `TROLLY_CHECKPOINT_INTERVAL_SECS`. Empty/invalid → 60s. `0` means
/// persist after every update (old behavior).
pub fn parse_checkpoint_interval_secs(raw: Option<&str>) -> Duration {
    let secs = match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => DEFAULT_CHECKPOINT_INTERVAL_SECS,
        Some(s) => s.parse().unwrap_or(DEFAULT_CHECKPOINT_INTERVAL_SECS),
    };
    Duration::from_secs(secs)
}

pub fn checkpoint_interval_from_env() -> Duration {
    let owned = std::env::var("TROLLY_CHECKPOINT_INTERVAL_SECS").ok();
    parse_checkpoint_interval_secs(owned.as_deref())
}

/// Wall-clock gate for overwriting `latest.safetensors` during a train slice.
///
/// Starts counting when constructed. Interval `0` is always due. The weekday
/// trainer also flushes once when a slice ends, so a SIGTERM loses at most
/// one interval of work.
#[derive(Debug, Clone)]
pub struct CheckpointClock {
    pub interval: Duration,
    started: Instant,
    last_save: Option<Instant>,
}

impl CheckpointClock {
    pub fn from_env() -> Self {
        Self::new(checkpoint_interval_from_env())
    }

    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            started: Instant::now(),
            last_save: None,
        }
    }

    pub fn due(&self) -> bool {
        if self.interval.is_zero() {
            return true;
        }
        self.last_save.unwrap_or(self.started).elapsed() >= self.interval
    }

    pub fn mark(&mut self) {
        self.last_save = Some(Instant::now());
    }
}

impl fmt::Display for DeviceSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::Cpu => write!(f, "cpu"),
            Self::Cuda(i) => write!(f, "cuda:{i}"),
        }
    }
}

/// Local weekday working-hours window.
///
/// Defaults: Monday–Friday 09:00–17:00 local time. Override with
/// `TROLLY_WORK_START`, `TROLLY_WORK_END` (`HH:MM`), and
/// `TROLLY_WORK_WEEKDAYS` (`1-5` or `1,2,3,4,5`; Monday = 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingHours {
    pub start_minutes: Minutes,
    pub end_minutes: Minutes,
    /// Index 1..=7; index 0 unused.
    pub weekdays: [bool; 8],
}

impl Default for WorkingHours {
    fn default() -> Self {
        let mut weekdays = [false; 8];
        for day in 1..=5 {
            weekdays[day] = true;
        }
        Self {
            start_minutes: 9 * 60,
            end_minutes: 17 * 60,
            weekdays,
        }
    }
}

impl WorkingHours {
    pub fn from_env() -> Self {
        let mut hours = Self::default();
        if let Ok(raw) = std::env::var("TROLLY_WORK_START") {
            if let Some(minutes) = parse_hhmm(&raw) {
                hours.start_minutes = minutes;
            }
        }
        if let Ok(raw) = std::env::var("TROLLY_WORK_END") {
            if let Some(minutes) = parse_hhmm(&raw) {
                hours.end_minutes = minutes;
            }
        }
        if let Ok(raw) = std::env::var("TROLLY_WORK_WEEKDAYS") {
            if let Some(weekdays) = parse_weekdays(&raw) {
                hours.weekdays = weekdays;
            }
        }
        hours
    }

    pub fn contains(&self, weekday: IsoWeekday, minutes: Minutes) -> bool {
        (1..=7).contains(&weekday)
            && self.weekdays[weekday as usize]
            && minutes >= self.start_minutes
            && minutes < self.end_minutes
    }

    /// Remaining seconds in the current window, if we are inside it.
    pub fn remaining_secs(&self, weekday: IsoWeekday, minutes: Minutes) -> Option<u64> {
        if !self.contains(weekday, minutes) {
            return None;
        }
        Some(u64::from(self.end_minutes.saturating_sub(minutes)) * 60)
    }

    /// Seconds until the next window opens. `0` if already inside.
    pub fn secs_until_next_window(&self, weekday: IsoWeekday, minutes: Minutes) -> u64 {
        if self.contains(weekday, minutes) {
            return 0;
        }
        for offset in 0..8u8 {
            let day = wrap_weekday(weekday, offset);
            if !self.weekdays[day as usize] {
                continue;
            }
            if offset == 0 {
                if minutes < self.start_minutes {
                    return u64::from(self.start_minutes - minutes) * 60;
                }
                continue;
            }
            let mins_left_today = 24 * 60 - minutes;
            let full_days = u16::from(offset - 1) * 24 * 60;
            return u64::from(mins_left_today + full_days + self.start_minutes) * 60;
        }
        24 * 3600
    }
}

/// Local ISO weekday and minutes since midnight (`date +%u %H %M`).
pub fn local_weekday_and_minutes() -> std::io::Result<(IsoWeekday, Minutes)> {
    let output = Command::new("date").args(["+%u %H %M"]).output()?;
    if !output.status.success() {
        return Err(std::io::Error::other("date +%u %H %M failed"));
    }
    parse_date_fields(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| std::io::Error::other("unrecognized date output"))
}

pub fn parse_hhmm(raw: &str) -> Option<Minutes> {
    let raw = raw.trim();
    if let Some((h, m)) = raw.split_once(':') {
        let hours: u16 = h.parse().ok()?;
        let mins: u16 = m.parse().ok()?;
        if hours < 24 && mins < 60 {
            return Some(hours * 60 + mins);
        }
        return None;
    }
    if raw.len() == 4 && raw.chars().all(|c| c.is_ascii_digit()) {
        let hours: u16 = raw[..2].parse().ok()?;
        let mins: u16 = raw[2..].parse().ok()?;
        if hours < 24 && mins < 60 {
            return Some(hours * 60 + mins);
        }
    }
    None
}

pub fn parse_weekdays(raw: &str) -> Option<[bool; 8]> {
    let mut weekdays = [false; 8];
    let raw = raw.trim();
    if let Some((start, end)) = raw.split_once('-') {
        let start: u8 = start.parse().ok()?;
        let end: u8 = end.parse().ok()?;
        if !(1..=7).contains(&start) || !(1..=7).contains(&end) || start > end {
            return None;
        }
        for day in start..=end {
            weekdays[day as usize] = true;
        }
        return Some(weekdays);
    }
    let mut any = false;
    for part in raw.split(',') {
        let day: u8 = part.trim().parse().ok()?;
        if !(1..=7).contains(&day) {
            return None;
        }
        weekdays[day as usize] = true;
        any = true;
    }
    any.then_some(weekdays)
}

fn wrap_weekday(weekday: IsoWeekday, offset: u8) -> IsoWeekday {
    let raw = weekday + offset;
    if raw > 7 {
        raw - 7
    } else {
        raw
    }
}

fn parse_date_fields(raw: &str) -> Option<(IsoWeekday, Minutes)> {
    let mut parts = raw.split_whitespace();
    let weekday: IsoWeekday = parts.next()?.parse().ok()?;
    let hour: u16 = parts.next()?.parse().ok()?;
    let minute: u16 = parts.next()?.parse().ok()?;
    if !(1..=7).contains(&weekday) || hour > 23 || minute > 59 {
        return None;
    }
    Some((weekday, hour * 60 + minute))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_window_is_weekday_nine_to_five() {
        let hours = WorkingHours::default();
        assert!(hours.contains(1, 9 * 60));
        assert!(hours.contains(5, 16 * 60 + 59));
        assert!(!hours.contains(5, 17 * 60));
        assert!(!hours.contains(6, 12 * 60));
        assert!(!hours.contains(1, 8 * 60 + 59));
    }

    #[test]
    fn remaining_secs_inside_window() {
        let hours = WorkingHours::default();
        assert_eq!(hours.remaining_secs(2, 16 * 60), Some(60 * 60));
        assert_eq!(hours.remaining_secs(6, 12 * 60), None);
    }

    #[test]
    fn wait_from_friday_evening_to_monday_morning() {
        let hours = WorkingHours::default();
        // Friday 18:00 → Monday 09:00 = 6h Fri + Sat + Sun + 9h Mon
        let secs = hours.secs_until_next_window(5, 18 * 60);
        assert_eq!(secs, (6 + 24 + 24 + 9) * 3600);
    }

    #[test]
    fn wait_same_morning_before_start() {
        let hours = WorkingHours::default();
        assert_eq!(hours.secs_until_next_window(3, 8 * 60), 3600);
        assert_eq!(hours.secs_until_next_window(3, 10 * 60), 0);
    }

    #[test]
    fn parse_device_and_hours() {
        assert_eq!(DeviceSpec::parse("auto"), Some(DeviceSpec::Auto));
        assert_eq!(DeviceSpec::parse("CUDA:1"), Some(DeviceSpec::Cuda(1)));
        assert_eq!(DeviceSpec::parse("rocm"), Some(DeviceSpec::Cuda(0)));
        assert_eq!(parse_hhmm("09:00"), Some(540));
        assert_eq!(parse_hhmm("1700"), Some(17 * 60));
        let days = parse_weekdays("1-5").unwrap();
        assert!(days[1] && days[5] && !days[6]);
        let days = parse_weekdays("1,3,5").unwrap();
        assert!(days[1] && days[3] && days[5] && !days[2]);
    }

    #[test]
    fn parse_date_fields_iso() {
        assert_eq!(parse_date_fields("4 20 16"), Some((4, 20 * 60 + 16)));
    }

    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("trolly-orch-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"").unwrap();
    }

    #[test]
    fn empty_tree_trains_microstructure_then_matrix() {
        let root = temp_root("empty");
        let gap = select_train_jobs(&root, None);
        assert_eq!(gap.jobs, ["microstructure", "matrix"]);
        assert!(gap.reason.contains("stream-shaped"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn incomplete_microstructure_focuses_budget() {
        let root = temp_root("micro-partial");
        touch(&root.join("microstructure/mlp").join(LATEST_CHECKPOINT_NAME));
        let gap = select_train_jobs(&root, None);
        assert_eq!(gap.jobs, ["microstructure"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn complete_microstructure_without_matrix_picks_nes_gate() {
        let root = temp_root("micro-done");
        touch(&root.join("microstructure/mlp").join(COMPLETED_MARKER_NAME));
        touch(
            &root
                .join("microstructure/liquid")
                .join(COMPLETED_MARKER_NAME),
        );
        let gap = select_train_jobs(&root, None);
        assert_eq!(gap.jobs, ["matrix"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn override_jobs_wins() {
        let root = temp_root("override");
        let gap = select_train_jobs(&root, Some("matrix"));
        assert_eq!(gap.jobs, ["matrix"]);
        assert_eq!(gap.reason, "TROLLY_TRAIN_JOBS override");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn progress_log_records_goal_and_jobs() {
        let root = temp_root("progress");
        let gap = TrainGap {
            jobs: vec!["microstructure".into()],
            reason: "test slice".into(),
        };
        let path = write_progress_log(&root, "2026-08-14", &gap, &[("nes_dist", "0.12")]).unwrap();
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains(BROAD_GOAL));
        assert!(body.contains("microstructure"));
        assert!(body.contains("2026-08-14"));
        assert!(body.contains("0.12"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn checkpoint_interval_defaults_and_zero() {
        assert_eq!(
            parse_checkpoint_interval_secs(None),
            Duration::from_secs(DEFAULT_CHECKPOINT_INTERVAL_SECS)
        );
        assert_eq!(
            parse_checkpoint_interval_secs(Some("")),
            Duration::from_secs(DEFAULT_CHECKPOINT_INTERVAL_SECS)
        );
        assert_eq!(
            parse_checkpoint_interval_secs(Some("bogus")),
            Duration::from_secs(DEFAULT_CHECKPOINT_INTERVAL_SECS)
        );
        assert_eq!(parse_checkpoint_interval_secs(Some("0")), Duration::ZERO);
        assert_eq!(
            parse_checkpoint_interval_secs(Some("120")),
            Duration::from_secs(120)
        );
    }

    #[test]
    fn checkpoint_clock_zero_is_always_due() {
        let clock = CheckpointClock::new(Duration::ZERO);
        assert!(clock.due());
    }

    #[test]
    fn checkpoint_clock_waits_for_interval() {
        let clock = CheckpointClock::new(Duration::from_secs(3600));
        assert!(!clock.due());
    }

    #[test]
    fn continue_training_prompt_names_clickhouse_and_fingerprints() {
        assert!(CONTINUE_TRAINING_LOCALLY.contains("ClickHouse"));
        assert!(CONTINUE_TRAINING_LOCALLY.contains("bail if the DB is down"));
        assert!(CONTINUE_TRAINING_LOCALLY.contains("fingerprint"));
        assert!(CONTINUE_TRAINING_LOCALLY.contains("continue training locally"));
        assert!(CONTINUE_TRAINING_LOCALLY.contains("git fetch"));
        assert!(CONTINUE_TRAINING_LOCALLY.contains("@{u}"));
    }
}
