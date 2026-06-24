//! Ring-buffer replay store for stream-derived feature windows and on-policy rollouts.

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
}
