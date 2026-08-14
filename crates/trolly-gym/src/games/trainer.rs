//! Self-play training harness for two-player zero-sum matrix games.
//!
//! Use [`run_wolf_ppo_self_play_with_checkpoints`] for short development runs
//! that persist row-player weights after each update.
//!
//! Drives PPO and WoLF-PPO agents (from WP-018 `crate::ppo`) on matrix games
//! and measures convergence to the known Nash Equilibrium Strategy (NES).
//!
//! ## Methodology (paper Table I)
//!
//! For each run:
//! 1. Both players are initialised with fresh networks and optimisers.
//! 2. At each update step, a batch of `batch_size` self-play interactions is
//!    collected.  The observation is a constant zero vector — appropriate for
//!    a single-state normal-form game.
//! 3. Advantages are centred: `adv_t = r_t − mean(r)`.
//! 4. After the PPO / WoLF-PPO gradient step, the row player's current
//!    policy probabilities are extracted via softmax of the actor logits.
//! 5. Euclidean distance to the known NES is recorded for every update.
//! 6. The reported metric is **max distance over the last 10 updates** per run
//!    (paper Table I methodology).

use std::path::{Path, PathBuf};

use tch::{Device, Kind, Tensor};

use crate::ppo::{ActorCritic, PpoConfig, PpoTrainer, RolloutBatch, WolfPpoConfig, WolfPpoTrainer};
use crate::train::checkpoint::{
    load_checkpoint_if_exists, resolve_resume_checkpoint, save_checkpoint,
    save_checkpoint_with_fingerprint, FINAL_CHECKPOINT, FINAL_ROW_CHECKPOINT, LATEST_CHECKPOINT,
    LATEST_OPPONENT_CHECKPOINT,
};
use super::matrix_game::MatrixGame;
use super::metrics::euclidean_distance_to_nes;

/// Observation dimension for matrix games.
///
/// A single-state normal-form game has no meaningful observation; the
/// constant zero scalar forces the network to encode the strategy in its
/// bias terms and higher-layer weights.
const OBS_DIM: i64 = 1;

/// Configuration for a self-play training run.
#[derive(Debug, Clone)]
pub struct SelfPlayConfig {
    /// Number of policy update iterations.
    pub num_updates: usize,
    /// Number of self-play interactions collected per update (rollout batch size).
    pub batch_size: usize,
    /// PPO hyperparameters shared by both players.
    pub ppo_config: PpoConfig,
}

impl Default for SelfPlayConfig {
    fn default() -> Self {
        Self {
            num_updates: 200,
            batch_size: 64,
            ppo_config: PpoConfig::default(),
        }
    }
}

/// Result of a single self-play training run.
#[derive(Debug)]
pub struct SelfPlayResult {
    /// Maximum Euclidean distance from the NES over the **last 10** policy
    /// updates (paper Table I reporting methodology).
    ///
    /// If fewer than 10 updates were run, the maximum over all updates is used.
    pub max_distance_last_10: f64,
    /// Row player's policy probabilities at the end of training.
    pub final_policy_probs: Vec<f64>,
    /// Distance from NES recorded after each update step (full history).
    pub distances_per_update: Vec<f64>,
}

/// Train both players with standard PPO and return the row player's result.
///
/// Both `p1` (row) and `p2` (column) use the same `config.ppo_config`.
/// `p2`'s payoff is the negation of `p1`'s (zero-sum).
pub fn run_ppo_self_play(
    game: &MatrixGame,
    nes: &[f64],
    config: SelfPlayConfig,
) -> SelfPlayResult {
    let num_actions = game.num_row_actions as i64;
    let mut p1 = PpoTrainer::new(OBS_DIM, num_actions, config.ppo_config.clone());
    let mut p2 = PpoTrainer::new(OBS_DIM, num_actions, config.ppo_config);
    let mut distances = Vec::with_capacity(config.num_updates);

    for _ in 0..config.num_updates {
        let (batch1, batch2) =
            collect_rollout(game, &p1.actor_critic, &p2.actor_critic, config.batch_size);
        p1.policy_update(&batch1);
        p2.policy_update(&batch2);

        let probs = policy_probs(&p1.actor_critic);
        distances.push(euclidean_distance_to_nes(&probs, nes));
    }

    to_result(&p1.actor_critic, distances)
}

/// Train both players with WoLF-PPO and return the row player's result.
///
/// `wolf_config` is applied to both players.  The episode return passed to
/// `WolfPpoTrainer::policy_update` is the mean batch payoff for that player.
pub fn run_wolf_ppo_self_play(
    game: &MatrixGame,
    nes: &[f64],
    config: SelfPlayConfig,
    wolf_config: WolfPpoConfig,
) -> SelfPlayResult {
    let num_actions = game.num_row_actions as i64;
    let mut p1 = WolfPpoTrainer::new(OBS_DIM, num_actions, wolf_config.clone());
    let mut p2 = WolfPpoTrainer::new(OBS_DIM, num_actions, wolf_config);
    let mut distances = Vec::with_capacity(config.num_updates);

    for _ in 0..config.num_updates {
        let (batch1, batch2) = collect_rollout(
            game,
            &p1.inner.actor_critic,
            &p2.inner.actor_critic,
            config.batch_size,
        );

        let p1_return = batch1.returns.mean(Kind::Float).double_value(&[]);
        let p2_return = batch2.returns.mean(Kind::Float).double_value(&[]);

        p1.policy_update(&batch1, p1_return);
        p2.policy_update(&batch2, p2_return);

        let probs = policy_probs(&p1.inner.actor_critic);
        distances.push(euclidean_distance_to_nes(&probs, nes));
    }

    to_result(&p1.inner.actor_critic, distances)
}

/// Train with WoLF-PPO and save the row player's weights after each update.
///
/// Checkpoints are written to `{checkpoint_dir}/update_{n}.safetensors`.
/// Returns the training result and the list of checkpoint paths.
///
/// Each call starts from scratch. For continuous training across timed loops,
/// use [`WolfPpoSelfPlaySession`].
pub fn run_wolf_ppo_self_play_with_checkpoints(
    game: &MatrixGame,
    nes: &[f64],
    config: SelfPlayConfig,
    wolf_config: WolfPpoConfig,
    checkpoint_dir: impl AsRef<Path>,
) -> (SelfPlayResult, Vec<PathBuf>) {
    let checkpoint_dir = checkpoint_dir.as_ref();
    std::fs::create_dir_all(checkpoint_dir).expect("create checkpoint dir");

    let mut session = WolfPpoSelfPlaySession::new(game, &config, wolf_config);
    let distances = session.run_updates(game, nes, config.num_updates, checkpoint_dir, true);
    let checkpoint_paths: Vec<PathBuf> = (0..config.num_updates)
        .map(|update| checkpoint_dir.join(format!("update_{update}.safetensors")))
        .collect();

    (to_result(&session.p1.inner.actor_critic, distances), checkpoint_paths)
}

/// Long-lived WoLF-PPO self-play session that can resume from checkpoints.
///
/// Saves `latest.safetensors` (row player) and `latest_opponent.safetensors`
/// after each update. On resume, loads both when present.
pub struct WolfPpoSelfPlaySession {
    pub p1: WolfPpoTrainer,
    pub p2: WolfPpoTrainer,
    batch_size: usize,
    pub update_count: usize,
}

impl WolfPpoSelfPlaySession {
    /// Create a fresh session with new row and column players.
    pub fn new(
        game: &MatrixGame,
        config: &SelfPlayConfig,
        wolf_config: WolfPpoConfig,
    ) -> Self {
        Self::new_on_device(game, config, wolf_config, Device::Cpu)
    }

    /// Create a fresh session with both players on `device`.
    pub fn new_on_device(
        game: &MatrixGame,
        config: &SelfPlayConfig,
        wolf_config: WolfPpoConfig,
        device: Device,
    ) -> Self {
        let num_actions = game.num_row_actions as i64;
        Self {
            p1: WolfPpoTrainer::new_on_device(OBS_DIM, num_actions, wolf_config.clone(), device),
            p2: WolfPpoTrainer::new_on_device(OBS_DIM, num_actions, wolf_config, device),
            batch_size: config.batch_size,
            update_count: 0,
        }
    }

    /// Create a session and load `latest` / `final` row weights plus opponent
    /// weights from `checkpoint_dir` when those files exist.
    pub fn resume_from(
        checkpoint_dir: impl AsRef<Path>,
        game: &MatrixGame,
        config: &SelfPlayConfig,
        wolf_config: WolfPpoConfig,
    ) -> Self {
        Self::resume_from_on_device(checkpoint_dir, game, config, wolf_config, Device::Cpu)
    }

    pub fn resume_from_on_device(
        checkpoint_dir: impl AsRef<Path>,
        game: &MatrixGame,
        config: &SelfPlayConfig,
        wolf_config: WolfPpoConfig,
        device: Device,
    ) -> Self {
        let checkpoint_dir = checkpoint_dir.as_ref();
        std::fs::create_dir_all(checkpoint_dir).expect("create checkpoint dir");
        let mut session = Self::new_on_device(game, config, wolf_config, device);
        if let Some(path) = resolve_resume_checkpoint(checkpoint_dir) {
            load_checkpoint_if_exists(&mut session.p1.inner.vs, &path);
        }
        let opponent = checkpoint_dir.join(LATEST_OPPONENT_CHECKPOINT);
        load_checkpoint_if_exists(&mut session.p2.inner.vs, &opponent);
        session
    }

    /// Run `num_updates` policy updates, optionally saving numbered checkpoints.
    ///
    /// Always writes `latest.safetensors` and `latest_opponent.safetensors`.
    pub fn run_updates(
        &mut self,
        game: &MatrixGame,
        nes: &[f64],
        num_updates: usize,
        checkpoint_dir: &Path,
        save_numbered: bool,
    ) -> Vec<f64> {
        std::fs::create_dir_all(checkpoint_dir).expect("create checkpoint dir");
        let mut distances = Vec::with_capacity(num_updates);

        for _ in 0..num_updates {
            let (batch1, batch2) = collect_rollout(
                game,
                &self.p1.inner.actor_critic,
                &self.p2.inner.actor_critic,
                self.batch_size,
            );

            let p1_return = batch1.returns.mean(Kind::Float).double_value(&[]);
            let p2_return = batch2.returns.mean(Kind::Float).double_value(&[]);

            self.p1.policy_update(&batch1, p1_return);
            self.p2.policy_update(&batch2, p2_return);

            let probs = policy_probs(&self.p1.inner.actor_critic);
            distances.push(euclidean_distance_to_nes(&probs, nes));
            self.update_count += 1;

            if save_numbered {
                let path = checkpoint_dir
                    .join(format!("update_{}.safetensors", self.update_count - 1));
                save_checkpoint(&self.p1.inner.vs, &path).expect("save numbered checkpoint");
            }

            save_checkpoint_with_fingerprint(
                &self.p1.inner.vs,
                &checkpoint_dir.join(LATEST_CHECKPOINT),
                &format!("matrix:{}", checkpoint_dir.display()),
                "wolf-ppo-self-play",
            )
            .expect("save latest row checkpoint + fingerprint");
            save_checkpoint(
                &self.p2.inner.vs,
                &checkpoint_dir.join(LATEST_OPPONENT_CHECKPOINT),
            )
            .expect("save latest opponent checkpoint");
        }

        distances
    }

    /// Copy latest row weights to `final.safetensors` and `final_row_player.safetensors`.
    pub fn finalize_checkpoints(&self, checkpoint_dir: &Path) {
        let latest = checkpoint_dir.join(LATEST_CHECKPOINT);
        if !latest.exists() {
            return;
        }
        for name in [FINAL_CHECKPOINT, FINAL_ROW_CHECKPOINT] {
            std::fs::copy(&latest, checkpoint_dir.join(name)).expect("copy final checkpoint");
        }
    }
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Collect one rollout batch for both players via self-play.
///
/// The observation is a `[batch_size, OBS_DIM]` tensor of zeros — appropriate
/// for a single-state game.  Column player's payoff is the negation of the row
/// player's (zero-sum).
fn collect_rollout(
    game: &MatrixGame,
    p1_ac: &ActorCritic,
    p2_ac: &ActorCritic,
    batch_size: usize,
) -> (RolloutBatch, RolloutBatch) {
    let t = batch_size as i64;
    let device = p1_ac.device();
    let obs = Tensor::zeros(&[t, OBS_DIM], (Kind::Float, device));

    let (p1_actions, p1_log_probs) = p1_ac.action_and_log_prob(&obs);
    let (p2_actions, p2_log_probs) = p2_ac.action_and_log_prob(&obs);

    let mut p1_pay = vec![0.0_f32; batch_size];
    let mut p2_pay = vec![0.0_f32; batch_size];
    for i in 0..batch_size {
        let a1 = p1_actions.int64_value(&[i as i64]) as usize;
        let a2 = p2_actions.int64_value(&[i as i64]) as usize;
        let r = game.payoff_for(a1, a2) as f32;
        p1_pay[i] = r;
        p2_pay[i] = -r;
    }

    let p1_ret = Tensor::from_slice(&p1_pay).to_device(device);
    let p2_ret = Tensor::from_slice(&p2_pay).to_device(device);
    let p1_adv = centred_advantage(&p1_ret);
    let p2_adv = centred_advantage(&p2_ret);

    // Detach old_log_probs so they are treated as fixed constants in the PPO
    // loss.  Without detach, the behaviour-policy graph is freed after epoch-1
    // backward and epoch-2 fails with "saved tensors already freed".
    (
        RolloutBatch {
            observations: obs.shallow_clone(),
            actions: p1_actions,
            old_log_probs: p1_log_probs.detach(),
            returns: p1_ret,
            advantages: p1_adv,
        },
        RolloutBatch {
            observations: obs,
            actions: p2_actions,
            old_log_probs: p2_log_probs.detach(),
            returns: p2_ret,
            advantages: p2_adv,
        },
    )
}

/// Monte Carlo baseline: `adv_t = r_t − mean(r)`.
fn centred_advantage(returns: &Tensor) -> Tensor {
    let mean = returns.mean(Kind::Float);
    returns - &mean
}

/// Extract softmax policy probabilities from the actor for the single zero state.
fn policy_probs(ac: &ActorCritic) -> Vec<f64> {
    let obs = Tensor::zeros(&[1, OBS_DIM], (Kind::Float, ac.device()));
    let (logits, _) = ac.forward(&obs);
    let probs = logits.softmax(-1, Kind::Float).squeeze();
    let n = probs.size()[0] as usize;
    (0..n)
        .map(|i| probs.double_value(&[i as i64]))
        .collect()
}

/// Aggregate distance history into a [`SelfPlayResult`].
fn to_result(ac: &ActorCritic, distances: Vec<f64>) -> SelfPlayResult {
    let last_10_start = distances.len().saturating_sub(10);
    let max_distance_last_10 = distances[last_10_start..]
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    // Guard against empty distance history (num_updates == 0).
    let max_distance_last_10 = if max_distance_last_10.is_infinite() {
        0.0
    } else {
        max_distance_last_10
    };

    SelfPlayResult {
        max_distance_last_10,
        final_policy_probs: policy_probs(ac),
        distances_per_update: distances,
    }
}
