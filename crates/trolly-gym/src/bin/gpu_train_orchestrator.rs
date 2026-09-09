//! Local weekday working-hours GPU training orchestrator.
//!
//! Runs the existing trolly-gym WoLF-PPO sessions (matrix games + microstructure)
//! on this machine during `TROLLY_WORK_*` hours. Not a cloud job and not the
//! Daily workplan orchestrator — complementary: same [`BROAD_GOAL`], this
//! process only trains. Job choice prefers incomplete stream-shaped
//! checkpoints; each `--once` window writes `progress.json`.
//!
//! ```bash
//! export LIBTORCH_USE_PYTORCH=1
//! export LIBTORCH_BYPASS_VERSION_CHECK=1
//! cargo run -p trolly-gym --features torch --bin gpu_train_orchestrator -- --probe
//! cargo run -p trolly-gym --features torch --bin gpu_train_orchestrator -- --once
//! cargo run -p trolly-gym --features torch --bin gpu_train_orchestrator -- --continue-local
//! ```
//!
//! `--continue-local` (also `TROLLY_CONTINUE_LOCAL=1`, or the chat phrase
//! "continue training locally"): `git fetch` + rebase onto `@{u}` when the
//! tree is clean, start ClickHouse, ingest sim ticks, train a daily slice,
//! write checkpoint fingerprints into `progress.json`.
//!
//! Enable the user systemd timer:
//! ```bash
//! mkdir -p ~/.config/systemd/user
//! cp crates/trolly-gym/systemd/trolly-gpu-train.* ~/.config/systemd/user/
//! systemctl --user daemon-reload
//! systemctl --user enable --now trolly-gpu-train.timer
//! ```

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use trolly_gym::device::{describe_device, gpu_available, require_rx_training_device};
use trolly_gym::fingerprint::load_sidecar;
use trolly_gym::games::{
    matching_pennies::{matching_pennies_weighted, WEIGHTED_NES},
    rock_paper_scissors::{rps_weighted, WEIGHTED_NES as RPS_WEIGHTED_NES},
    SelfPlayConfig, WolfPpoSelfPlaySession,
};
use trolly_gym::orchestrator::{
    checkpoint_interval_from_env, local_weekday_and_minutes, prepare_local_training,
    select_train_jobs, sync_local_git_upstream, write_progress_log, CheckpointClock, DeviceSpec,
    LocalTrainPrep, TrainGap, WorkingHours, BROAD_GOAL, CONTINUE_TRAINING_LOCALLY,
};
use trolly_gym::ppo::{ActorCriticArchitecture, PpoConfig, WolfPpoConfig};
use trolly_gym::sim::MicrostructureConfig;
use trolly_gym::ticks::{
    clickhouse_unreachable_error, ensure_local_clickhouse, require_clickhouse_reachable,
    ClickHouseTicks,
};
use trolly_gym::train::checkpoint::LATEST_CHECKPOINT;
use trolly_gym::train::{
    GaussianArchitecture, GaussianMicrostructureTrainConfig, GaussianMicrostructureTrainSession,
    GaussianTrainDriverConfig, GAUSSIAN_LIQUID_ARCH, GAUSSIAN_MLP_ARCH,
    RETIRED_UNIT_LOT_MICROSTRUCTURE,
};

fn main() {
    let mode = parse_mode();
    let hours = WorkingHours::from_env();
    let spec = DeviceSpec::from_env();

    match mode {
        Mode::Probe => {
            if let Err(err) = probe(&hours, spec) {
                eprintln!("{err}");
                std::process::exit(1);
            }
        }
        Mode::Once => {
            if let Err(err) = run_once(&hours, spec) {
                eprintln!("{err}");
                std::process::exit(1);
            }
        }
        Mode::Continue => {
            if let Err(err) = run_continue(spec) {
                eprintln!("{err}");
                std::process::exit(1);
            }
        }
        Mode::Daemon => {
            if let Err(err) = run_daemon(&hours, spec) {
                eprintln!("{err}");
                std::process::exit(1);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Once,
    Continue,
    Daemon,
    Probe,
}

fn parse_mode() -> Mode {
    if std::env::var("TROLLY_CONTINUE_LOCAL")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return Mode::Continue;
    }
    let mut mode = Mode::Once;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--once" => mode = Mode::Once,
            "--continue" | "--continue-local" => mode = Mode::Continue,
            "--daemon" => mode = Mode::Daemon,
            "--probe" => mode = Mode::Probe,
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other}");
                print_help();
                std::process::exit(2);
            }
        }
    }
    mode
}

fn print_help() {
    eprintln!(
        "gpu_train_orchestrator [--once|--continue-local|--daemon|--probe]\n\
         \n\
         Local weekday GPU training. Defaults: Mon–Fri 09:00–17:00 local.\n\
         --continue-local (alias --continue, TROLLY_CONTINUE_LOCAL=1): fetch\n\
         origin (rebase onto git upstream if clean), ingest ticks to ClickHouse, train a\n\
         daily slice, store hash fingerprints.\n\
         Goal: {BROAD_GOAL}\n\
         {CONTINUE_TRAINING_LOCALLY}\n\
         Training bails unless HIP sees a discrete RX card (not CPU, not iGPU)\n\
         and ClickHouse answers on TROLLY_CLICKHOUSE_URL (no in-memory tape).\n\
         Env: TROLLY_WORK_START TROLLY_WORK_END TROLLY_WORK_WEEKDAYS\n\
              TROLLY_TRAIN_DEVICE TROLLY_TRAIN_JOBS TROLLY_TRAIN_DURATION_SECS\n\
              TROLLY_TRAIN_GPU_MATCH TROLLY_CONTINUE_LOCAL TROLLY_CLICKHOUSE_URL\n\
              TROLLY_CHECKPOINT_INTERVAL_SECS CHECKPOINT_DIR"
    );
}

fn probe(hours: &WorkingHours, spec: DeviceSpec) -> Result<(), String> {
    let (weekday, minutes) = local_weekday_and_minutes().map_err(|e| e.to_string())?;
    let device = require_rx_training_device(spec)?;
    let ch = require_clickhouse_reachable()?;
    let gap = planned_jobs();
    println!("goal: {BROAD_GOAL}");
    println!(
        "schedule: weekdays={:?} {:02}:{:02}-{:02}:{:02} local",
        (1..=7)
            .filter(|d| hours.weekdays[*d as usize])
            .collect::<Vec<_>>(),
        hours.start_minutes / 60,
        hours.start_minutes % 60,
        hours.end_minutes / 60,
        hours.end_minutes % 60,
    );
    println!(
        "now: weekday={weekday} {:02}:{:02} in_window={}",
        minutes / 60,
        minutes % 60,
        hours.contains(weekday, minutes)
    );
    println!(
        "device_spec={spec} resolved={} gpu_available={}",
        describe_device(device),
        gpu_available()
    );
    println!("clickhouse: {} ok", ch.url);
    println!(
        "checkpoint_interval={}s (TROLLY_CHECKPOINT_INTERVAL_SECS; 0=every update)",
        checkpoint_interval_from_env().as_secs()
    );
    println!("jobs={:?} reason={}", gap.jobs, gap.reason);
    println!("continue: {CONTINUE_TRAINING_LOCALLY}");
    Ok(())
}

fn run_continue(spec: DeviceSpec) -> Result<(), String> {
    let budget = std::env::var("TROLLY_TRAIN_DURATION_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(120);
    println!("continue training locally ({budget}s)");
    train_window(spec, budget)
}

fn run_once(hours: &WorkingHours, spec: DeviceSpec) -> Result<(), String> {
    let (weekday, minutes) = local_weekday_and_minutes().map_err(|e| e.to_string())?;
    let Some(remaining) = hours.remaining_secs(weekday, minutes) else {
        println!(
            "outside working hours (weekday={weekday} {:02}:{:02}); skipping",
            minutes / 60,
            minutes % 60
        );
        return Ok(());
    };
    train_window(spec, remaining)
}

fn run_daemon(hours: &WorkingHours, spec: DeviceSpec) -> Result<(), String> {
    loop {
        let (weekday, minutes) = local_weekday_and_minutes().map_err(|e| e.to_string())?;
        if let Some(remaining) = hours.remaining_secs(weekday, minutes) {
            train_window(spec, remaining)?;
        } else {
            let wait = hours.secs_until_next_window(weekday, minutes).max(1);
            println!("sleeping {wait}s until next working-hours window");
            thread::sleep(Duration::from_secs(wait.min(3600)));
        }
    }
}

fn train_window(spec: DeviceSpec, remaining_secs: u64) -> Result<(), String> {
    let device = require_rx_training_device(spec)?;
    ensure_local_clickhouse()
        .map_err(|err| clickhouse_unreachable_error(&ClickHouseTicks::default().url, err))?;
    let git_note = sync_local_git_upstream();
    println!("git: {git_note}");
    let cap = std::env::var("TROLLY_TRAIN_DURATION_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(remaining_secs);
    let budget = remaining_secs.min(cap).max(1);
    let gap = planned_jobs();
    let prep = prepare_local_training()?;
    let slice = (budget / gap.jobs.len() as u64).max(1);
    println!("goal: {BROAD_GOAL}");
    println!(
        "gpu_train_orchestrator: device={} spec={spec} budget={budget}s jobs={:?} reason={} checkpoint_interval={}s",
        describe_device(device),
        gap.jobs,
        gap.reason,
        checkpoint_interval_from_env().as_secs()
    );
    println!(
        "ticks: {} ch_ok={} {}",
        prep.ticks_written, prep.clickhouse_ok, prep.note
    );

    let mut extras = vec![
        ("budget_secs", budget.to_string()),
        ("ticks_written", prep.ticks_written.to_string()),
        ("clickhouse_ok", prep.clickhouse_ok.to_string()),
        ("data_window", prep.data_window.clone()),
        ("session_id", prep.session_id.clone()),
        ("git_sync", git_note),
    ];
    for job in &gap.jobs {
        match job.as_str() {
            "matrix" => {
                let last = train_matrix(device, Duration::from_secs(slice))?;
                extras.push(("matrix_last_nes_dist", last));
            }
            "microstructure" => {
                let last = train_microstructure(device, Duration::from_secs(slice), &prep)?;
                extras.push(("microstructure_mean_action_vs_hold", last));
            }
            other => return Err(format!("unknown TROLLY_TRAIN_JOBS entry: {other}")),
        }
    }
    let fingerprints = collect_fingerprint_json(&checkpoint_root());
    extras.push(("fingerprints", fingerprints));
    let extra_refs: Vec<(&str, &str)> = extras.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let path = write_progress_log(&checkpoint_root(), &today_iso(), &gap, &extra_refs)
        .map_err(|e| e.to_string())?;
    println!("progress: {}", path.display());
    Ok(())
}

fn collect_fingerprint_json(root: &PathBuf) -> String {
    let mut parts = Vec::new();
    let dirs = [
        root.join("microstructure").join(GAUSSIAN_MLP_ARCH),
        root.join("microstructure").join(GAUSSIAN_LIQUID_ARCH),
        root.join("microstructure/mlp"),
        root.join("microstructure/liquid"),
        root.join("matrix/mlp/matching_pennies_weighted"),
        root.join("matrix/liquid/matching_pennies_weighted"),
    ];
    for dir in dirs {
        if let Some(fp) = load_sidecar(&dir) {
            if let Ok(text) = serde_json::to_string(&fp) {
                parts.push(text);
            }
        }
    }
    format!("[{}]", parts.join(","))
}

fn planned_jobs() -> TrainGap {
    select_train_jobs(
        &checkpoint_root(),
        std::env::var("TROLLY_TRAIN_JOBS").ok().as_deref(),
    )
}

fn today_iso() -> String {
    std::process::Command::new("date")
        .args(["+%F"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn checkpoint_root() -> PathBuf {
    std::env::var("CHECKPOINT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("checkpoints/gpu_train_orchestrator"))
}

fn train_matrix(device: tch::Device, duration: Duration) -> Result<String, String> {
    let root = checkpoint_root().join("matrix");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let mp = matching_pennies_weighted();
    let rps = rps_weighted();
    let games: [(&str, &trolly_gym::games::MatrixGame, &[f64]); 2] = [
        ("matching_pennies_weighted", &mp, &WEIGHTED_NES),
        ("rps_weighted", &rps, &RPS_WEIGHTED_NES),
    ];
    let per = duration / 4;
    let mut last_nes = 0.0_f64;
    for (architecture, arch_name) in [
        (ActorCriticArchitecture::Mlp, "mlp"),
        (ActorCriticArchitecture::Liquid, "liquid"),
    ] {
        for (name, game, nes) in games {
            let out_dir = root.join(arch_name).join(name);
            std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
            let config = SelfPlayConfig {
                num_updates: 10,
                batch_size: 64,
                ppo_config: PpoConfig {
                    architecture,
                    ppo_epochs: 2,
                    ..Default::default()
                },
            };
            let wolf = WolfPpoConfig {
                ppo: config.ppo_config.clone(),
                ..WolfPpoConfig::default().with_alpha_lose(0.1)
            };
            let resumed = out_dir.join(LATEST_CHECKPOINT).exists();
            let mut session = WolfPpoSelfPlaySession::resume_from_on_device(
                &out_dir, game, &config, wolf, device,
            );
            println!(
                "=== matrix {arch_name}/{name} {}s ({}) ===",
                per.as_secs(),
                if resumed { "resumed" } else { "fresh" }
            );
            let mut clock = CheckpointClock::from_env();
            let start = Instant::now();
            while start.elapsed() < per {
                let distances = session.run_updates(game, nes, 10, &out_dir, false, false);
                last_nes = distances.last().copied().unwrap_or(last_nes);
                if clock.due() {
                    session.persist_latest(&out_dir);
                    clock.mark();
                    println!(
                        "  checkpoint updates={} nes_dist={last_nes:.4} elapsed={:.1}s",
                        session.update_count,
                        start.elapsed().as_secs_f64()
                    );
                }
            }
            session.persist_latest(&out_dir);
            session.finalize_checkpoints(&out_dir);
            println!(
                "  done updates={} nes_dist={last_nes:.4} elapsed={:.1}s",
                session.update_count,
                start.elapsed().as_secs_f64()
            );
        }
    }
    Ok(format!("{last_nes:.4}"))
}

fn train_microstructure(
    device: tch::Device,
    duration: Duration,
    prep: &LocalTrainPrep,
) -> Result<String, String> {
    let retired = checkpoint_root().join(RETIRED_UNIT_LOT_MICROSTRUCTURE);
    if retired.exists() {
        println!(
            "skip resume of retired unit-lot fossils at {}",
            retired.display()
        );
    }
    let sim = MicrostructureConfig::default();
    let per = duration / 2;
    let mut last_eval = String::from("n/a");
    for (architecture, arch_name) in [
        (GaussianArchitecture::Mlp, GAUSSIAN_MLP_ARCH),
        (GaussianArchitecture::LiquidRungs, GAUSSIAN_LIQUID_ARCH),
    ] {
        let out_dir = checkpoint_root().join("microstructure").join(arch_name);
        std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
        let config = GaussianMicrostructureTrainConfig {
            sim: sim.clone(),
            driver: GaussianTrainDriverConfig {
                obs_dim: sim.ladder_obs_dim(),
                horizon: sim.episode_steps,
                architecture,
                rung_count: sim.rung_count as i64,
                ..Default::default()
            },
            wolf: WolfPpoConfig::default(),
            checkpoint_dir: Some(out_dir.clone()),
            device,
            data_window: prep.data_window.clone(),
            ..Default::default()
        };
        let mut session = GaussianMicrostructureTrainSession::resume_from(&out_dir, &config);
        println!(
            "=== microstructure {arch_name} {}s (ladder tanh-Gaussian; do not resume {}) ===",
            per.as_secs(),
            RETIRED_UNIT_LOT_MICROSTRUCTURE
        );
        let mut clock = CheckpointClock::from_env();
        let start = Instant::now();
        while start.elapsed() < per {
            let (metrics, _stats) = session.train_step();
            if clock.due() {
                session.save_checkpoint(&out_dir, false);
                clock.mark();
                let eval = session.evaluate_mean_action(&[1000, 1001, 1002]);
                println!(
                    "  checkpoint loss={:.4} mean_action={:.3} hold={:.3} mean_|q|={:.3} elapsed={:.1}s",
                    metrics.policy_loss,
                    eval.mean_action_reward,
                    eval.hold_reward,
                    eval.mean_abs_inventory,
                    start.elapsed().as_secs_f64()
                );
            }
        }
        session.save_checkpoint(&out_dir, false);
        let eval = session.evaluate_mean_action(&[1000, 1001, 1002]);
        last_eval = format!(
            "{:.3}/{:.3}",
            eval.mean_action_reward, eval.hold_reward
        );
        println!(
            "  done {arch_name} updates={} mean_action={:.3} vs hold={:.3} mean_|q|={:.3} elapsed={:.1}s",
            session.update_count,
            eval.mean_action_reward,
            eval.hold_reward,
            eval.mean_abs_inventory,
            start.elapsed().as_secs_f64()
        );
        if let Some(fp) = load_sidecar(&out_dir) {
            println!(
                "  fingerprint weights={} data={}",
                &fp.weights_sha256[..12.min(fp.weights_sha256.len())],
                &fp.data_window_sha256[..12.min(fp.data_window_sha256.len())]
            );
        }
    }
    Ok(last_eval)
}
