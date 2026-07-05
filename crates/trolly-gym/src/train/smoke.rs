//! Short training runs with checkpoint I/O for development smoke tests.

use std::path::{Path, PathBuf};

use crate::ppo::WolfPpoConfig;

use super::checkpoint::save_checkpoint;
use super::driver::{TrainDriverConfig, TrainMetrics, WolfPpoTrainDriver};
use super::rollout::StepOutput;

/// Configuration for a short WoLF-PPO smoke training run.
#[derive(Debug, Clone)]
pub struct SmokeTrainConfig {
    pub driver: TrainDriverConfig,
    pub wolf: WolfPpoConfig,
    /// Number of collect-update cycles to run.
    pub num_steps: usize,
    /// Directory to write checkpoints into. When `None`, uses a temp dir.
    pub checkpoint_dir: Option<PathBuf>,
}

impl Default for SmokeTrainConfig {
    fn default() -> Self {
        Self {
            driver: TrainDriverConfig {
                obs_dim: 4,
                num_actions: 3,
                horizon: 16,
                ..Default::default()
            },
            wolf: WolfPpoConfig::default(),
            num_steps: 3,
            checkpoint_dir: None,
        }
    }
}

/// Run a short WoLF-PPO training loop and save a checkpoint after each step.
///
/// Returns per-step metrics and the paths of written checkpoint files
/// (`{checkpoint_dir}/step_{n}.safetensors`).
pub fn smoke_train_loop(config: SmokeTrainConfig) -> (Vec<TrainMetrics>, Vec<PathBuf>) {
    let checkpoint_dir = config.checkpoint_dir.unwrap_or_else(|| {
        std::env::temp_dir().join(format!(
            "trolly_gym_smoke_train_{}",
            std::process::id()
        ))
    });
    std::fs::create_dir_all(&checkpoint_dir).expect("create checkpoint dir");

    let obs_dim = config.driver.obs_dim;
    let mut driver = WolfPpoTrainDriver::new(config.driver, config.wolf);
    let mut metrics_log = Vec::with_capacity(config.num_steps);
    let mut checkpoint_paths = Vec::with_capacity(config.num_steps);

    for step in 0..config.num_steps {
        let reward = if step % 2 == 0 { 1.0_f32 } else { -1.0_f32 };
        let metrics = driver.train_step(
            vec![0.0_f32; obs_dim as usize],
            |obs, _action| StepOutput {
                next_observation: obs,
                reward,
                done: false,
            },
            0.0,
            None,
        );
        metrics_log.push(metrics);

        let path = checkpoint_path(&checkpoint_dir, step);
        save_checkpoint(&driver.trainer.inner.vs, &path).expect("save checkpoint");
        checkpoint_paths.push(path);
    }

    (metrics_log, checkpoint_paths)
}

fn checkpoint_path(dir: &Path, step: usize) -> PathBuf {
    dir.join(format!("step_{step}.safetensors"))
}
