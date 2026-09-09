//! WoLF-PPO path for the WP-033 tanh-Gaussian inventory policy.
//!
//! On-policy transitions store `action: f32` (target inventory). Matrix-game
//! self-play and live `PolicyProvider` stay on the categorical 3-logit path.

use std::path::{Path, PathBuf};

use tch::{nn, nn::OptimizerConfig, Device, Kind, Tensor};

use crate::ppo::{
    GaussianActorCritic, PpoConfig, RolloutBatch, RungLiquidGaussian, WolfPpoConfig,
};
use crate::sim::{
    clamp_inventory_target, run_episode_with_targets, MicrostructureConfig, MicrostructureSim,
    MicrostructureStats,
};

use super::checkpoint::{
    load_checkpoint_if_exists, resolve_resume_checkpoint, save_checkpoint,
    save_checkpoint_with_fingerprint, FINAL_CHECKPOINT, LATEST_CHECKPOINT,
};
use super::driver::TrainMetrics;
use super::rollout::{compute_gae, OnPolicyTransition, StepOutput};

/// Host fossils of the old 3-logit unit-lot microstructure policy.
///
/// Do not resume these weights: the Gaussian ladder head cannot load them.
pub const RETIRED_UNIT_LOT_MICROSTRUCTURE: &str = "_retired_unit_lot_microstructure";

/// Checkpoint architecture directory for the Gaussian MLP ladder policy.
pub const GAUSSIAN_MLP_ARCH: &str = "gaussian_mlp";

/// Checkpoint architecture directory for the WP-034 Liquid-on-rungs Gaussian.
pub const GAUSSIAN_LIQUID_ARCH: &str = "gaussian_liquid";

/// Function-approximator kind for the continuous inventory policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GaussianArchitecture {
    /// Flattened-ladder MLP (WP-033).
    Mlp,
    /// Liquid unrolled along rungs `v` (WP-034).
    LiquidRungs,
}

impl Default for GaussianArchitecture {
    fn default() -> Self {
        Self::Mlp
    }
}

/// Shared tanh-Gaussian actor used by the ladder trainer.
pub enum GaussianPolicy {
    Mlp(GaussianActorCritic),
    LiquidRungs(RungLiquidGaussian),
}

impl GaussianPolicy {
    pub fn device(&self) -> Device {
        match self {
            Self::Mlp(m) => m.device(),
            Self::LiquidRungs(m) => m.device(),
        }
    }

    pub fn mean_action(&self, obs: &Tensor) -> Tensor {
        match self {
            Self::Mlp(m) => m.mean_action(obs),
            Self::LiquidRungs(m) => m.mean_action(obs),
        }
    }

    pub fn action_and_log_prob(&self, obs: &Tensor) -> (Tensor, Tensor, Tensor) {
        match self {
            Self::Mlp(m) => m.action_and_log_prob(obs),
            Self::LiquidRungs(m) => m.action_and_log_prob(obs),
        }
    }

    pub fn evaluate_actions(&self, obs: &Tensor, actions: &Tensor) -> (Tensor, Tensor) {
        match self {
            Self::Mlp(m) => m.evaluate_actions(obs, actions),
            Self::LiquidRungs(m) => m.evaluate_actions(obs, actions),
        }
    }

    pub fn forward(&self, obs: &Tensor) -> (Tensor, Tensor, Tensor) {
        match self {
            Self::Mlp(m) => m.forward(obs),
            Self::LiquidRungs(m) => m.forward(obs),
        }
    }
}

/// `true` when `path` is under the retired unit-lot fossil tree.
pub fn is_retired_unit_lot_dir(path: impl AsRef<Path>) -> bool {
    path.as_ref().components().any(|c| {
        c.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(RETIRED_UNIT_LOT_MICROSTRUCTURE)
    })
}

/// Continuous-action transition; GAE reuses the discrete helper via a shim.
#[derive(Debug, Clone)]
pub struct ContinuousOnPolicyTransition {
    pub observation: Vec<f32>,
    pub action: f32,
    pub log_prob: f32,
    pub value: f32,
    pub reward: f32,
    pub done: bool,
}

/// Collects `horizon` tanh-Gaussian transitions (`action: f32`).
pub struct GaussianRolloutCollector {
    transitions: Vec<ContinuousOnPolicyTransition>,
    horizon: usize,
    gamma: f64,
    gae_lambda: f64,
}

impl GaussianRolloutCollector {
    pub fn new(horizon: usize, gamma: f64, gae_lambda: f64) -> Self {
        Self {
            transitions: Vec::with_capacity(horizon),
            horizon,
            gamma,
            gae_lambda,
        }
    }

    pub fn collect<F>(
        &mut self,
        actor_critic: &GaussianPolicy,
        initial_obs: Vec<f32>,
        mut env_step: F,
    ) where
        F: FnMut(Vec<f32>, f32) -> StepOutput,
    {
        self.transitions.clear();
        let obs_dim = initial_obs.len() as i64;
        let mut current_obs = initial_obs;

        for _ in 0..self.horizon {
            let obs_t = Tensor::from_slice(&current_obs)
                .unsqueeze(0)
                .to_kind(Kind::Float)
                .to_device(actor_critic.device());

            let (action, log_prob, value_t) = {
                let _no_grad = tch::no_grad_guard();
                actor_critic.action_and_log_prob(&obs_t)
            };

            let action_f = clamp_inventory_target(action.double_value(&[]) as f32);
            let log_prob_f = log_prob.double_value(&[]) as f32;
            let value_f = value_t.double_value(&[]) as f32;

            let out = env_step(current_obs.clone(), action_f);
            self.transitions.push(ContinuousOnPolicyTransition {
                observation: current_obs,
                action: action_f,
                log_prob: log_prob_f,
                value: value_f,
                reward: out.reward,
                done: out.done,
            });

            current_obs = out.next_observation;
            if out.done {
                current_obs = vec![0.0_f32; obs_dim as usize];
            }
        }
    }

    pub fn into_batch_on(&self, bootstrap_value: f32, device: Device) -> RolloutBatch {
        let t = self.transitions.len();
        assert!(t > 0, "GaussianRolloutCollector: no transitions collected");

        let shims: Vec<OnPolicyTransition> = self
            .transitions
            .iter()
            .map(|tr| OnPolicyTransition {
                observation: tr.observation.clone(),
                action: 0,
                log_prob: tr.log_prob,
                value: tr.value,
                reward: tr.reward,
                done: tr.done,
            })
            .collect();
        let (returns, advantages) = compute_gae(&shims, bootstrap_value, self.gamma, self.gae_lambda);

        let obs_dim = self.transitions[0].observation.len() as i64;
        let obs_flat: Vec<f32> = self
            .transitions
            .iter()
            .flat_map(|tr| tr.observation.iter().copied())
            .collect();
        let actions_vec: Vec<f32> = self.transitions.iter().map(|tr| tr.action).collect();
        let log_probs_vec: Vec<f32> = self.transitions.iter().map(|tr| tr.log_prob).collect();

        RolloutBatch {
            observations: Tensor::from_slice(&obs_flat)
                .reshape(&[t as i64, obs_dim])
                .to_kind(Kind::Float)
                .to_device(device),
            actions: Tensor::from_slice(&actions_vec)
                .to_kind(Kind::Float)
                .to_device(device),
            old_log_probs: Tensor::from_slice(&log_probs_vec)
                .to_kind(Kind::Float)
                .to_device(device),
            returns: Tensor::from_slice(&returns)
                .to_kind(Kind::Float)
                .to_device(device),
            advantages: Tensor::from_slice(&advantages)
                .to_kind(Kind::Float)
                .to_device(device),
        }
    }

    pub fn len(&self) -> usize {
        self.transitions.len()
    }
}

/// PPO trainer whose actor is a tanh-Gaussian MLP (same L^CLIP as discrete).
pub struct GaussianPpoTrainer {
    pub vs: nn::VarStore,
    pub actor_critic: GaussianPolicy,
    pub config: PpoConfig,
    pub optimizer: nn::Optimizer,
}

impl GaussianPpoTrainer {
    pub fn new_on_device(
        obs_dim: i64,
        config: PpoConfig,
        architecture: GaussianArchitecture,
        rung_count: i64,
        device: Device,
    ) -> Self {
        let vs = nn::VarStore::new(device);
        let actor_critic = match architecture {
            GaussianArchitecture::Mlp => {
                GaussianPolicy::Mlp(GaussianActorCritic::new(&vs, obs_dim, &config))
            }
            GaussianArchitecture::LiquidRungs => GaussianPolicy::LiquidRungs(
                RungLiquidGaussian::new(&vs, obs_dim, rung_count, &config),
            ),
        };
        let optimizer = if config.use_adam {
            nn::Adam::default()
                .build(&vs, config.lr)
                .expect("build Adam optimizer")
        } else {
            nn::Sgd::default()
                .build(&vs, config.lr)
                .expect("build SGD optimizer")
        };
        Self {
            vs,
            actor_critic,
            config,
            optimizer,
        }
    }

    pub fn device(&self) -> Device {
        self.vs.device()
    }

    pub fn set_lr(&mut self, lr: f64) {
        self.optimizer.set_lr(lr);
    }

    pub fn policy_update(&mut self, batch: &RolloutBatch) -> f64 {
        let mut total_loss = 0.0_f64;
        for _ in 0..self.config.ppo_epochs {
            let (log_probs, entropy) = self
                .actor_critic
                .evaluate_actions(&batch.observations, &batch.actions);
            let (_, _, values) = self.actor_critic.forward(&batch.observations);

            let ratio = (&log_probs - &batch.old_log_probs).exp();
            let surr1 = &ratio * &batch.advantages;
            let surr2 = ratio.clamp(
                1.0 - self.config.clip_epsilon,
                1.0 + self.config.clip_epsilon,
            ) * &batch.advantages;
            let policy_loss = -surr1.min_other(&surr2).mean(Kind::Float);
            let value_loss = (&values - &batch.returns)
                .pow_tensor_scalar(2)
                .mean(Kind::Float);
            let entropy_bonus = entropy.mean(Kind::Float);
            let loss = &policy_loss + self.config.value_coef * &value_loss
                - self.config.entropy_coef * &entropy_bonus;

            self.optimizer.backward_step(&loss);
            total_loss += f64::try_from(loss.detach()).unwrap_or(f64::NAN);
        }
        total_loss / self.config.ppo_epochs as f64
    }
}

/// WoLF dual-rate wrapper around [`GaussianPpoTrainer`].
pub struct GaussianWolfPpoTrainer {
    pub inner: GaussianPpoTrainer,
    config: WolfPpoConfig,
    payoff_history: std::collections::VecDeque<f64>,
    rolling_avg_payoff: f64,
    current_payoff: f64,
}

impl GaussianWolfPpoTrainer {
    pub fn new_on_device(
        obs_dim: i64,
        config: WolfPpoConfig,
        architecture: GaussianArchitecture,
        rung_count: i64,
        device: Device,
    ) -> Self {
        let inner = GaussianPpoTrainer::new_on_device(
            obs_dim,
            config.ppo.clone(),
            architecture,
            rung_count,
            device,
        );
        Self {
            inner,
            config,
            payoff_history: std::collections::VecDeque::new(),
            rolling_avg_payoff: 0.0,
            current_payoff: 0.0,
        }
    }

    pub fn device(&self) -> Device {
        self.inner.device()
    }

    pub fn is_winning(&self) -> bool {
        self.current_payoff > self.rolling_avg_payoff
    }

    pub fn active_lr(&self) -> f64 {
        if self.is_winning() {
            self.config.alpha_win
        } else {
            self.config.alpha_lose
        }
    }

    pub fn policy_update(&mut self, batch: &RolloutBatch, episode_return: f64) -> f64 {
        self.current_payoff = episode_return;
        self.payoff_history.push_back(episode_return);
        if self.payoff_history.len() > self.config.payoff_window {
            self.payoff_history.pop_front();
        }
        let n = self.payoff_history.len() as f64;
        self.rolling_avg_payoff = self.payoff_history.iter().sum::<f64>() / n;
        let lr = self.active_lr();
        self.inner.set_lr(lr);
        self.inner.policy_update(batch)
    }
}

/// Driver config for the Gaussian ladder job (no discrete `num_actions`).
#[derive(Debug, Clone)]
pub struct GaussianTrainDriverConfig {
    pub obs_dim: i64,
    pub horizon: usize,
    pub gamma: f64,
    pub gae_lambda: f64,
    pub architecture: GaussianArchitecture,
    pub rung_count: i64,
}

impl Default for GaussianTrainDriverConfig {
    fn default() -> Self {
        Self {
            obs_dim: 40,
            horizon: 64,
            gamma: 0.99,
            gae_lambda: 0.95,
            architecture: GaussianArchitecture::Mlp,
            rung_count: 8,
        }
    }
}

/// Collect-then-update driver for the tanh-Gaussian inventory policy.
pub struct GaussianWolfPpoTrainDriver {
    pub trainer: GaussianWolfPpoTrainer,
    collector: GaussianRolloutCollector,
    total_steps: usize,
}

impl GaussianWolfPpoTrainDriver {
    pub fn new_on_device(
        driver_config: GaussianTrainDriverConfig,
        wolf_config: WolfPpoConfig,
        device: Device,
    ) -> Self {
        let collector = GaussianRolloutCollector::new(
            driver_config.horizon,
            driver_config.gamma,
            driver_config.gae_lambda,
        );
        let trainer = GaussianWolfPpoTrainer::new_on_device(
            driver_config.obs_dim,
            wolf_config,
            driver_config.architecture,
            driver_config.rung_count,
            device,
        );
        Self {
            trainer,
            collector,
            total_steps: 0,
        }
    }

    pub fn actor_critic(&self) -> &GaussianPolicy {
        &self.trainer.inner.actor_critic
    }

    pub fn train_step<F>(
        &mut self,
        initial_obs: Vec<f32>,
        env_step: F,
        bootstrap_value: f32,
    ) -> TrainMetrics
    where
        F: FnMut(Vec<f32>, f32) -> StepOutput,
    {
        let ac = &self.trainer.inner.actor_critic;
        self.collector.collect(ac, initial_obs, env_step);
        let steps = self.collector.len();
        self.total_steps += steps;
        let batch = self
            .collector
            .into_batch_on(bootstrap_value, self.trainer.device());
        let (value_loss, entropy) = gaussian_diagnostics(&self.trainer.inner.actor_critic, &batch);
        let episode_return = batch.returns.mean(Kind::Float).double_value(&[]);
        let active_lr = self.trainer.active_lr();
        let policy_loss = self.trainer.policy_update(&batch, episode_return);
        TrainMetrics {
            policy_loss,
            value_loss,
            entropy,
            nes_distance: None,
            active_lr,
            steps_collected: steps,
        }
    }
}

fn gaussian_diagnostics(ac: &GaussianPolicy, batch: &RolloutBatch) -> (f64, f64) {
    let _g = tch::no_grad_guard();
    let (_, entropy) = ac.evaluate_actions(&batch.observations, &batch.actions);
    let (_, _, values) = ac.forward(&batch.observations);
    let value_loss = (&values - &batch.returns)
        .pow_tensor_scalar(2)
        .mean(Kind::Float)
        .double_value(&[]);
    let entropy_mean = entropy.mean(Kind::Float).double_value(&[]);
    (value_loss, entropy_mean)
}

/// Training configuration for the Gaussian ladder microstructure job.
#[derive(Debug, Clone)]
pub struct GaussianMicrostructureTrainConfig {
    pub sim: MicrostructureConfig,
    pub driver: GaussianTrainDriverConfig,
    pub wolf: WolfPpoConfig,
    pub num_updates: usize,
    pub checkpoint_dir: Option<PathBuf>,
    pub device: Device,
    pub data_window: String,
}

impl Default for GaussianMicrostructureTrainConfig {
    fn default() -> Self {
        let sim = MicrostructureConfig::default();
        Self {
            driver: GaussianTrainDriverConfig {
                obs_dim: sim.ladder_obs_dim(),
                horizon: sim.episode_steps,
                rung_count: sim.rung_count as i64,
                ..Default::default()
            },
            sim,
            wolf: WolfPpoConfig::default(),
            num_updates: 3,
            checkpoint_dir: None,
            device: Device::Cpu,
            data_window: "sim:gaussian-ladder".into(),
        }
    }
}

/// Held-out mean-action eval vs Hold (`a = 0`).
#[derive(Debug, Clone)]
pub struct GaussianMeanActionEval {
    pub mean_action_reward: f32,
    pub hold_reward: f32,
    pub mean_abs_inventory: f32,
    pub hold_mean_abs_inventory: f32,
}

/// Long-lived Gaussian ladder session. Refuses retired unit-lot checkpoints.
pub struct GaussianMicrostructureTrainSession {
    driver: GaussianWolfPpoTrainDriver,
    sim_config: MicrostructureConfig,
    data_window: String,
    config_fingerprint: String,
    pub update_count: usize,
}

impl GaussianMicrostructureTrainSession {
    pub fn new(config: &GaussianMicrostructureTrainConfig) -> Self {
        Self {
            driver: GaussianWolfPpoTrainDriver::new_on_device(
                config.driver.clone(),
                config.wolf.clone(),
                config.device,
            ),
            sim_config: config.sim.clone(),
            data_window: config.data_window.clone(),
            config_fingerprint: format!(
                "gaussian {:?} hidden={:?} seed={} lambda={}",
                config.driver.architecture,
                config.wolf.ppo.hidden_sizes,
                config.sim.seed,
                config.sim.lambda
            ),
            update_count: 0,
        }
    }

    pub fn resume_from(
        checkpoint_dir: impl AsRef<Path>,
        config: &GaussianMicrostructureTrainConfig,
    ) -> Self {
        let checkpoint_dir = checkpoint_dir.as_ref();
        std::fs::create_dir_all(checkpoint_dir).expect("create gaussian checkpoint dir");
        let mut session = Self::new(config);
        if is_retired_unit_lot_dir(checkpoint_dir) {
            return session;
        }
        if let Some(path) = resolve_resume_checkpoint(checkpoint_dir) {
            if !is_retired_unit_lot_dir(&path) {
                load_checkpoint_if_exists(&mut session.driver.trainer.inner.vs, &path);
            }
        }
        session
    }

    pub fn actor_critic(&self) -> &GaussianPolicy {
        self.driver.actor_critic()
    }

    pub fn train_step(&mut self) -> (TrainMetrics, MicrostructureStats) {
        let mut sim = MicrostructureSim::new(self.sim_config.clone());
        let _ = sim.reset();
        let mut last_stats = MicrostructureStats::default();

        let metrics = self.driver.train_step(
            sim.ladder_observation(),
            |current_obs, action| {
                let _ = current_obs;
                let result = sim.step_target(action);
                if result.done {
                    last_stats = sim.finish_episode();
                    let _ = sim.reset();
                }
                StepOutput {
                    next_observation: sim.ladder_observation(),
                    reward: result.reward,
                    done: result.done,
                }
            },
            0.0,
        );

        self.update_count += 1;
        (metrics, last_stats)
    }

    /// Held-out **mean-action** eval vs Hold (`a = 0`), not last in-episode reward.
    pub fn evaluate_mean_action(&self, seeds: &[u64]) -> GaussianMeanActionEval {
        evaluate_gaussian_mean_action(self.actor_critic(), &self.sim_config, seeds)
    }

    pub fn save_checkpoint(&self, checkpoint_dir: &Path, save_numbered: bool) {
        if is_retired_unit_lot_dir(checkpoint_dir) {
            return;
        }
        std::fs::create_dir_all(checkpoint_dir).expect("create checkpoint dir");
        if save_numbered {
            let path = checkpoint_dir.join(format!("update_{}.safetensors", self.update_count - 1));
            save_checkpoint(&self.driver.trainer.inner.vs, &path)
                .expect("save numbered gaussian checkpoint");
        }
        let latest = checkpoint_dir.join(LATEST_CHECKPOINT);
        save_checkpoint_with_fingerprint(
            &self.driver.trainer.inner.vs,
            &latest,
            &self.data_window,
            &self.config_fingerprint,
        )
        .expect("save latest gaussian checkpoint + fingerprint");
    }

    pub fn finalize_checkpoints(&self, checkpoint_dir: &Path) {
        if is_retired_unit_lot_dir(checkpoint_dir) {
            return;
        }
        let latest = checkpoint_dir.join(LATEST_CHECKPOINT);
        if !latest.exists() {
            return;
        }
        std::fs::copy(&latest, checkpoint_dir.join(FINAL_CHECKPOINT)).expect("copy final checkpoint");
    }
}

/// Mean-action (`tanh(μ)`) eval versus always-Hold `a = 0`.
pub fn evaluate_gaussian_mean_action(
    actor_critic: &GaussianPolicy,
    config: &MicrostructureConfig,
    seeds: &[u64],
) -> GaussianMeanActionEval {
    let obs_dim = config.ladder_obs_dim();
    let mut policy_rewards = Vec::new();
    let mut policy_abs_q = Vec::new();
    let mut hold_rewards = Vec::new();
    let mut hold_abs_q = Vec::new();

    for &seed in seeds {
        let stats = run_episode_with_targets(config, seed, |_, ladder| {
            mean_action_from_obs(actor_critic, ladder, obs_dim)
        });
        policy_rewards.push(stats.total_reward);
        policy_abs_q.push(stats.mean_abs_inventory);

        let hold = run_episode_with_targets(config, seed, |_, _| 0.0);
        hold_rewards.push(hold.total_reward);
        hold_abs_q.push(hold.mean_abs_inventory);
    }

    GaussianMeanActionEval {
        mean_action_reward: mean_f32(&policy_rewards),
        hold_reward: mean_f32(&hold_rewards),
        mean_abs_inventory: mean_f32(&policy_abs_q),
        hold_mean_abs_inventory: mean_f32(&hold_abs_q),
    }
}

fn mean_action_from_obs(ac: &GaussianPolicy, obs: &[f32], obs_dim: i64) -> f32 {
    let t = Tensor::from_slice(obs)
        .to_device(ac.device())
        .to_kind(Kind::Float)
        .view([1, obs_dim]);
    let _g = tch::no_grad_guard();
    clamp_inventory_target(ac.mean_action(&t).double_value(&[]) as f32)
}

fn mean_f32(values: &[f32]) -> f32 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f32>() / values.len() as f32
    }
}

/// Short train + mean-action eval (torch smoke).
pub fn run_gaussian_ladder_train(
    config: GaussianMicrostructureTrainConfig,
) -> (Vec<TrainMetrics>, GaussianMeanActionEval, PathBuf) {
    let checkpoint_dir = config.checkpoint_dir.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("trolly_gym_gaussian_{}", std::process::id()))
    });
    let mut session = GaussianMicrostructureTrainSession::resume_from(&checkpoint_dir, &config);
    let mut metrics_log = Vec::with_capacity(config.num_updates);
    for _ in 0..config.num_updates {
        let (metrics, _) = session.train_step();
        metrics_log.push(metrics);
        session.save_checkpoint(&checkpoint_dir, false);
    }
    let eval = session.evaluate_mean_action(&[1000, 1001, 1002]);
    (metrics_log, eval, checkpoint_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppo::WolfPpoConfig;

    #[test]
    fn retired_dir_is_detected() {
        assert!(is_retired_unit_lot_dir(
            "checkpoints/gpu_train_orchestrator/_retired_unit_lot_microstructure/mlp"
        ));
        assert!(!is_retired_unit_lot_dir(
            "checkpoints/gpu_train_orchestrator/microstructure/gaussian_mlp"
        ));
    }

    #[test]
    fn gaussian_log_prob_and_short_ladder_train_eval() {
        let sim = MicrostructureConfig {
            episode_steps: 16,
            window_frames: 1,
            rung_count: 4,
            lambda: 0.25,
            resample_episode_seeds: true,
            mid_noise: 0.1,
            ..Default::default()
        };
        let dir = std::env::temp_dir().join(format!(
            "trolly_gym_gaussian_smoke_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let config = GaussianMicrostructureTrainConfig {
            driver: GaussianTrainDriverConfig {
                obs_dim: sim.ladder_obs_dim(),
                horizon: 16,
                ..Default::default()
            },
            sim: sim.clone(),
            wolf: WolfPpoConfig {
                ppo: PpoConfig {
                    ppo_epochs: 1,
                    ..Default::default()
                },
                ..Default::default()
            },
            num_updates: 2,
            checkpoint_dir: Some(dir.clone()),
            device: Device::Cpu,
            data_window: "test:gaussian".into(),
        };
        let (metrics, eval, _) = run_gaussian_ladder_train(config);
        assert_eq!(metrics.len(), 2);
        assert!(metrics.last().unwrap().policy_loss.is_finite());
        assert!(eval.mean_action_reward.is_finite());
        assert!(eval.hold_reward.is_finite());
        assert!(eval.mean_abs_inventory.is_finite());
        assert!(eval.mean_abs_inventory <= 1.0 + 1e-5);
        assert!(eval.hold_mean_abs_inventory.abs() < 1e-5);
        assert!(dir.join(LATEST_CHECKPOINT).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn planted_lambda_hold_is_mean_reverting() {
        let sim = MicrostructureConfig {
            episode_steps: 8,
            lambda: 0.25,
            mid_noise: 0.0,
            resample_episode_seeds: false,
            ..Default::default()
        };
        let vs = nn::VarStore::new(Device::Cpu);
        let ac = GaussianPolicy::Mlp(GaussianActorCritic::new(
            &vs,
            sim.ladder_obs_dim(),
            &PpoConfig::default(),
        ));
        let eval = evaluate_gaussian_mean_action(&ac, &sim, &[7]);
        assert!(eval.hold_mean_abs_inventory.abs() < 1e-5);
        assert!(eval.mean_abs_inventory >= 0.0 && eval.mean_abs_inventory <= 1.0);
    }

    #[test]
    fn liquid_on_rungs_short_train_eval() {
        let sim = MicrostructureConfig {
            episode_steps: 12,
            window_frames: 1,
            rung_count: 4,
            lambda: 0.25,
            resample_episode_seeds: true,
            mid_noise: 0.05,
            ..Default::default()
        };
        let dir = std::env::temp_dir().join(format!(
            "trolly_gym_gaussian_liquid_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let config = GaussianMicrostructureTrainConfig {
            driver: GaussianTrainDriverConfig {
                obs_dim: sim.ladder_obs_dim(),
                horizon: 12,
                architecture: GaussianArchitecture::LiquidRungs,
                rung_count: sim.rung_count as i64,
                ..Default::default()
            },
            sim,
            wolf: WolfPpoConfig {
                ppo: PpoConfig {
                    ppo_epochs: 1,
                    ..Default::default()
                },
                ..Default::default()
            },
            num_updates: 2,
            checkpoint_dir: Some(dir.clone()),
            device: Device::Cpu,
            data_window: "test:gaussian-liquid".into(),
        };
        let (metrics, eval, _) = run_gaussian_ladder_train(config);
        assert_eq!(metrics.len(), 2);
        assert!(metrics.last().unwrap().policy_loss.is_finite());
        assert!(eval.mean_action_reward.is_finite());
        assert!(eval.hold_mean_abs_inventory.abs() < 1e-5);
        assert!(dir.join(LATEST_CHECKPOINT).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
