//! Self-play matrix-game training harness for PPO and WoLF-PPO validation.

use tch::{Device, Kind, Tensor};

use crate::ppo::{ActorCritic, OptimizerKind, PpoConfig, PpoTrainer, RolloutBatch, WolfPpoConfig, WolfPpoTrainer};

use super::matrix::{MatrixGame, MatrixGameKind};
use super::metrics::{euclidean_distance, max_distance_over_last, mean_of_run_maxima};

/// Default policy updates per run (paper uses long runs; smoke tests override).
pub const DEFAULT_POLICY_UPDATES: usize = 200;

/// Rollout batch size per policy update (simultaneous matrix-game plays).
pub const DEFAULT_ROLLOUT_BATCH_SIZE: i64 = 64;

/// Paper Table I: max distance over the last 10 policy updates per run.
pub const DEFAULT_TRACK_LAST_N: usize = 10;

/// Full paper reproduction uses 50 independent seeds.
pub const DEFAULT_NUM_RUNS: usize = 50;

/// Shared experimental setup for one self-play run.
#[derive(Debug, Clone)]
pub struct HarnessConfig {
    pub game: MatrixGameKind,
    pub seed: u64,
    pub num_policy_updates: usize,
    pub rollout_batch_size: i64,
    pub alpha_lose: f64,
    pub track_last_n: usize,
}

impl HarnessConfig {
    pub fn new(game: MatrixGameKind, seed: u64) -> Self {
        Self {
            game,
            seed,
            num_policy_updates: DEFAULT_POLICY_UPDATES,
            rollout_batch_size: DEFAULT_ROLLOUT_BATCH_SIZE,
            alpha_lose: 0.01,
            track_last_n: DEFAULT_TRACK_LAST_N,
        }
    }

    pub fn smoke(game: MatrixGameKind, seed: u64) -> Self {
        Self {
            num_policy_updates: 8,
            ..Self::new(game, seed)
        }
    }

    pub fn benchmark(game: MatrixGameKind, alpha_lose: f64, seed: u64) -> Self {
        Self {
            alpha_lose,
            num_policy_updates: DEFAULT_POLICY_UPDATES,
            ..Self::new(game, seed)
        }
    }
}

/// Outcome of one seeded self-play training run.
#[derive(Debug, Clone)]
pub struct SelfPlayRunResult {
    /// Euclidean distance from NES after each policy update (player 1 policy).
    pub distances: Vec<f64>,
    /// Max distance over the last `track_last_n` updates (Table I per-run statistic).
    pub max_distance_last_n: f64,
}

impl SelfPlayRunResult {
    pub fn final_distance(&self) -> f64 {
        self.distances.last().copied().unwrap_or(f64::NAN)
    }
}

/// Aggregate benchmark over multiple seeds.
#[derive(Debug, Clone)]
pub struct BenchmarkSummary {
    pub ppo_mean_max_distance: f64,
    pub wolf_mean_max_distance: f64,
    pub num_runs: usize,
}

/// Run standard PPO self-play on a matrix game.
pub fn run_ppo_self_play(config: &HarnessConfig) -> SelfPlayRunResult {
    let game = MatrixGame::new(config.game);
    let ppo_config = ppo_config_for_game(&game, config.alpha_lose);
    tch::manual_seed(config.seed as i64);
    let mut agent1 = PpoTrainer::new(ppo_config.clone());
    let mut agent2 = PpoTrainer::new(ppo_config);
    run_self_play(config, &game, &mut agent1, &mut agent2)
}

/// Run WoLF-PPO self-play on a matrix game.
pub fn run_wolf_ppo_self_play(config: &HarnessConfig) -> SelfPlayRunResult {
    let game = MatrixGame::new(config.game);
    let wolf_config = wolf_config_for_game(&game, config.alpha_lose);
    tch::manual_seed(config.seed as i64);
    let mut agent1 = WolfPpoTrainer::new(wolf_config.clone());
    let mut agent2 = WolfPpoTrainer::new(wolf_config);
    run_self_play(config, &game, &mut agent1, &mut agent2)
}

/// Compare PPO vs WoLF-PPO over `num_runs` seeds (paper Table I style).
pub fn benchmark_ppo_vs_wolf(
    game: MatrixGameKind,
    alpha_lose: f64,
    num_runs: usize,
) -> BenchmarkSummary {
    let mut ppo_maxima = Vec::with_capacity(num_runs);
    let mut wolf_maxima = Vec::with_capacity(num_runs);

    for run in 0..num_runs {
        let seed = 1_000 + run as u64;
        let cfg = HarnessConfig::benchmark(game, alpha_lose, seed);
        ppo_maxima.push(run_ppo_self_play(&cfg).max_distance_last_n);
        wolf_maxima.push(run_wolf_ppo_self_play(&cfg).max_distance_last_n);
    }

    BenchmarkSummary {
        ppo_mean_max_distance: mean_of_run_maxima(&ppo_maxima),
        wolf_mean_max_distance: mean_of_run_maxima(&wolf_maxima),
        num_runs,
    }
}

trait SelfPlayAgent {
    fn actor_critic(&self) -> &ActorCritic;
    fn apply_policy_update(&mut self, batch: &RolloutBatch);
}

impl SelfPlayAgent for PpoTrainer {
    fn actor_critic(&self) -> &ActorCritic {
        PpoTrainer::actor_critic(self)
    }

    fn apply_policy_update(&mut self, batch: &RolloutBatch) {
        let _ = self.policy_update(batch);
    }
}

impl SelfPlayAgent for WolfPpoTrainer {
    fn actor_critic(&self) -> &ActorCritic {
        WolfPpoTrainer::actor_critic(self)
    }

    fn apply_policy_update(&mut self, batch: &RolloutBatch) {
        let _ = self.policy_update(batch);
    }
}

fn ppo_config_for_game(game: &MatrixGame, alpha_lose: f64) -> PpoConfig {
    PpoConfig {
        obs_dim: game.obs_dim(),
        num_actions: game.num_actions() as i64,
        hidden_dims: vec![20, 20],
        clip_eps: 0.2,
        value_coef: 0.5,
        entropy_coef: 0.01,
        ppo_epochs: 4,
        learning_rate: alpha_lose,
        optimizer: OptimizerKind::Sgd,
    }
}

fn wolf_config_for_game(game: &MatrixGame, alpha_lose: f64) -> WolfPpoConfig {
    WolfPpoConfig {
        ppo: ppo_config_for_game(game, alpha_lose),
        alpha_lose,
        win_lose_ratio: 4.0,
    }
}

fn run_self_play<A1, A2>(
    config: &HarnessConfig,
    game: &MatrixGame,
    agent1: &mut A1,
    agent2: &mut A2,
) -> SelfPlayRunResult
where
    A1: SelfPlayAgent,
    A2: SelfPlayAgent,
{
    let device = Device::Cpu;
    let obs_dim = game.obs_dim();
    let batch = config.rollout_batch_size;
    let mut distances = Vec::with_capacity(config.num_policy_updates);

    for _ in 0..config.num_policy_updates {
        let obs = Tensor::zeros([batch, obs_dim], (Kind::Float, device));

        let (actions1, log_probs1, values1) = agent1.actor_critic().act(&obs);
        let (actions2, log_probs2, values2) = agent2.actor_critic().act(&obs);

        let rewards1 = batch_rewards(game, &actions1, &actions2, true);
        let rewards2 = batch_rewards(game, &actions1, &actions2, false);

        let returns1 = rewards1.shallow_clone();
        let returns2 = rewards2.shallow_clone();
        let advantages1 = &returns1 - &values1.detach();
        let advantages2 = &returns2 - &values2.detach();

        let batch1 = RolloutBatch {
            observations: obs.shallow_clone(),
            actions: actions1.detach(),
            old_log_probs: log_probs1.detach(),
            returns: returns1,
            advantages: advantages1,
            rewards: rewards1,
        };
        let batch2 = RolloutBatch {
            observations: obs,
            actions: actions2.detach(),
            old_log_probs: log_probs2.detach(),
            returns: returns2,
            advantages: advantages2,
            rewards: rewards2,
        };

        agent1.apply_policy_update(&batch1);
        agent2.apply_policy_update(&batch2);

        let probs = policy_probabilities(agent1.actor_critic(), game);
        distances.push(euclidean_distance(&probs, game.nes_probabilities()));
    }

    let max_distance_last_n = max_distance_over_last(&distances, config.track_last_n);
    SelfPlayRunResult {
        distances,
        max_distance_last_n,
    }
}

fn batch_rewards(
    game: &MatrixGame,
    row_actions: &Tensor,
    col_actions: &Tensor,
    for_row_player: bool,
) -> Tensor {
    let rows = tensor_to_i64_vec(row_actions);
    let cols = tensor_to_i64_vec(col_actions);
    let rewards: Vec<f32> = rows
        .iter()
        .zip(cols.iter())
        .map(|(&r, &c)| {
            let payoff = if for_row_player {
                game.row_payoff(r as usize, c as usize)
            } else {
                game.col_payoff(r as usize, c as usize)
            };
            payoff as f32
        })
        .collect();
    Tensor::from_slice(&rewards)
}

fn tensor_to_i64_vec(tensor: &Tensor) -> Vec<i64> {
    let flat = tensor.to_kind(Kind::Int64).flatten(0, -1);
    (0..flat.size()[0])
        .map(|i| flat.int64_value(&[i]))
        .collect()
}

fn policy_probabilities(actor_critic: &ActorCritic, game: &MatrixGame) -> Vec<f64> {
    let obs = Tensor::zeros([1, game.obs_dim()], (Kind::Float, Device::Cpu));
    let (_, logits) = actor_critic.forward(&obs);
    let probs = logits.softmax(-1, Kind::Float);
    let flat = probs.squeeze();
    (0..flat.size()[0])
        .map(|i| flat.double_value(&[i]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_wolf_ppo_produces_finite_nes_distance() {
        let config = HarnessConfig::smoke(MatrixGameKind::WeightedMatchingPennies, 42);
        let result = run_wolf_ppo_self_play(&config);
        assert!(result.final_distance().is_finite());
        assert!(result.max_distance_last_n.is_finite());
        assert_eq!(result.distances.len(), config.num_policy_updates);
    }

    #[test]
    fn smoke_ppo_produces_finite_nes_distance() {
        let config = HarnessConfig::smoke(MatrixGameKind::MatchingPennies, 7);
        let result = run_ppo_self_play(&config);
        assert!(result.final_distance().is_finite());
        assert!(result.max_distance_last_n.is_finite());
    }
}
