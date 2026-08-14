//! Ring-buffer replay store for stream-derived feature windows and on-policy rollouts.
//!
//! SOTA takeaway (2024–2026) vs this stack (on-policy WoLF-PPO / PPO, not DQN/SAC):
//! - PER / LAP / PAL (Schaul TD-error priorities) assume off-policy 1-step updates;
//!   they go stale under fast policy drift (FreshPER 2026) and do not fit clipped PPO.
//! - Reverb (DeepMind) is a distributed chunk store — too much infra for local gym.
//! - What fits: a **recency-bounded FIFO of whole trajectories** (HP3O 2025;
//!   Reverb-style sequence items). Keep recent episodes, evict by age, flatten
//!   for the existing GAE / PPO update. Do not sample isolated transitions.

use crate::action::Action;

/// One environment transition recorded for offline training.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    pub observation: Vec<f32>,
    pub action: Action,
    pub reward: f32,
    pub done: bool,
}

/// One on-policy step with PPO fields (obs, action, log-prob, value, reward, done).
///
/// Compatible with [`crate::ppo::RolloutBatch`] via
/// [`crate::train::rollout_buffer_to_batch`] (`torch` feature). The existing
/// [`ReplayBuffer`] keeps stream transitions without log-prob/value; use this
/// buffer for WoLF-PPO / PPO update steps.
#[derive(Debug, Clone, PartialEq)]
pub struct OnPolicyStep {
    pub observation: Vec<f32>,
    pub action: i64,
    pub log_prob: f64,
    pub value: f64,
    pub reward: f64,
    pub done: bool,
}

/// In-memory on-policy trajectory buffer for a single rollout.
#[derive(Debug, Clone, Default)]
pub struct OnPolicyRolloutBuffer {
    steps: Vec<OnPolicyStep>,
}

impl OnPolicyRolloutBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            steps: Vec::with_capacity(capacity),
        }
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn steps(&self) -> &[OnPolicyStep] {
        &self.steps
    }

    pub fn push(&mut self, step: OnPolicyStep) {
        self.steps.push(step);
    }

    pub fn clear(&mut self) {
        self.steps.clear();
    }

    /// Append a step from an env transition plus policy-side fields.
    /// Drain steps as a finished trajectory (caller then pushes into [`TrajectoryReplay`]).
    pub fn into_trajectory(self, episode_return: f64) -> Trajectory {
        Trajectory {
            steps: self.steps,
            episode_return,
            age: 0,
        }
    }

    pub fn push_env_step(
        &mut self,
        observation: Vec<f32>,
        action: Action,
        log_prob: f64,
        value: f64,
        reward: f64,
        done: bool,
    ) {
        self.push(OnPolicyStep {
            observation,
            action: action_index(action),
            log_prob,
            value,
            reward,
            done,
        });
    }
}

/// One finished on-policy episode (sequence), not a single transition.
#[derive(Debug, Clone, PartialEq)]
pub struct Trajectory {
    pub steps: Vec<OnPolicyStep>,
    pub episode_return: f64,
    /// Insertions since this trajectory was written (freshness / stale-data).
    pub age: usize,
}

impl Trajectory {
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// Recency-bounded FIFO of trajectories for on-policy / slightly-stale PPO.
///
/// Evicts oldest episodes (HP3O FIFO) instead of PER-sampling by TD error.
/// Age is incremented on each [`Self::push`] so callers can drop stale items
/// without a priority heap.
#[derive(Debug, Clone)]
pub struct TrajectoryReplay {
    capacity: usize,
    items: Vec<Trajectory>,
}

impl TrajectoryReplay {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            items: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn trajectories(&self) -> &[Trajectory] {
        &self.items
    }

    /// Push a finished episode; evict the oldest when full.
    pub fn push(&mut self, mut trajectory: Trajectory) {
        for item in &mut self.items {
            item.age = item.age.saturating_add(1);
        }
        trajectory.age = 0;
        if self.items.len() == self.capacity {
            self.items.remove(0);
        }
        self.items.push(trajectory);
    }

    /// Flatten recent trajectories (oldest-first) for a PPO/GAE batch.
    pub fn flatten_steps(&self) -> Vec<OnPolicyStep> {
        self.items
            .iter()
            .flat_map(|t| t.steps.iter().cloned())
            .collect()
    }

    /// Highest-return episode still in the FIFO (HP3O "best trajectory" hint).
    pub fn best_return(&self) -> Option<&Trajectory> {
        self.items
            .iter()
            .max_by(|a, b| a.episode_return.total_cmp(&b.episode_return))
    }
}

/// Map discrete gym actions to categorical policy indices (Hold=0, Buy=1, Sell=2).
pub fn action_index(action: Action) -> i64 {
    match action {
        Action::Hold => 0,
        Action::Buy => 1,
        Action::Sell => 2,
    }
}

/// Map policy index back to a gym action.
pub fn action_from_index(index: i64) -> Action {
    match index {
        0 => Action::Hold,
        1 => Action::Buy,
        _ => Action::Sell,
    }
}

/// Fixed-capacity ring buffer of transitions (training replay stub).
#[derive(Debug, Clone)]
pub struct ReplayBuffer {
    capacity: usize,
    slots: Vec<Option<Transition>>,
    head: usize,
    len: usize,
}

impl ReplayBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            slots: (0..capacity).map(|_| None).collect(),
            head: 0,
            len: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_full(&self) -> bool {
        self.len == self.capacity
    }

    /// Push a transition, overwriting the oldest entry when full.
    pub fn push(&mut self, transition: Transition) {
        self.slots[self.head] = Some(transition);
        self.head = (self.head + 1) % self.capacity;
        if self.len < self.capacity {
            self.len += 1;
        }
    }

    /// Snapshot stored transitions in insertion order (oldest first).
    pub fn snapshot(&self) -> Vec<Transition> {
        if self.len == 0 {
            return Vec::new();
        }
        let start = if self.len == self.capacity {
            self.head
        } else {
            0
        };
        (0..self.len)
            .filter_map(|i| {
                let idx = (start + i) % self.capacity;
                self.slots[idx].clone()
            })
            .collect()
    }

    /// Store the latest flattened observation window without an action (prefill stub).
    pub fn push_observation_window(&mut self, window: &[f32]) {
        self.push(Transition {
            observation: window.to_vec(),
            action: Action::Hold,
            reward: 0.0,
            done: false,
        });
    }

    /// Store a full transition from an observation window and action.
    pub fn push_step(
        &mut self,
        observation: Vec<f32>,
        action: Action,
        reward: f32,
        done: bool,
    ) {
        self.push(Transition {
            observation,
            action,
            reward,
            done,
        });
    }

    /// N-step returns over the FIFO ring, segmented by `done`.
    ///
    /// Endpoint Replay (RLC 2026) and PTR-PPO keep temporal chains instead of
    /// isolated 1-step TD targets. This is the cheap local form: walk the
    /// existing snapshot and bootstrap `n` steps or until episode end.
    pub fn n_step_returns(&self, n: usize, gamma: f32) -> Vec<(Transition, f32)> {
        let snap = self.snapshot();
        n_step_returns(&snap, n.max(1), gamma)
    }

    /// Freshness-aware sample from the FIFO ring (newest age = 0).
    ///
    /// Why this, not classic PER / Reverb / LAP:
    /// - Schaul PER and LAP/PAL assume off-policy Q-learning with a large
    ///   uniform-or-TD-error buffer. This stack trains on-policy WoLF-PPO.
    /// - FreshPER (2026) showed undecayed PER lets stale high-priority
    ///   trajectories dominate once the policy drifts; they multiply priority
    ///   by `exp(-age/τ)`. HP3O / DyJR keep a short FIFO so reuse stays near
    ///   on-policy. Reverb is a distributed service we do not need locally.
    /// - Sampling here is `p_i ∝ exp(-age/τ)` over the existing ring. No
    ///   sum-tree, no importance-sampling ratios (PPO already clips).
    pub fn sample_fresh(&self, k: usize, tau: f32, rng_state: &mut u64) -> Vec<Transition> {
        let snap = self.snapshot();
        sample_fresh(&snap, k, tau, rng_state)
    }
}

/// N-step discounted returns for a chronological transition slice.
pub fn n_step_returns(steps: &[Transition], n: usize, gamma: f32) -> Vec<(Transition, f32)> {
    let n = n.max(1);
    let mut out = Vec::with_capacity(steps.len());
    for i in 0..steps.len() {
        let mut acc = 0.0_f32;
        let mut discount = 1.0_f32;
        for step in steps.iter().skip(i).take(n) {
            acc += discount * step.reward;
            if step.done {
                break;
            }
            discount *= gamma;
        }
        out.push((steps[i].clone(), acc));
    }
    out
}

/// Age-decayed sample; `steps` is oldest-first (age = len-1-i).
pub fn sample_fresh(
    steps: &[Transition],
    k: usize,
    tau: f32,
    rng_state: &mut u64,
) -> Vec<Transition> {
    if steps.is_empty() || k == 0 {
        return Vec::new();
    }
    let tau = if tau <= 0.0 { 1.0 } else { tau };
    let len = steps.len();
    let weights: Vec<f32> = (0..len)
        .map(|i| {
            let age = (len - 1 - i) as f32;
            (-age / tau).exp()
        })
        .collect();
    let total: f32 = weights.iter().sum();
    let mut out = Vec::with_capacity(k.min(len));
    for _ in 0..k {
        let pick = next_unit(rng_state) * total;
        let mut acc = 0.0;
        let mut idx = len - 1;
        for (i, w) in weights.iter().enumerate() {
            acc += *w;
            if acc >= pick {
                idx = i;
                break;
            }
        }
        out.push(steps[idx].clone());
    }
    out
}

fn next_unit(state: &mut u64) -> f32 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1);
    (*state >> 33) as f32 / u32::MAX as f32
}

/// Convenience helper: ring buffer of raw feature vectors from stream frames.
#[derive(Debug, Clone)]
pub struct FeatureRingBuffer {
    capacity: usize,
    frames: Vec<Option<crate::observation::FeatureVector>>,
    head: usize,
    len: usize,
}

impl FeatureRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            frames: (0..capacity).map(|_| None).collect(),
            head: 0,
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn push(&mut self, frame: crate::observation::FeatureVector) {
        self.frames[self.head] = Some(frame);
        self.head = (self.head + 1) % self.capacity;
        if self.len < self.capacity {
            self.len += 1;
        }
    }

    pub fn snapshot(&self) -> Vec<crate::observation::FeatureVector> {
        if self.len == 0 {
            return Vec::new();
        }
        let start = if self.len == self.capacity {
            self.head
        } else {
            0
        };
        (0..self.len)
            .filter_map(|i| {
                let idx = (start + i) % self.capacity;
                self.frames[idx].clone()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_buffer_overwrites_oldest() {
        let mut buf = ReplayBuffer::new(2);
        buf.push_step(vec![1.0], Action::Hold, 0.0, false);
        buf.push_step(vec![2.0], Action::Buy, 1.0, false);
        buf.push_step(vec![3.0], Action::Sell, -1.0, true);
        let snap = buf.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].observation, vec![2.0]);
        assert_eq!(snap[1].observation, vec![3.0]);
        assert!(snap[1].done);
    }

    #[test]
    fn on_policy_buffer_stores_ppo_fields() {
        let mut buf = OnPolicyRolloutBuffer::new();
        buf.push_env_step(vec![1.0, 2.0], Action::Buy, -0.5, 0.1, 0.02, false);
        assert_eq!(buf.len(), 1);
        let step = &buf.steps()[0];
        assert_eq!(step.action, 1);
        assert!((step.log_prob + 0.5).abs() < 1e-9);
        assert!((step.value - 0.1).abs() < 1e-9);
    }

    #[test]
    fn action_index_roundtrip() {
        for action in [Action::Hold, Action::Buy, Action::Sell] {
            assert_eq!(action_from_index(action_index(action)), action);
        }
    }

    #[test]
    fn trajectory_fifo_evicts_oldest_and_ages() {
        let mut replay = TrajectoryReplay::new(2);
        let mut a = OnPolicyRolloutBuffer::new();
        a.push_env_step(vec![1.0], Action::Hold, 0.0, 0.0, 1.0, true);
        replay.push(a.into_trajectory(1.0));
        let mut b = OnPolicyRolloutBuffer::new();
        b.push_env_step(vec![2.0], Action::Buy, 0.0, 0.0, 2.0, true);
        replay.push(b.into_trajectory(2.0));
        let mut c = OnPolicyRolloutBuffer::new();
        c.push_env_step(vec![3.0], Action::Sell, 0.0, 0.0, 0.5, true);
        replay.push(c.into_trajectory(0.5));
        assert_eq!(replay.len(), 2);
        assert_eq!(replay.trajectories()[0].steps[0].observation, vec![2.0]);
        assert_eq!(replay.trajectories()[0].age, 1);
        assert_eq!(replay.trajectories()[1].age, 0);
        assert_eq!(replay.best_return().unwrap().episode_return, 2.0);
        assert_eq!(replay.flatten_steps().len(), 2);
    }

    #[test]
    fn n_step_stops_at_done() {
        let mut buf = ReplayBuffer::new(8);
        buf.push_step(vec![1.0], Action::Hold, 1.0, false);
        buf.push_step(vec![2.0], Action::Buy, 1.0, true);
        buf.push_step(vec![3.0], Action::Sell, 10.0, false);
        let returns = buf.n_step_returns(3, 1.0);
        assert!((returns[0].1 - 2.0).abs() < 1e-5);
        assert!((returns[1].1 - 1.0).abs() < 1e-5);
        assert!((returns[2].1 - 10.0).abs() < 1e-5);
    }

    #[test]
    fn sample_fresh_prefers_newer() {
        let mut buf = ReplayBuffer::new(8);
        buf.push_step(vec![0.0], Action::Hold, 0.0, false);
        buf.push_step(vec![1.0], Action::Buy, 0.0, false);
        let mut rng = 7u64;
        let samples = buf.sample_fresh(64, 0.25, &mut rng);
        let newest = samples
            .iter()
            .filter(|t| t.observation == vec![1.0])
            .count();
        assert!(newest > 32, "fresh samples should lean newest, got {newest}/64");
    }
}
