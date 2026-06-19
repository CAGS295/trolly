//! Ring and replay buffers for stream-derived feature windows.

use std::collections::VecDeque;

use crate::observation::{flatten_window, FEATURES_PER_EVENT};

/// Fixed-capacity store of recent observation vectors (oldest entries evicted).
#[derive(Debug, Clone, PartialEq)]
pub struct RingBuffer {
    capacity: usize,
    entries: VecDeque<Vec<f64>>,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::new(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn push(&mut self, features: Vec<f64>) {
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(features);
    }

    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &Vec<f64>> {
        self.entries.iter()
    }

    /// Flatten the trailing `window_len` rows into one observation vector.
    pub fn window(&self, window_len: usize) -> Vec<f64> {
        let rows: Vec<Vec<f64>> = self.entries.iter().cloned().collect();
        flatten_window(&rows, window_len)
    }
}

/// Stub replay buffer retaining recent transitions for offline training hooks.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayBuffer {
    ring: RingBuffer,
    transitions: Vec<Transition>,
    capacity: usize,
}

/// A single `(observation, action)` transition stub.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    pub observation: Vec<f64>,
    pub action_index: usize,
}

impl ReplayBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            ring: RingBuffer::new(capacity),
            transitions: Vec::new(),
            capacity: capacity.max(1),
        }
    }

    pub fn ring(&self) -> &RingBuffer {
        &self.ring
    }

    pub fn ring_mut(&mut self) -> &mut RingBuffer {
        &mut self.ring
    }

    pub fn push_observation(&mut self, features: Vec<f64>) {
        self.ring.push(features);
    }

    pub fn record_transition(&mut self, observation: Vec<f64>, action_index: usize) {
        if self.transitions.len() >= self.capacity {
            self.transitions.remove(0);
        }
        self.transitions.push(Transition {
            observation,
            action_index,
        });
    }

    pub fn transitions(&self) -> &[Transition] {
        &self.transitions
    }

    pub fn window(&self, window_len: usize) -> Vec<f64> {
        self.ring.window(window_len)
    }

    pub fn feature_dim(&self, window_len: usize) -> usize {
        window_len * FEATURES_PER_EVENT
    }
}
