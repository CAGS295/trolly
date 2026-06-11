//! libtorch.rs training gym scaffold over trolly streams.
//!
//! Consumes normalized [`StreamEvent`] updates from the strategy layer, builds
//! observation windows, and dispatches outbound [`Command`] values through the
//! strategy egress API. Enable `--features torch` for the libtorch integration stub.

mod buffer;
mod env;
mod observation;

#[cfg(feature = "torch")]
pub mod torch;

pub use buffer::{ReplayBuffer, RingBuffer, Transition};
pub use env::{Env, GymAction, StepResult};
pub use observation::{event_to_features, flatten_window, FEATURES_PER_EVENT};
pub use trolly_strategy;

#[cfg(feature = "torch")]
pub use torch::TorchPolicy;

pub use binance_spot_exec;
pub use binance_usdm_exec;

/// Crate identifier for workspace wiring tests.
pub const CRATE: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    use super::*;
    use trolly_strategy::{command_egress, Command, StreamEventKind};

    #[test]
    fn crate_name() {
        assert_eq!(CRATE, "trolly-gym");
    }

    #[test]
    fn ring_buffer_evicts_oldest() {
        let mut ring = RingBuffer::new(2);
        ring.push(vec![1.0]);
        ring.push(vec![2.0]);
        ring.push(vec![3.0]);
        assert_eq!(ring.len(), 2);
        let window = ring.window(2);
        assert_eq!(window, vec![2.0, 0.0, 0.0, 3.0, 0.0, 0.0]);
    }

    #[test]
    fn offline_smoke_mock_observations() {
        let (egress, mut rx) = command_egress();
        let mut env = Env::new(egress, 2);

        let depth = trolly_strategy::StreamEvent::new("BTCUSDT", StreamEventKind::Depth, "100");
        let exec =
            trolly_strategy::StreamEvent::new("ETHUSDT", StreamEventKind::Execution, "fill");
        let account =
            trolly_strategy::StreamEvent::new("BTCUSDT", StreamEventKind::Account, "1");

        assert_eq!(env.push_event(depth), vec![0.0, 100.0, 7.0]);
        assert_eq!(env.push_event(exec), vec![1.0, 0.0, 7.0]);
        env.push_event(account);

        let obs = env.observation();
        assert_eq!(obs.len(), 2 * FEATURES_PER_EVENT);
        assert_eq!(obs[0], 1.0);
        assert_eq!(obs[3], 2.0);

        let result = env.step(
            1,
            GymAction::SendMessage("gym:act".into()),
        );
        assert!(result.dispatched);
        assert_eq!(result.action_index, 1);
        assert_eq!(env.replay_buffer().transitions().len(), 1);

        let cmd = rx.try_recv().unwrap();
        assert_eq!(cmd, Command::send_message("gym:act"));
    }

    #[cfg(feature = "torch")]
    #[test]
    fn torch_policy_stub_infer() {
        let mut policy = TorchPolicy::new();
        assert_eq!(policy.infer(&[1.0, 2.0]), 0);
        policy.load_stub();
        assert!(policy.is_loaded());
        assert_eq!(policy.infer(&[1.0, 2.0, 3.0]), 6 % 8);
    }
}
