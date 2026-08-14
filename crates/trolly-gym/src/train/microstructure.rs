//! WoLF-PPO training on [`MicrostructureSim`](crate::sim::MicrostructureSim).

use std::path::{Path, PathBuf};

use crate::ppo::{ActorCritic, WolfPpoConfig};
use crate::replay::action_from_index;
use crate::sim::{MicrostructureConfig, MicrostructureSim, MicrostructureStats};

use crate::ticks::TickTape;

use super::checkpoint::{
    load_checkpoint_if_exists, resolve_resume_checkpoint, save_checkpoint,
    save_checkpoint_with_fingerprint, FINAL_CHECKPOINT, LATEST_CHECKPOINT,
};
use super::driver::{TrainDriverConfig, TrainMetrics, WolfPpoTrainDriver};
use super::microstructure_completion::{
    evaluate_policy_greedy, MicrostructureCompletionCriteria, MicrostructureCompletionRecord,
    MicrostructureCompletionState, MicrostructureEvalSummary,
};
use super::rollout::StepOutput;

/// Training configuration for the synthetic microstructure benchmark.
#[derive(Debug, Clone)]
pub struct MicrostructureTrainConfig {
    pub sim: MicrostructureConfig,
    pub driver: TrainDriverConfig,
    pub wolf: WolfPpoConfig,
    pub num_updates: usize,
    pub checkpoint_dir: Option<PathBuf>,
    /// Completion thresholds; defaults to a tier inferred from `sim`.
    pub completion: Option<MicrostructureCompletionCriteria>,
    /// Compute device for the WoLF-PPO driver (CPU or CUDA/ROCm).
    pub device: tch::Device,
    /// Optional ClickHouse / sim tick tape (same [`TickRow`] schema as ingest).
    pub tape: Option<TickTape>,
    /// Data-window identity hashed into the checkpoint fingerprint.
    pub data_window: String,
}

impl Default for MicrostructureTrainConfig {
    fn default() -> Self {
        let sim = MicrostructureConfig::default();
        Self {
            driver: TrainDriverConfig {
                obs_dim: sim.obs_dim(),
                num_actions: 3,
                horizon: sim.episode_steps,
                ..Default::default()
            },
            sim: sim.clone(),
            wolf: WolfPpoConfig::default(),
            num_updates: 3,
            checkpoint_dir: None,
            completion: Some(MicrostructureCompletionCriteria::for_config(&sim)),
            device: tch::Device::Cpu,
            tape: None,
            data_window: "sim:default".into(),
        }
    }
}

impl MicrostructureTrainConfig {
    pub fn completion_criteria(&self) -> MicrostructureCompletionCriteria {
        self.completion
            .clone()
            .unwrap_or_else(|| MicrostructureCompletionCriteria::for_config(&self.sim))
    }
}

/// Long-lived microstructure training session with checkpoint resume.
pub struct MicrostructureTrainSession {
    driver: WolfPpoTrainDriver,
    sim_config: MicrostructureConfig,
    completion: MicrostructureCompletionState,
    tape: Option<TickTape>,
    data_window: String,
    config_fingerprint: String,
    pub update_count: usize,
}

impl MicrostructureTrainSession {
    /// Create a fresh driver and simulation config.
    pub fn new(config: &MicrostructureTrainConfig) -> Self {
        Self {
            driver: WolfPpoTrainDriver::new_on_device(
                config.driver.clone(),
                config.wolf.clone(),
                config.device,
            ),
            sim_config: config.sim.clone(),
            completion: MicrostructureCompletionState::new(config.completion_criteria()),
            tape: config.tape.clone(),
            data_window: config.data_window.clone(),
            config_fingerprint: format!(
                "microstructure arch={:?} hidden={:?} seed={}",
                config.wolf.ppo.architecture, config.wolf.ppo.hidden_sizes, config.sim.seed
            ),
            update_count: 0,
        }
    }

    /// Create a session and load weights from `checkpoint_dir` when present.
    pub fn resume_from(checkpoint_dir: impl AsRef<Path>, config: &MicrostructureTrainConfig) -> Self {
        let checkpoint_dir = checkpoint_dir.as_ref();
        std::fs::create_dir_all(checkpoint_dir).expect("create checkpoint dir");
        let mut session = Self::new(config);
        if let Some(path) = resolve_resume_checkpoint(checkpoint_dir) {
            load_checkpoint_if_exists(&mut session.driver.trainer.inner.vs, &path);
        }
        if let Some(record) = MicrostructureCompletionState::load_marker(checkpoint_dir) {
            if record.completed {
                session.completion.record = Some(record);
            }
        }
        session
    }

    /// Whether this model already satisfies completion criteria.
    pub fn is_completed(&self) -> bool {
        self.completion.is_completed()
    }

    /// Persisted completion record, if any.
    pub fn completion_record(&self) -> Option<&MicrostructureCompletionRecord> {
        self.completion.record.as_ref()
    }

    /// Borrow the inner actor-critic for inference or tests.
    pub fn actor_critic(&self) -> &ActorCritic {
        self.driver.actor_critic()
    }

    /// Run one collect-and-update step on a fresh episode.
    ///
    /// No-op when [`Self::is_completed`] is true.
    pub fn train_step(&mut self) -> (TrainMetrics, MicrostructureStats) {
        if self.is_completed() {
            return (TrainMetrics::idle(), MicrostructureStats::default());
        }

        let mut sim = if let Some(tape) = &self.tape {
            MicrostructureSim::with_tape(self.sim_config.clone(), tape.clone())
        } else {
            MicrostructureSim::new(self.sim_config.clone())
        };
        let mut obs = sim.reset();
        let mut last_stats = MicrostructureStats::default();

        let metrics = self.driver.train_step(
            obs.clone(),
            |current_obs, action_idx| {
                let _ = current_obs;
                let result = sim.step(action_from_index(action_idx));
                if result.done {
                    last_stats = sim.finish_episode();
                    obs = sim.reset();
                } else {
                    obs = result.observation.clone();
                }
                StepOutput {
                    next_observation: obs.clone(),
                    reward: result.reward,
                    done: result.done,
                }
            },
            0.0,
            None,
        );

        self.update_count += 1;
        (metrics, last_stats)
    }

    /// Run held-out eval after a training update; returns summary when eval runs.
    pub fn maybe_evaluate_completion(
        &mut self,
        checkpoint_dir: &Path,
    ) -> Option<(MicrostructureEvalSummary, bool)> {
        if !self.completion.should_eval(self.update_count) {
            return None;
        }
        let summary = evaluate_policy_greedy(
            self.actor_critic(),
            &self.sim_config,
            self.completion.criteria(),
        );
        let completed = self.completion.apply_eval_summary(&summary, self.update_count);
        if completed {
            if let Some(record) = self.completion.record.as_mut() {
                record.tier =
                    MicrostructureCompletionCriteria::tier_name(&self.sim_config).into();
            }
            self.completion.save_marker(checkpoint_dir);
            if let Some(parent) = checkpoint_dir.parent().and_then(|p| p.parent()) {
                super::microstructure_completion::refresh_completed_manifest(parent);
            }
        }
        Some((summary, completed))
    }

    /// Persist latest weights, fingerprint sidecar, and optionally a numbered snapshot.
    pub fn save_checkpoint(&self, checkpoint_dir: &Path, save_numbered: bool) {
        std::fs::create_dir_all(checkpoint_dir).expect("create checkpoint dir");
        if save_numbered {
            let path = checkpoint_dir.join(format!("update_{}.safetensors", self.update_count - 1));
            save_checkpoint(&self.driver.trainer.inner.vs, &path)
                .expect("save numbered microstructure checkpoint");
        }
        let latest = checkpoint_dir.join(LATEST_CHECKPOINT);
        save_checkpoint_with_fingerprint(
            &self.driver.trainer.inner.vs,
            &latest,
            &self.data_window,
            &self.config_fingerprint,
        )
        .expect("save latest microstructure checkpoint + fingerprint");
    }

    /// Copy `latest.safetensors` to `final.safetensors`.
    pub fn finalize_checkpoints(&self, checkpoint_dir: &Path) {
        let latest = checkpoint_dir.join(LATEST_CHECKPOINT);
        if !latest.exists() {
            return;
        }
        std::fs::copy(&latest, checkpoint_dir.join(FINAL_CHECKPOINT)).expect("copy final checkpoint");
    }
}

/// Train on [`MicrostructureSim`] and save row-player weights after each update.
///
/// Resumes from existing checkpoints in `checkpoint_dir` when present.
/// Stops early when completion criteria are met.
pub fn run_microstructure_train_with_checkpoints(
    config: MicrostructureTrainConfig,
) -> (Vec<TrainMetrics>, Vec<MicrostructureStats>, Vec<PathBuf>) {
    let checkpoint_dir = config
        .checkpoint_dir
        .clone()
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!(
                "trolly_gym_microstructure_{}",
                std::process::id()
            ))
        });
    let mut session = MicrostructureTrainSession::resume_from(&checkpoint_dir, &config);
    let mut metrics_log = Vec::with_capacity(config.num_updates);
    let mut stats_log = Vec::with_capacity(config.num_updates);
    let mut checkpoint_paths = Vec::with_capacity(config.num_updates);

    for _ in 0..config.num_updates {
        if session.is_completed() {
            break;
        }
        let (metrics, stats) = session.train_step();
        metrics_log.push(metrics);
        stats_log.push(stats);
        session.save_checkpoint(&checkpoint_dir, true);
        checkpoint_paths.push(
            checkpoint_dir.join(format!("update_{}.safetensors", session.update_count - 1)),
        );
        session.maybe_evaluate_completion(&checkpoint_dir);
    }

    (metrics_log, stats_log, checkpoint_paths)
}

/// Run a short smoke training loop (3 updates) into a temp directory.
#[cfg(test)]
pub fn smoke_microstructure_train() -> (Vec<TrainMetrics>, Vec<PathBuf>) {
    let (metrics, _stats, paths) = run_microstructure_train_with_checkpoints(
        MicrostructureTrainConfig::default(),
    );
    (metrics, paths)
}
