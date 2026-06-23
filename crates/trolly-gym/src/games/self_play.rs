//! Self-play training loop driving WP-018 PPO / WoLF-PPO on matrix games.

use tch::{kind::Kind, Device, Tensor};

use crate::ppo::{
    ActorCritic, OptimizerKind, PpoConfig, PpoTrainer, RolloutBatch, WolfPpoConfig, WolfPpoTrainer,
};

use super::matrix::{MatrixGame, MatrixGameKind};
use super::nes::{max_distance_over_last_n, mean_max_distance_last_n, NesPolicy};

/// Which WP-018 trainer drives self-play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixTrainerKind {
    Ppo,
    WolfPpo,
}

/// Shared experimental setup for matrix-game runs (paper Table I).
#[derive(Debug, Clone)]
pub struct MatrixExperimentConfig {
    pub game: MatrixGameKind,
    pub trainer: MatrixTrainerKind,
    /// WoLF α_LOSE and PPO learning rate for fair comparison (paper Table I).
    pub alpha_lose: f64,
    /// Policy updates per run.
    pub num_updates: usize,
    /// Simultaneous plays collected per update per agent.
    pub batch_size: i64,
    /// Random seed for weight init and action sampling.
    pub seed: u64,
    /// Window for max-distance metric (paper: last 10 updates).
    pub distance_window: usize,
    pub ppo: PpoConfig,
}

impl MatrixExperimentConfig {
    /// Paper-aligned defaults: SGD, hidden `[20, 20]`, 4 PPO epochs.
    pub fn paper_default(game: MatrixGameKind, trainer: MatrixTrainerKind, alpha_lose: f64) -> Self {
        Self {
            game,
            trainer,
            alpha_lose,
            num_updates: 500,
            batch_size: 64,
            seed: 0,
            distance_window: 10,
            ppo: PpoConfig {
                learning_rate: alpha_lose,
                hidden_layers: vec![20, 20],
                optimizer: OptimizerKind::Sgd,
                ..PpoConfig::default()
            },
        }
    }

    /// Short config for smoke tests and CI.
    pub fn smoke(game: MatrixGameKind, seed: u64) -> Self {
        Self {
            game,
            trainer: MatrixTrainerKind::WolfPpo,
            alpha_lose: 0.1,
            num_updates: 8,
            batch_size: 32,
            seed,
            distance_window: 10,
            ppo: PpoConfig {
                learning_rate: 0.1,
                ppo_epochs: 2,
                hidden_layers: vec![20, 20],
                optimizer: OptimizerKind::Sgd,
                ..PpoConfig::default()
            },
        }
    }
}

/// Outcome of one self-play run.
#[derive(Debug, Clone)]
pub struct MatrixRunResult {
    /// Euclidean NES distance after each policy update (row player).
    pub distances: Vec<f64>,
    /// Max distance over the last `distance_window` updates (Table I).
    pub max_distance_last_n: f64,
}

/// Aggregate over multiple seeded runs.
#[derive(Debug, Clone)]
pub struct MatrixBenchmarkResult {
    pub run_max_distances: Vec<f64>,
    pub mean_max_distance: f64,
}

/// Run self-play for both players with the configured trainer.
pub fn run_self_play(config: &MatrixExperimentConfig) -> MatrixRunResult {
    tch::manual_seed(config.seed as i64);
    let game = MatrixGame::from_kind(config.game);
    let nes = NesPolicy::for_game(&game);
    let n_actions = game.n_actions() as i64;
    let obs_dim = 1_i64;
    let obs = constant_obs(config.batch_size, obs_dim);

    let mut row = build_trainer(config, obs_dim, n_actions);
    let mut col = build_trainer(config, obs_dim, n_actions);

    let mut distances = Vec::with_capacity(config.num_updates);

    for _ in 0..config.num_updates {
        update_player(
            &mut row,
            actor_critic_ref(&col),
            &game,
            &obs,
            config.batch_size,
            true,
        );
        update_player(
            &mut col,
            actor_critic_ref(&row),
            &game,
            &obs,
            config.batch_size,
            false,
        );

        let probs = policy_probabilities(actor_critic_ref(&row), &constant_obs(1, obs_dim));
        distances.push(nes.euclidean_distance(&probs));
    }

    let max_distance_last_n = max_distance_over_last_n(&distances, config.distance_window);

    MatrixRunResult {
        distances,
        max_distance_last_n,
    }
}

/// Run `num_runs` independent seeds and aggregate Table I statistics.
pub fn run_benchmark(config: &MatrixExperimentConfig, num_runs: usize) -> MatrixBenchmarkResult {
    let mut run_max_distances = Vec::with_capacity(num_runs);
    for run_idx in 0..num_runs {
        let mut run_config = config.clone();
        run_config.seed = config.seed.wrapping_add(run_idx as u64);
        let result = run_self_play(&run_config);
        run_max_distances.push(result.max_distance_last_n);
    }
    let mean_max_distance = mean_max_distance_last_n(&run_max_distances);
    MatrixBenchmarkResult {
        run_max_distances,
        mean_max_distance,
    }
}

enum TrainerHandle {
    Ppo(PpoTrainer),
    Wolf(WolfPpoTrainer),
}

fn build_trainer(config: &MatrixExperimentConfig, obs_dim: i64, n_actions: i64) -> TrainerHandle {
    match config.trainer {
        MatrixTrainerKind::Ppo => {
            let mut ppo = config.ppo.clone();
            ppo.learning_rate = config.alpha_lose;
            TrainerHandle::Ppo(PpoTrainer::new(obs_dim, n_actions, ppo))
        }
        MatrixTrainerKind::WolfPpo => {
            let wolf = WolfPpoConfig {
                ppo: config.ppo.clone(),
                alpha_lose: config.alpha_lose,
                win_lose_ratio: 4.0,
            };
            TrainerHandle::Wolf(WolfPpoTrainer::new(obs_dim, n_actions, wolf))
        }
    }
}

fn actor_critic_ref(trainer: &TrainerHandle) -> &ActorCritic {
    match trainer {
        TrainerHandle::Ppo(t) => &t.actor_critic,
        TrainerHandle::Wolf(t) => t.actor_critic(),
    }
}

fn update_player(
    player: &mut TrainerHandle,
    opponent: &ActorCritic,
    game: &MatrixGame,
    obs: &Tensor,
    batch_size: i64,
    is_row: bool,
) {
    let batch = collect_rollout(player, opponent, game, obs, batch_size, is_row);
    match player {
        TrainerHandle::Ppo(t) => {
            t.policy_update(&batch);
        }
        TrainerHandle::Wolf(t) => {
            t.policy_update(&batch);
        }
    }
}

fn collect_rollout(
    player: &TrainerHandle,
    opponent: &ActorCritic,
    game: &MatrixGame,
    obs: &Tensor,
    batch_size: i64,
    is_row: bool,
) -> RolloutBatch {
    let ac = actor_critic_ref(player);

    let (player_actions, old_log_probs, values) = sample_actions(ac, obs, batch_size);
    let opponent_actions = sample_actions_only(opponent, obs, batch_size);

    let rewards: Vec<f64> = (0..batch_size)
        .map(|i| {
            let row = if is_row {
                player_actions.int64_value(&[i])
            } else {
                opponent_actions.int64_value(&[i])
            };
            let col = if is_row {
                opponent_actions.int64_value(&[i])
            } else {
                player_actions.int64_value(&[i])
            };
            if is_row {
                game.row_payoff(row, col)
            } else {
                game.col_payoff(row, col)
            }
        })
        .collect();

    let returns = Tensor::from_slice(&rewards).to_device(Device::Cpu);
    let advantages = &returns - values.detach();
    let mean_reward = rewards.iter().sum::<f64>() / rewards.len() as f64;

    RolloutBatch {
        obs: obs.shallow_clone(),
        actions: player_actions,
        old_log_probs,
        advantages,
        returns,
        mean_reward,
    }
}

fn constant_obs(batch_size: i64, obs_dim: i64) -> Tensor {
    Tensor::zeros([batch_size, obs_dim], (Kind::Float, Device::Cpu))
}

fn sample_actions(
    ac: &ActorCritic,
    obs: &Tensor,
    _batch_size: i64,
) -> (Tensor, Tensor, Tensor) {
    tch::no_grad(|| {
        let (logits, values) = ac.forward(obs);
        let log_probs_all = logits.log_softmax(-1, Kind::Float);
        let probs = logits.softmax(-1, Kind::Float);
        let actions = probs.multinomial(1, true).squeeze_dim(1);
        let log_probs = log_probs_all
            .gather(1, &actions.unsqueeze(1), false)
            .squeeze_dim(1);
        (actions, log_probs, values.squeeze_dim(1))
    })
}

fn sample_actions_only(ac: &ActorCritic, obs: &Tensor, _batch_size: i64) -> Tensor {
    tch::no_grad(|| {
        let (logits, _) = ac.forward(obs);
        let probs = logits.softmax(-1, Kind::Float);
        probs.multinomial(1, true).squeeze_dim(1)
    })
}

/// Softmax policy probabilities for a batch-1 observation.
pub fn policy_probabilities(ac: &ActorCritic, obs: &Tensor) -> Vec<f64> {
    tch::no_grad(|| {
        let (logits, _) = ac.forward(obs);
        let probs = logits.softmax(-1, Kind::Float);
        let n = probs.size()[1];
        (0..n)
            .map(|i| f64::try_from(&probs.select(1, i as i64).select(0, 0)).unwrap_or(0.0))
            .collect()
    })
}
