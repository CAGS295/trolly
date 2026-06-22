//! Ring-buffer replay store for stream-derived feature windows (stub).
//!
//! [`ReplayBuffer`] stores off-policy [`Transition`] rows (observation, action, reward, done).
//! [`RolloutCollector`] is the on-policy parallel buffer used by PPO / WoLF-PPO: it also
//! records behaviour-policy log-probabilities and value estimates for each step.

use crate::action::Action;
use crate::observation::FeatureVector;

/// One environment transition recorded for offline training.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    pub observation: Vec<f32>,
    pub action: Action,
    pub reward: f32,
    pub done: bool,
}

/// One on-policy rollout step (behaviour policy log-prob and value included).
///
/// Compatible with [`RolloutBatch`](crate::ppo::RolloutBatch) when the `torch` feature is enabled.
#[derive(Debug, Clone, PartialEq)]
pub struct RolloutStep {
    pub observation: Vec<f32>,
    pub action: Action,
    pub log_prob: f32,
    pub value: f32,
    pub reward: f32,
    pub done: bool,
}

impl RolloutStep {
    pub fn action_index(&self) -> i64 {
        self.action.to_index()
    }
}

/// In-memory collector for on-policy trajectories before a PPO update.
#[derive(Debug, Clone, Default)]
pub struct RolloutCollector {
    steps: Vec<RolloutStep>,
}

impl RolloutCollector {
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

    pub fn steps(&self) -> &[RolloutStep] {
        &self.steps
    }

    pub fn push(&mut self, step: RolloutStep) {
        self.steps.push(step);
    }

    pub fn clear(&mut self) {
        self.steps.clear();
    }

    /// Mean step reward in the collected trajectory (WoLF current payoff).
    pub fn mean_reward(&self) -> f64 {
        if self.steps.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.steps.iter().map(|s| f64::from(s.reward)).sum();
        sum / self.steps.len() as f64
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
    frames: Vec<Option<FeatureVector>>,
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

    pub fn push(&mut self, frame: FeatureVector) {
        self.frames[self.head] = Some(frame);
        self.head = (self.head + 1) % self.capacity;
        if self.len < self.capacity {
            self.len += 1;
        }
    }

    pub fn snapshot(&self) -> Vec<FeatureVector> {
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
    fn rollout_collector_tracks_mean_reward() {
        let mut collector = RolloutCollector::with_capacity(4);
        collector.push(RolloutStep {
            observation: vec![1.0],
            action: Action::Hold,
            log_prob: -0.5,
            value: 0.1,
            reward: 1.0,
            done: false,
        });
        collector.push(RolloutStep {
            observation: vec![2.0],
            action: Action::Buy,
            log_prob: -0.3,
            value: 0.2,
            reward: -1.0,
            done: false,
        });
        assert_eq!(collector.len(), 2);
        assert!((collector.mean_reward() - 0.0).abs() < f64::EPSILON);
    }
}
