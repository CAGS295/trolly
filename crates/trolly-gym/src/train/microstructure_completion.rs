//! Completion criteria and eval for the microstructure benchmark.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tch::{Device, Kind, Tensor};

use crate::ppo::ActorCritic;
use crate::replay::action_from_index;
use crate::sim::{
    oracle_reward_estimate, run_baseline_episode, run_episode_with_actions, BaselinePolicy,
    MicrostructureConfig,
};

/// Sidecar written when a model satisfies completion criteria.
pub const COMPLETED_MARKER: &str = "completed.json";

/// Aggregated list of all completed models under a checkpoint root.
pub const COMPLETED_MODELS_MANIFEST: &str = "completed_models.json";

/// Thresholds and eval protocol for declaring a model trained.
#[derive(Debug, Clone)]
pub struct MicrostructureCompletionCriteria {
    /// Held-out seeds (distinct from training `sim.seed`).
    pub eval_seeds: Vec<u64>,
    pub eval_episodes_per_seed: usize,
    /// Consecutive eval rounds that must pass before marking complete.
    pub window_evals: usize,
    /// Minimum policy updates before completion is allowed.
    pub min_updates: usize,
    /// Run eval every N training updates.
    pub eval_every_updates: usize,
    /// Drift configs: mean eval reward must be within this gap of oracle.
    pub max_oracle_gap: Option<f32>,
    /// Absolute floor on mean eval reward.
    pub min_mean_eval_reward: Option<f32>,
    /// Zero-drift: mean eval must beat hold baseline by this margin.
    pub min_margin_over_hold: Option<f32>,
    /// Cap on mean position changes per episode.
    pub max_trades_per_episode: Option<f32>,
    /// Max std of mean eval reward across the stability window.
    pub max_eval_std: Option<f32>,
}

impl Default for MicrostructureCompletionCriteria {
    fn default() -> Self {
        Self::zero_drift()
    }
}

impl MicrostructureCompletionCriteria {
    /// Criteria for default zero-drift random-walk config.
    pub fn zero_drift() -> Self {
        Self {
            eval_seeds: (1000..1010).collect(),
            eval_episodes_per_seed: 1,
            window_evals: 3,
            min_updates: 5,
            eval_every_updates: 5,
            max_oracle_gap: None,
            min_mean_eval_reward: Some(-0.5),
            min_margin_over_hold: Some(-0.25),
            max_trades_per_episode: Some(2.0),
            max_eval_std: Some(1.0),
        }
    }

    /// Criteria for drift-only teaching configs (`mid_drift != 0`).
    pub fn drift_tier(config: &MicrostructureConfig) -> Self {
        let gap = if config.mid_noise <= f32::EPSILON {
            1.0
        } else {
            2.0
        };
        Self {
            eval_seeds: (2000..2010).collect(),
            eval_episodes_per_seed: 1,
            window_evals: 3,
            min_updates: 5,
            eval_every_updates: 5,
            max_oracle_gap: Some(gap),
            min_mean_eval_reward: None,
            min_margin_over_hold: None,
            max_trades_per_episode: Some(3.0),
            max_eval_std: Some(1.5),
        }
    }

    /// Pick zero-drift vs drift tier from sim config.
    pub fn for_config(config: &MicrostructureConfig) -> Self {
        if config.mid_drift.abs() > f32::EPSILON {
            Self::drift_tier(config)
        } else {
            Self::zero_drift()
        }
    }

    pub fn tier_name(config: &MicrostructureConfig) -> &'static str {
        if config.mid_drift.abs() > f32::EPSILON {
            "drift"
        } else {
            "zero_drift"
        }
    }
}

/// Result of evaluating a policy across held-out seeds.
#[derive(Debug, Clone)]
pub struct MicrostructureEvalSummary {
    pub mean_reward: f32,
    pub mean_trades: f32,
    pub eval_std: f32,
    pub oracle_reward: f32,
    pub hold_baseline: f32,
    pub per_seed_rewards: Vec<f32>,
}

/// Persisted completion record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MicrostructureCompletionRecord {
    pub completed: bool,
    pub tier: String,
    pub mean_eval_reward: f32,
    pub oracle_reward: f32,
    pub hold_baseline_reward: f32,
    pub mean_trades: f32,
    pub eval_std: f32,
    pub update_count: usize,
    pub eval_seeds: Vec<u64>,
}

/// One entry in the aggregated completed-models manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompletedModelEntry {
    pub benchmark: String,
    pub architecture: String,
    pub checkpoint_dir: String,
    #[serde(flatten)]
    pub record: MicrostructureCompletionRecord,
}

/// All completed microstructure models under a checkpoint tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrainingCompletedManifest {
    pub updated_at: String,
    pub models: Vec<CompletedModelEntry>,
}

/// Scan `checkpoints_root` and rewrite `completed_models.json`.
pub fn refresh_completed_manifest(checkpoints_root: impl AsRef<Path>) {
    let checkpoints_root = checkpoints_root.as_ref();
    let micro_root = checkpoints_root.join("microstructure_train");
    let mut models = Vec::new();

    for arch in ["mlp", "liquid"] {
        let dir = micro_root.join(arch);
        if let Some(record) = MicrostructureCompletionState::load_marker(&dir) {
            if record.completed {
                models.push(CompletedModelEntry {
                    benchmark: "microstructure".into(),
                    architecture: arch.into(),
                    checkpoint_dir: dir.display().to_string(),
                    record,
                });
            }
        }
    }

    models.sort_by(|a, b| {
        (&a.benchmark, &a.architecture).cmp(&(&b.benchmark, &b.architecture))
    });

    let manifest = TrainingCompletedManifest {
        updated_at: chrono_lite_timestamp(),
        models,
    };
    let path = checkpoints_root.join(COMPLETED_MODELS_MANIFEST);
    let text = serde_json::to_string_pretty(&manifest).expect("serialize completed_models.json");
    std::fs::write(path, text).expect("write completed_models.json");
}

fn chrono_lite_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    format!("{secs}")
}

/// Tracks eval history and completion state for a training session.
#[derive(Debug, Clone)]
pub struct MicrostructureCompletionState {
    pub criteria: MicrostructureCompletionCriteria,
    eval_history: Vec<f32>,
    pub record: Option<MicrostructureCompletionRecord>,
}

impl MicrostructureCompletionState {
    pub fn new(criteria: MicrostructureCompletionCriteria) -> Self {
        Self {
            criteria,
            eval_history: Vec::new(),
            record: None,
        }
    }

    pub fn is_completed(&self) -> bool {
        self.record.as_ref().is_some_and(|r| r.completed)
    }

    pub fn load_marker(checkpoint_dir: impl AsRef<Path>) -> Option<MicrostructureCompletionRecord> {
        let path = checkpoint_dir.as_ref().join(COMPLETED_MARKER);
        if !path.exists() {
            return None;
        }
        let text = std::fs::read_to_string(&path).expect("read completed.json");
        serde_json::from_str(&text).expect("parse completed.json")
    }

    pub fn save_marker(&self, checkpoint_dir: impl AsRef<Path>) {
        if let Some(record) = &self.record {
            let path = checkpoint_dir.as_ref().join(COMPLETED_MARKER);
            let text = serde_json::to_string_pretty(record).expect("serialize completed.json");
            std::fs::write(path, text).expect("write completed.json");
        }
    }

    /// Greedy-policy eval; returns summary and whether completion criteria are met.
    pub fn apply_eval_summary(
        &mut self,
        summary: &MicrostructureEvalSummary,
        update_count: usize,
    ) -> bool {
        let passed = self.passes_thresholds(summary, update_count);
        if passed {
            self.eval_history.push(summary.mean_reward);
        } else {
            self.eval_history.clear();
        }

        let stable = self.check_stability();
        let completed = passed
            && stable
            && update_count >= self.criteria.min_updates
            && !self.is_completed();

        if completed {
            self.record = Some(MicrostructureCompletionRecord {
                completed: true,
                tier: String::new(), // filled in by caller
                mean_eval_reward: summary.mean_reward,
                oracle_reward: summary.oracle_reward,
                hold_baseline_reward: summary.hold_baseline,
                mean_trades: summary.mean_trades,
                eval_std: std_dev(&self.eval_history),
                update_count,
                eval_seeds: self.criteria.eval_seeds.clone(),
            });
        }

        completed
    }

    pub fn criteria(&self) -> &MicrostructureCompletionCriteria {
        &self.criteria
    }

    pub fn should_eval(&self, update_count: usize) -> bool {
        !self.is_completed()
            && update_count > 0
            && update_count % self.criteria.eval_every_updates == 0
    }

    fn passes_thresholds(&self, summary: &MicrostructureEvalSummary, update_count: usize) -> bool {
        if update_count < self.criteria.min_updates {
            return false;
        }
        if let Some(gap) = self.criteria.max_oracle_gap {
            if summary.mean_reward < summary.oracle_reward - gap {
                return false;
            }
        }
        if let Some(min) = self.criteria.min_mean_eval_reward {
            if summary.mean_reward < min {
                return false;
            }
        }
        if let Some(margin) = self.criteria.min_margin_over_hold {
            if summary.mean_reward < summary.hold_baseline + margin {
                return false;
            }
        }
        if let Some(max_trades) = self.criteria.max_trades_per_episode {
            if summary.mean_trades > max_trades {
                return false;
            }
        }
        true
    }

    fn check_stability(&self) -> bool {
        if self.eval_history.len() < self.criteria.window_evals {
            return false;
        }
        let window = &self.eval_history[self.eval_history.len() - self.criteria.window_evals..];
        if let Some(max_std) = self.criteria.max_eval_std {
            if std_dev(window) > max_std {
                return false;
            }
        }
        true
    }
}

/// Evaluate greedy policy over held-out seeds.
pub fn evaluate_policy_greedy(
    actor_critic: &ActorCritic,
    config: &MicrostructureConfig,
    criteria: &MicrostructureCompletionCriteria,
) -> MicrostructureEvalSummary {
    let obs_dim = config.obs_dim();
    let mut rewards = Vec::new();
    let mut trades = Vec::new();

    for &seed in &criteria.eval_seeds {
        for ep in 0..criteria.eval_episodes_per_seed {
            let eval_seed = seed.wrapping_add(ep as u64);
            let stats = run_episode_with_actions(config, eval_seed, |_, obs| {
                greedy_action(actor_critic, obs, obs_dim)
            });
            rewards.push(stats.total_reward);
            trades.push(stats.trades as f32);
        }
    }

    let mean_reward = mean(&rewards);
    let hold_baseline = mean_baseline(config, &criteria.eval_seeds, BaselinePolicy::Hold);

    MicrostructureEvalSummary {
        mean_reward,
        mean_trades: mean(&trades),
        eval_std: std_dev(&rewards),
        oracle_reward: oracle_reward_estimate(config),
        hold_baseline,
        per_seed_rewards: rewards,
    }
}

/// Check whether a baseline policy satisfies criteria (for tests).
pub fn baseline_meets_criteria(
    config: &MicrostructureConfig,
    criteria: &MicrostructureCompletionCriteria,
    policy: BaselinePolicy,
) -> bool {
    let summary = baseline_eval_summary(config, criteria, policy);
    let state = MicrostructureCompletionState::new(criteria.clone());
    state.passes_thresholds(&summary, criteria.min_updates)
}

fn baseline_eval_summary(
    config: &MicrostructureConfig,
    criteria: &MicrostructureCompletionCriteria,
    policy: BaselinePolicy,
) -> MicrostructureEvalSummary {
    let mut rewards = Vec::new();
    let mut trades = Vec::new();
    for &seed in &criteria.eval_seeds {
        let stats = run_baseline_episode(config, seed, policy);
        rewards.push(stats.total_reward);
        trades.push(stats.trades as f32);
    }
    MicrostructureEvalSummary {
        mean_reward: mean(&rewards),
        mean_trades: mean(&trades),
        eval_std: std_dev(&rewards),
        oracle_reward: oracle_reward_estimate(config),
        hold_baseline: mean_baseline(config, &criteria.eval_seeds, BaselinePolicy::Hold),
        per_seed_rewards: rewards,
    }
}

fn mean_baseline(
    config: &MicrostructureConfig,
    seeds: &[u64],
    policy: BaselinePolicy,
) -> f32 {
    let rewards: Vec<f32> = seeds
        .iter()
        .map(|&seed| run_baseline_episode(config, seed, policy).total_reward)
        .collect();
    mean(&rewards)
}

fn greedy_action(actor_critic: &ActorCritic, obs: &[f32], obs_dim: i64) -> crate::action::Action {
    let t = Tensor::from_slice(obs)
        .to_device(Device::Cpu)
        .to_kind(Kind::Float)
        .view([1, obs_dim]);
    let (logits, _) = actor_critic.forward(&t);
    let idx = logits.argmax(-1, false).int64_value(&[0]);
    action_from_index(idx)
}

fn mean(values: &[f32]) -> f32 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f32>() / values.len() as f32
    }
}

fn std_dev(values: &[f32]) -> f32 {
    if values.len() < 2 {
        return 0.0;
    }
    let m = mean(values);
    let var = values.iter().map(|v| (v - m).powi(2)).sum::<f32>() / values.len() as f32;
    var.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::MicrostructureConfig;

    #[test]
    fn hold_baseline_meets_zero_drift_criteria() {
        let config = MicrostructureConfig {
            episode_steps: 64,
            mid_noise: 0.0,
            ..Default::default()
        };
        let mut criteria = MicrostructureCompletionCriteria::zero_drift();
        criteria.eval_seeds = vec![1000, 1001, 1002];
        criteria.min_updates = 1;
        criteria.window_evals = 1;
        assert!(baseline_meets_criteria(
            &config,
            &criteria,
            BaselinePolicy::Hold,
        ));
    }

    #[test]
    fn oracle_meets_drift_criteria() {
        let config = MicrostructureConfig {
            episode_steps: 128,
            mid_drift: 0.1,
            mid_noise: 0.0,
            trade_cost: 0.5,
            ..Default::default()
        };
        let mut criteria = MicrostructureCompletionCriteria::drift_tier(&config);
        criteria.eval_seeds = vec![2000, 2001];
        criteria.min_updates = 1;
        criteria.window_evals = 1;
        assert!(baseline_meets_criteria(
            &config,
            &criteria,
            BaselinePolicy::Oracle,
        ));
    }

    #[test]
    fn random_baseline_fails_zero_drift_criteria() {
        let config = MicrostructureConfig {
            episode_steps: 64,
            mid_noise: 0.25,
            ..Default::default()
        };
        let mut criteria = MicrostructureCompletionCriteria::zero_drift();
        criteria.eval_seeds = vec![1000];
        criteria.min_updates = 1;
        assert!(!baseline_meets_criteria(
            &config,
            &criteria,
            BaselinePolicy::Random,
        ));
    }
}
