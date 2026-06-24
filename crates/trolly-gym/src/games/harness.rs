//! Self-play matrix-game training loop (PPO / WoLF-PPO).

use tch::{nn, Device, Kind, Tensor};

use crate::ppo::{
    compute_advantages, default_hidden_layers, tensor_from_vec_f64, ActorCritic, PpoConfig,
    PpoTrainer, RolloutBatch, WolfPpoConfig, WolfPpoTrainer,
};

use super::matrix::{MatrixGame, MatrixGameKind};
use super::metrics::{euclidean_distance, max_distance_last_n};

/// Paper reproduction uses 50 independent seeds (CI may use fewer).
pub const DEFAULT_NUM_RUNS: usize = 50;

/// Policy updates per run (paper tracks distance over training; tune for local benchmarks).
pub const DEFAULT_POLICY_UPDATES: usize = 500;

/// Table I: max distance over the last 10 policy updates per run.
pub const DEFAULT_LAST_N_POLICY_UPDATES: usize = 10;

/// Trainer variant under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixTrainerKind {
    Ppo,
    WolfPpo,
}

/// Shared experimental setup for matrix-game self-play.
#[derive(Debug, Clone)]
pub struct MatrixExperimentConfig {
    pub game: MatrixGameKind,
    pub trainer: MatrixTrainerKind,
    pub alpha_lose: f64,
    pub num_policy_updates: usize,
    pub rollout_steps: usize,
    pub seed: u64,
    pub last_n: usize,
}

impl MatrixExperimentConfig {
    pub fn smoke(game: MatrixGameKind, trainer: MatrixTrainerKind, seed: u64) -> Self {
        Self {
            game,
            trainer,
            alpha_lose: 0.01,
            num_policy_updates: 5,
            rollout_steps: 16,
            seed,
            last_n: DEFAULT_LAST_N_POLICY_UPDATES,
        }
    }

    pub fn benchmark(
        game: MatrixGameKind,
        trainer: MatrixTrainerKind,
        alpha_lose: f64,
        seed: u64,
    ) -> Self {
        Self {
            game,
            trainer,
            alpha_lose,
            num_policy_updates: DEFAULT_POLICY_UPDATES,
            rollout_steps: 64,
            seed,
            last_n: DEFAULT_LAST_N_POLICY_UPDATES,
        }
    }
}

/// Outcome of a single seeded matrix-game run.
#[derive(Debug, Clone)]
pub struct MatrixRunResult {
    pub seed: u64,
    pub nes_distances: Vec<f64>,
    pub max_nes_distance_last_n: f64,
}

/// Run one seeded self-play experiment (symmetric game, shared policy).
pub fn run_matrix_experiment(config: &MatrixExperimentConfig) -> MatrixRunResult {
    tch::manual_seed(config.seed as i64);
    let game = MatrixGame::from_kind(config.game);
    let device = Device::Cpu;
    let obs_dim = game.obs_dim() as i64;
    let action_count = game.action_count() as i64;

    let ppo_config = PpoConfig {
        learning_rate: config.alpha_lose,
        use_adam: false,
        ..Default::default()
    };

    let vs = nn::VarStore::new(device);
    let model = ActorCritic::new(
        &vs.root(),
        obs_dim,
        action_count,
        &default_hidden_layers(),
        &ppo_config,
    );
    let mut opt = ActorCritic::build_optimizer(&vs, &ppo_config);

    let ppo_trainer = PpoTrainer::new(ppo_config);
    let mut wolf_trainer = WolfPpoTrainer::new(WolfPpoConfig {
        ppo: ppo_config,
        alpha_lose: config.alpha_lose,
        ..Default::default()
    });

    let payoffs = payoff_tensor(&game, device);
    let mut nes_distances = Vec::with_capacity(config.num_policy_updates);

    for _ in 0..config.num_policy_updates {
        let obs = constant_obs(config.rollout_steps, obs_dim, device);
        let batch = collect_self_play_rollout(&game, &model, &obs);

        match config.trainer {
            MatrixTrainerKind::Ppo => {
                ppo_trainer.policy_update(&model, &mut opt, &batch);
            }
            MatrixTrainerKind::WolfPpo => {
                let payoff = tch::no_grad(|| {
                    let policy = policy_probs(&model, obs_dim, device);
                    model.expected_payoff(&unit_obs(obs_dim, device), &payoffs, &policy)
                });
                wolf_trainer.policy_update(&model, &mut opt, &batch, payoff);
            }
        }

        let learned = tch::no_grad(|| policy_probs_vec(&model, obs_dim, device));
        nes_distances.push(euclidean_distance(&learned, &game.nes));
    }

    let max_nes_distance_last_n = max_distance_last_n(&nes_distances, config.last_n);
    MatrixRunResult {
        seed: config.seed,
        nes_distances,
        max_nes_distance_last_n,
    }
}

/// Run `num_runs` experiments with seeds `base_seed..base_seed+num_runs`.
pub fn run_matrix_experiments(
    mut config: MatrixExperimentConfig,
    num_runs: usize,
    base_seed: u64,
) -> Vec<MatrixRunResult> {
    (0..num_runs)
        .map(|i| {
            config.seed = base_seed + i as u64;
            run_matrix_experiment(&config)
        })
        .collect()
}

fn payoff_tensor(game: &MatrixGame, device: Device) -> Tensor {
    let n = game.action_count() as i64;
    let flat: Vec<f64> = game
        .payoffs
        .iter()
        .flat_map(|row| row.iter().copied())
        .collect();
    Tensor::from_slice(&flat)
        .reshape(&[n, n])
        .to_kind(Kind::Float)
        .to_device(device)
}

fn constant_obs(steps: usize, obs_dim: i64, device: Device) -> Tensor {
    Tensor::ones(&[steps as i64, obs_dim], (Kind::Float, device))
}

fn unit_obs(obs_dim: i64, device: Device) -> Tensor {
    Tensor::ones(&[1, obs_dim], (Kind::Float, device))
}

fn policy_probs(model: &ActorCritic, obs_dim: i64, device: Device) -> Tensor {
    let (logits, _) = model.forward(&unit_obs(obs_dim, device));
    logits.softmax(-1, Kind::Float).squeeze()
}

fn policy_probs_vec(model: &ActorCritic, obs_dim: i64, device: Device) -> Vec<f64> {
    policy_probs(model, obs_dim, device)
        .iter::<f64>()
        .unwrap()
        .collect()
}

fn collect_self_play_rollout(
    game: &MatrixGame,
    model: &ActorCritic,
    obs: &Tensor,
) -> RolloutBatch {
    let steps = obs.size()[0];
    let device = obs.device();

    let (actions, log_probs, rewards, value_vec) = tch::no_grad(|| {
        let (actions, log_probs, values, _) = model.act(obs);
        let opp_actions = model.act(obs).0;

        let mut rewards = Vec::with_capacity(steps as usize);
        let mut value_vec = Vec::with_capacity(steps as usize);
        for i in 0..steps {
            let a = actions.int64_value(&[i]);
            let o = opp_actions.int64_value(&[i]);
            rewards.push(game.payoff(a as usize, o as usize));
            value_vec.push(values.double_value(&[i]));
        }
        (actions, log_probs, rewards, value_vec)
    });

    let (mut advantages, returns) = compute_advantages(&rewards, &value_vec, 0.0, 0.0);
    normalize_advantages(&mut advantages);

    RolloutBatch::new(
        obs.shallow_clone().detach(),
        actions.detach(),
        log_probs.detach(),
        tensor_from_vec_f64(&advantages).to_device(device),
        tensor_from_vec_f64(&returns).to_device(device),
    )
}

fn normalize_advantages(advantages: &mut [f64]) {
    if advantages.is_empty() {
        return;
    }
    let mean = advantages.iter().sum::<f64>() / advantages.len() as f64;
    let var = advantages
        .iter()
        .map(|a| (a - mean).powi(2))
        .sum::<f64>()
        / advantages.len() as f64;
    let std = var.sqrt().max(1e-8);
    for a in advantages {
        *a = (*a - mean) / std;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_smoke_run_produces_finite_nes_distance() {
        let config = MatrixExperimentConfig::smoke(
            MatrixGameKind::MatchingPenniesWeighted,
            MatrixTrainerKind::WolfPpo,
            7,
        );
        let result = run_matrix_experiment(&config);
        assert!(!result.nes_distances.is_empty());
        assert!(result.max_nes_distance_last_n.is_finite());
        assert!(result.max_nes_distance_last_n >= 0.0);
    }
}
