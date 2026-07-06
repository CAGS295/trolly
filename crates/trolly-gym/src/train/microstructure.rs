//! WoLF-PPO training on [`MicrostructureSim`](crate::sim::MicrostructureSim).

use std::path::PathBuf;

use crate::ppo::WolfPpoConfig;
use crate::replay::action_from_index;
use crate::sim::{MicrostructureConfig, MicrostructureSim, MicrostructureStats};

use super::checkpoint::save_checkpoint;
use super::driver::{TrainDriverConfig, TrainMetrics, WolfPpoTrainDriver};
use super::rollout::StepOutput;

/// Training configuration for the synthetic microstructure benchmark.
#[derive(Debug, Clone)]
pub struct MicrostructureTrainConfig {
    pub sim: MicrostructureConfig,
    pub driver: TrainDriverConfig,
    pub wolf: WolfPpoConfig,
    pub num_updates: usize,
    pub checkpoint_dir: Option<PathBuf>,
}

impl Default for MicrostructureTrainConfig {
    fn default() -> Self {
        let sim = MicrostructureConfig::default();
        Self {
            driver: TrainDriverConfig {
                obs_dim: sim.obs_dim(),
                num_actions: 3,
                horizon: sim.episode_steps.min(64),
                ..Default::default()
            },
            sim,
            wolf: WolfPpoConfig::default(),
            num_updates: 3,
            checkpoint_dir: None,
        }
    }
}

/// Train on [`MicrostructureSim`] and save row-player weights after each update.
pub fn run_microstructure_train_with_checkpoints(
    config: MicrostructureTrainConfig,
) -> (Vec<TrainMetrics>, Vec<MicrostructureStats>, Vec<PathBuf>) {
    let checkpoint_dir = config.checkpoint_dir.unwrap_or_else(|| {
        std::env::temp_dir().join(format!(
            "trolly_gym_microstructure_{}",
            std::process::id()
        ))
    });
    std::fs::create_dir_all(&checkpoint_dir).expect("create checkpoint dir");

    let mut sim = MicrostructureSim::new(config.sim.clone());
    let mut driver = WolfPpoTrainDriver::new(config.driver.clone(), config.wolf.clone());
    let mut metrics_log = Vec::with_capacity(config.num_updates);
    let mut stats_log = Vec::with_capacity(config.num_updates);
    let mut checkpoint_paths = Vec::with_capacity(config.num_updates);

    for update in 0..config.num_updates {
        let mut obs = sim.reset();
        let mut last_stats = MicrostructureStats::default();

        let metrics = driver.train_step(
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

        metrics_log.push(metrics);
        stats_log.push(last_stats);

        let path = checkpoint_dir.join(format!("update_{update}.safetensors"));
        save_checkpoint(&driver.trainer.inner.vs, &path).expect("save microstructure checkpoint");
        checkpoint_paths.push(path);
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
