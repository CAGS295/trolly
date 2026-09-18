//! Stream-fed training environment stepping on observations and dispatching actions.

use trolly_strategy::{DepthUpdate, OutboundMessage, StreamEgress, StreamEvent};
use trolly_stream::Message;

use crate::action::{Action, ActionDecision};
use crate::observation::{
    features_from_event, join_feature_frames, ladder_features, ladder_features_from_depth,
    zero_stream_features, DepthLadderSpec, ObservationWindow,
};
use crate::policy::PolicyProvider;
use crate::replay::ReplayBuffer;

/// Result of one environment step.
#[derive(Debug, Clone, PartialEq)]
pub struct StepResult {
    pub observation: Vec<f32>,
    pub reward: f32,
    pub done: bool,
}

/// Error returned by [`run_offline_policy_harness`].
#[derive(Debug)]
pub enum OfflinePolicyHarnessError<E> {
    Parse(trolly_strategy::ParseError),
    Egress(E),
}

impl<E: std::fmt::Debug> std::fmt::Display for OfflinePolicyHarnessError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(err) => write!(f, "{err}"),
            Self::Egress(err) => write!(f, "egress error: {err:?}"),
        }
    }
}

impl<E: std::fmt::Debug> std::error::Error for OfflinePolicyHarnessError<E> {}

/// Configuration for [`Env`] observation windows and replay capacity.
#[derive(Debug, Clone)]
pub struct EnvConfig {
    pub symbol: String,
    /// Extra public-depth symbols joined into the policy observation.
    ///
    /// Empty keeps the single-symbol Env. The first entry of
    /// [`EnvConfig::tracked_symbols`] is the default dispatch / inventory pair
    /// (`symbol`). Extras join the observation and may be selected by
    /// [`crate::policy::PolicyProvider::decide`] or [`EnvConfig::dispatch_symbol`].
    pub observation_symbols: Vec<String>,
    pub window_frames: usize,
    pub replay_capacity: usize,
    pub default_qty: String,
    pub episode_steps: u64,
    pub reward: RewardConfig,
    /// Feature layout passed to [`PolicyProvider::act`].
    pub observation_layout: ObservationLayout,
    /// Ladder spec used when [`ObservationLayout::Ladder`] is selected.
    pub ladder: DepthLadderSpec,
    /// When false, multi-symbol Ladder `act()` sees only the primary `V×5`.
    /// Default true keeps the WP-045 concatenated `N×V×5` join.
    pub join_ladder_symbols: bool,
    /// Fallback dispatch pair when [`crate::policy::PolicyProvider::decide`]
    /// does not name a symbol. `None` keeps the primary `symbol`.
    pub dispatch_symbol: Option<String>,
}

impl EnvConfig {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            observation_symbols: Vec::new(),
            window_frames: 8,
            replay_capacity: 256,
            default_qty: "0.01".into(),
            episode_steps: 128,
            reward: RewardConfig::default(),
            observation_layout: ObservationLayout::Stream,
            ladder: DepthLadderSpec::default(),
            join_ladder_symbols: true,
            dispatch_symbol: None,
        }
    }

    /// Pin Buy/Sell/Hold onto a tracked extra symbol when the policy omits one.
    ///
    /// The name is appended to [`EnvConfig::observation_symbols`] if missing so
    /// public depth for that pair can join the observation. Qty stays
    /// `default_qty`.
    pub fn set_dispatch_symbol(&mut self, symbol: impl Into<String>) {
        let trimmed = symbol.into();
        let trimmed = trimmed.trim();
        if trimmed.is_empty() {
            self.dispatch_symbol = None;
            return;
        }
        if !self.tracks_symbol(trimmed) {
            let mut extras: Vec<String> = self
                .tracked_symbols()
                .into_iter()
                .skip(1)
                .map(str::to_string)
                .collect();
            extras.push(trimmed.to_string());
            self.observe_symbols(extras);
        }
        self.dispatch_symbol = Some(trimmed.to_string());
    }

    /// Resolve a requested pair to a tracked observation symbol.
    pub fn resolve_dispatch_symbol<'a>(&'a self, requested: Option<&'a str>) -> &'a str {
        let requested = requested.or(self.dispatch_symbol.as_deref());
        let Some(requested) = requested else {
            return self.symbol.as_str();
        };
        self.tracked_symbols()
            .into_iter()
            .find(|tracked| tracked.eq_ignore_ascii_case(requested))
            .unwrap_or(self.symbol.as_str())
    }

    /// Track additional symbols (after the primary dispatch symbol) in one Env.
    pub fn observe_symbols<I, S>(&mut self, symbols: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut tracked = Vec::new();
        Self::push_unique_symbol(&mut tracked, self.symbol.clone());
        for symbol in symbols {
            Self::push_unique_symbol(&mut tracked, symbol.into());
        }
        if let Some(primary) = tracked.first() {
            self.symbol = primary.clone();
        }
        self.observation_symbols = tracked;
    }

    fn push_unique_symbol(tracked: &mut Vec<String>, symbol: String) {
        let trimmed = symbol.trim();
        if trimmed.is_empty() {
            return;
        }
        if tracked
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(trimmed))
        {
            return;
        }
        tracked.push(trimmed.to_string());
    }

    /// Primary dispatch symbol first, then extra observation symbols.
    pub fn tracked_symbols(&self) -> Vec<&str> {
        if self.observation_symbols.len() > 1 {
            self.observation_symbols
                .iter()
                .map(String::as_str)
                .collect()
        } else if self.observation_symbols.len() == 1 {
            vec![self.observation_symbols[0].as_str()]
        } else {
            vec![self.symbol.as_str()]
        }
    }

    pub fn tracks_symbol(&self, symbol: &str) -> bool {
        self.tracked_symbols()
            .iter()
            .any(|tracked| tracked.eq_ignore_ascii_case(symbol))
    }

    pub fn is_multi_symbol(&self) -> bool {
        self.tracked_symbols().len() > 1
    }

    /// Feed `PolicyProvider::act` the WP-032 `V×5` ladder instead of 7-D frames.
    pub fn use_ladder_observation(&mut self) {
        self.observation_layout = ObservationLayout::Ladder;
    }

    pub fn uses_ladder_observation(&self) -> bool {
        matches!(self.observation_layout, ObservationLayout::Ladder)
    }
}

/// Observation vector handed to a [`PolicyProvider`].
///
/// [`ObservationLayout::Stream`] is the live 7-D depth window (Hold / ONNX /
/// 3-logit). [`ObservationLayout::Ladder`] is the Gaussian FA layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ObservationLayout {
    #[default]
    Stream,
    Ladder,
}

/// Market reward configuration.
///
/// Reward is `inventory * Δmid - spread_cost` where inventory is the unit
/// position after the action (`-1`, `0`, or `1`) and spread cost is charged
/// only when the action changes inventory.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RewardConfig {
    pub spread_cost_multiplier: f32,
}

impl Default for RewardConfig {
    fn default() -> Self {
        Self {
            spread_cost_multiplier: 1.0,
        }
    }
}

/// Input accepted by [`Env::step`].
pub trait StepActionSource {
    fn select_decision(&self, observation: &[f32]) -> ActionDecision;

    fn select_action(&self, observation: &[f32]) -> Action {
        self.select_decision(observation).action
    }
}

impl StepActionSource for Action {
    fn select_decision(&self, _observation: &[f32]) -> ActionDecision {
        ActionDecision::from(*self)
    }
}

impl StepActionSource for ActionDecision {
    fn select_decision(&self, _observation: &[f32]) -> ActionDecision {
        self.clone()
    }
}

impl<P> StepActionSource for &P
where
    P: PolicyProvider + ?Sized,
{
    fn select_decision(&self, observation: &[f32]) -> ActionDecision {
        (*self).decide(observation)
    }
}

/// Per-symbol book state joined into the multi-symbol policy observation.
#[derive(Debug, Clone)]
struct SymbolObs {
    symbol: String,
    window: ObservationWindow,
    last_depth: Option<DepthUpdate>,
    position: i8,
    last_mid: Option<f32>,
}

/// Training gym environment fed by normalized stream events.
///
/// Ingest stream updates to grow the observation window; call [`Env::step`] to
/// choose an action, record a transition, and dispatch via strategy egress.
pub struct Env<E>
where
    E: StreamEgress,
{
    config: EnvConfig,
    window: ObservationWindow,
    symbol_obs: Vec<SymbolObs>,
    replay: ReplayBuffer,
    egress: E,
    last_observation: Vec<f32>,
    steps: u64,
    reward_state: RewardState,
}

#[derive(Debug, Clone, Default)]
struct RewardState {
    position: i8,
    last_mid: Option<f32>,
    last_depth: Option<trolly_strategy::DepthUpdate>,
}

impl<E> Env<E>
where
    E: StreamEgress,
{
    pub fn new(config: EnvConfig, egress: E) -> Self {
        let window_frames = config.window_frames;
        let replay_capacity = config.replay_capacity;
        let symbol_obs = config
            .tracked_symbols()
            .into_iter()
            .map(|symbol| SymbolObs {
                symbol: symbol.to_string(),
                window: ObservationWindow::new(window_frames),
                last_depth: None,
                position: 0,
                last_mid: None,
            })
            .collect();
        Self {
            config,
            window: ObservationWindow::new(window_frames),
            symbol_obs,
            replay: ReplayBuffer::new(replay_capacity),
            egress,
            last_observation: Vec::new(),
            steps: 0,
            reward_state: RewardState::default(),
        }
    }

    pub fn symbol(&self) -> &str {
        &self.config.symbol
    }

    pub fn tracked_symbols(&self) -> Vec<&str> {
        self.config.tracked_symbols()
    }

    pub fn observation_window(&self) -> &ObservationWindow {
        &self.window
    }

    pub fn replay_buffer(&self) -> &ReplayBuffer {
        &self.replay
    }

    pub fn replay_buffer_mut(&mut self) -> &mut ReplayBuffer {
        &mut self.replay
    }

    pub fn egress(&self) -> &E {
        &self.egress
    }

    pub fn egress_mut(&mut self) -> &mut E {
        &mut self.egress
    }

    pub fn steps(&self) -> u64 {
        self.steps
    }

    pub fn position(&self) -> i8 {
        self.reward_state.position
    }

    /// Inventory on a tracked pair. Unknown names use the primary book.
    pub fn position_for(&self, symbol: &str) -> i8 {
        if symbol.eq_ignore_ascii_case(&self.config.symbol) {
            return self.reward_state.position;
        }
        self.symbol_obs
            .iter()
            .find(|slot| slot.symbol.eq_ignore_ascii_case(symbol))
            .map(|slot| slot.position)
            .unwrap_or(self.reward_state.position)
    }

    /// Consume one normalized stream event and update the observation window.
    pub fn ingest_event(&mut self, event: &StreamEvent) -> bool {
        if !self.config.tracks_symbol(event.routing_id()) {
            return false;
        }
        if let Some(frame) = features_from_event(event) {
            let is_primary = event.routing_id().eq_ignore_ascii_case(&self.config.symbol);
            if let StreamEvent::Depth(depth) = event {
                if is_primary {
                    self.reward_state.last_depth = Some(depth.clone());
                }
                if let Some(slot) = self.symbol_slot_mut(event.routing_id()) {
                    slot.last_depth = Some(depth.clone());
                }
            }
            if let Some(slot) = self.symbol_slot_mut(event.routing_id()) {
                slot.window.push(frame.clone());
            }
            if is_primary {
                self.window.push(frame);
            }
            self.last_observation = self.policy_observation();
            self.replay.push_observation_window(&self.last_observation);
            crate::ticks::try_ingest_event(event, "stream", "stream");
            true
        } else {
            false
        }
    }

    fn symbol_slot_mut(&mut self, symbol: &str) -> Option<&mut SymbolObs> {
        self.symbol_obs
            .iter_mut()
            .find(|slot| slot.symbol.eq_ignore_ascii_case(symbol))
    }

    /// Ingest a websocket text envelope (stream ingress hook).
    pub fn ingest_message(&mut self, msg: Message) -> Result<bool, trolly_strategy::ParseError> {
        let event = trolly_strategy::parse_envelope(msg)?;
        Ok(self.ingest_event(&event))
    }

    /// Choose/apply an action, dispatch egress, record transition, return step result.
    ///
    /// Pass either an explicit [`Action`] or a borrowed [`PolicyProvider`]:
    /// `env.step(Action::Buy)` and `env.step(&policy)` both use
    /// [`Action::dispatch`] as the only egress path.
    pub fn step<S>(&mut self, source: S) -> Result<StepResult, E::Error>
    where
        S: StepActionSource,
    {
        let stream_observation = self.window.flattened();
        let observation = if self.last_observation.is_empty() {
            self.policy_observation()
        } else {
            self.last_observation.clone()
        };
        let decision = source.select_decision(&observation);
        let action = decision.action;
        let dispatch_symbol = self
            .config
            .resolve_dispatch_symbol(decision.symbol.as_deref())
            .to_string();
        let reward = self.market_reward(&stream_observation, action, &dispatch_symbol);
        let done = self.steps + 1 >= self.config.episode_steps.max(1);

        action.dispatch(
            &mut self.egress,
            &dispatch_symbol,
            &self.config.default_qty,
            None,
        )?;

        self.replay
            .push_step(observation.clone(), action, reward, done);
        self.steps += 1;

        Ok(StepResult {
            observation,
            reward,
            done,
        })
    }

    /// Dispatch a pre-built outbound message (escape hatch for custom policies).
    pub fn dispatch(&mut self, message: OutboundMessage) -> Result<(), E::Error> {
        self.egress.dispatch(message)
    }

    fn market_reward(&mut self, observation: &[f32], action: Action, dispatch_symbol: &str) -> f32 {
        let is_primary = dispatch_symbol.eq_ignore_ascii_case(&self.config.symbol);
        if is_primary || !self.config.is_multi_symbol() {
            return self.apply_market_reward(observation, action, None);
        }
        let extra_obs = self
            .symbol_obs
            .iter()
            .find(|slot| slot.symbol.eq_ignore_ascii_case(dispatch_symbol))
            .map(|slot| slot.window.flattened())
            .filter(|frame| !frame.is_empty())
            .unwrap_or_else(|| observation.to_vec());
        self.apply_market_reward(&extra_obs, action, Some(dispatch_symbol))
    }

    fn apply_market_reward(
        &mut self,
        observation: &[f32],
        action: Action,
        extra_symbol: Option<&str>,
    ) -> f32 {
        let previous_position = match extra_symbol {
            Some(symbol) => self.position_for(symbol),
            None => self.reward_state.position,
        };
        let next_position = action.target_position(previous_position);
        let last_mid = match extra_symbol {
            Some(symbol) => self
                .symbol_obs
                .iter()
                .find(|slot| slot.symbol.eq_ignore_ascii_case(symbol))
                .and_then(|slot| slot.last_mid),
            None => self.reward_state.last_mid,
        };
        let snapshot = MarketSnapshot::from_observation(observation);
        let (delta_mid, spread) = match snapshot {
            Some(snapshot) => {
                let delta = last_mid
                    .map(|last_mid| snapshot.mid - last_mid)
                    .unwrap_or(0.0);
                if let Some(symbol) = extra_symbol {
                    if let Some(slot) = self.symbol_slot_mut(symbol) {
                        slot.last_mid = Some(snapshot.mid);
                        slot.position = next_position;
                    }
                } else {
                    self.reward_state.last_mid = Some(snapshot.mid);
                    self.reward_state.position = next_position;
                    if let Some(slot) = self.symbol_slot_mut(&self.config.symbol.clone()) {
                        slot.last_mid = Some(snapshot.mid);
                        slot.position = next_position;
                    }
                }
                (delta, snapshot.spread)
            }
            None => {
                if let Some(symbol) = extra_symbol {
                    if let Some(slot) = self.symbol_slot_mut(symbol) {
                        slot.position = next_position;
                    }
                } else {
                    self.reward_state.position = next_position;
                }
                (0.0, 0.0)
            }
        };
        let spread_cost = if next_position != previous_position {
            spread * self.config.reward.spread_cost_multiplier
        } else {
            0.0
        };
        next_position as f32 * delta_mid - spread_cost
    }

    fn policy_observation(&self) -> Vec<f32> {
        if self.config.is_multi_symbol()
            && !(self.config.uses_ladder_observation() && !self.config.join_ladder_symbols)
        {
            return self.joined_policy_observation();
        }
        match self.config.observation_layout {
            ObservationLayout::Stream => self.window.flattened(),
            ObservationLayout::Ladder => {
                let q = self.reward_state.position as f32;
                match &self.reward_state.last_depth {
                    Some(depth) => ladder_features_from_depth(depth, &self.config.ladder, q).0,
                    None => ladder_features(&self.config.ladder, q).0,
                }
            }
        }
    }

    fn joined_policy_observation(&self) -> Vec<f32> {
        let q = self.reward_state.position as f32;
        let frames = self
            .symbol_obs
            .iter()
            .map(|slot| match self.config.observation_layout {
                ObservationLayout::Stream => slot
                    .window
                    .latest()
                    .cloned()
                    .unwrap_or_else(zero_stream_features),
                ObservationLayout::Ladder => match &slot.last_depth {
                    Some(depth) => ladder_features_from_depth(depth, &self.config.ladder, q),
                    None => ladder_features(&self.config.ladder, q),
                },
            });
        join_feature_frames(frames)
    }
}

/// Feed injected stream messages into an [`Env`] and step a policy per observation.
///
/// Each message is parsed through the same normalized stream envelope path as
/// [`Env::ingest_message`]. If the message updates this env's symbol, the env
/// calls [`Env::step`] with the provided policy, and that step dispatches only
/// through [`Action::dispatch`].
pub fn run_offline_policy_harness<E, P, I>(
    env: &mut Env<E>,
    policy: &P,
    messages: I,
) -> Result<Vec<StepResult>, OfflinePolicyHarnessError<E::Error>>
where
    E: StreamEgress,
    P: PolicyProvider + ?Sized,
    I: IntoIterator<Item = Message>,
{
    let mut steps = Vec::new();
    for message in messages {
        let ingested = env
            .ingest_message(message)
            .map_err(OfflinePolicyHarnessError::Parse)?;
        if ingested {
            let step = env
                .step(policy)
                .map_err(OfflinePolicyHarnessError::Egress)?;
            steps.push(step);
        }
    }
    Ok(steps)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MarketSnapshot {
    mid: f32,
    spread: f32,
}

impl MarketSnapshot {
    fn from_observation(observation: &[f32]) -> Option<Self> {
        if observation.len() < 7 {
            return None;
        }
        let frame = &observation[observation.len() - 7..];
        Some(Self {
            spread: frame[4],
            mid: frame[5],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trolly_strategy::{DepthUpdate, PriceLevel, RecordingEgress};

    fn depth_event(symbol: &str, bid: &str, ask: &str) -> StreamEvent {
        StreamEvent::Depth(DepthUpdate {
            symbol: symbol.into(),
            bids: vec![PriceLevel {
                price: bid.into(),
                qty: "1".into(),
            }],
            asks: vec![PriceLevel {
                price: ask.into(),
                qty: "1".into(),
            }],
            update_id: Some(1),
        })
    }

    #[test]
    fn ingest_filters_by_symbol() {
        let mut env = Env::new(EnvConfig::new("BTCUSDT"), RecordingEgress::default());
        assert!(!env.ingest_event(&depth_event("ETHUSDT", "100", "101")));
        assert!(env.ingest_event(&depth_event("BTCUSDT", "100", "102")));
        assert_eq!(env.observation_window().len(), 1);
    }

    #[test]
    fn multi_symbol_stream_joins_latest_7d_and_dispatches_primary() {
        let mut config = EnvConfig::new("BTCUSDT");
        config.window_frames = 1;
        config.observe_symbols(["BTCUSDT", "ETHUSDT"]);
        let mut env = Env::new(config, RecordingEgress::default());
        assert_eq!(env.tracked_symbols(), vec!["BTCUSDT", "ETHUSDT"]);
        assert!(env.ingest_event(&depth_event("BTCUSDT", "100", "102")));
        assert!(env.ingest_event(&depth_event("ETHUSDT", "200", "204")));

        let seen = std::cell::Cell::new(0usize);
        let bid0 = std::cell::Cell::new(0.0f32);
        let bid1 = std::cell::Cell::new(0.0f32);
        let policy = |obs: &[f32]| {
            seen.set(obs.len());
            bid0.set(obs[0]);
            bid1.set(obs[7]);
            Action::Buy
        };
        let result = env.step(&policy).unwrap();
        assert_eq!(seen.get(), 14);
        assert!((bid0.get() - 100.0).abs() < f32::EPSILON);
        assert!((bid1.get() - 200.0).abs() < f32::EPSILON);
        assert_eq!(result.observation.len(), 14);
        assert_eq!(
            env.egress().dispatched,
            vec![Action::Buy.to_outbound("BTCUSDT", "0.01", None)]
        );
    }

    #[test]
    fn multi_symbol_ladder_joins_per_symbol_vx5() {
        let mut config = EnvConfig::new("BTCUSDT");
        config.window_frames = 1;
        config.use_ladder_observation();
        config.ladder.rung_count = 2;
        config.observe_symbols(["ETHUSDT"]);
        let mut env = Env::new(config, RecordingEgress::default());
        env.ingest_event(&depth_event("BTCUSDT", "100", "104"));
        env.ingest_event(&depth_event("ETHUSDT", "200", "202"));

        let seen = std::cell::Cell::new(0usize);
        let btc_delta = std::cell::Cell::new(0.0f32);
        let eth_delta = std::cell::Cell::new(0.0f32);
        let policy = |obs: &[f32]| {
            seen.set(obs.len());
            btc_delta.set(obs[1]);
            eth_delta.set(obs[11]);
            Action::Hold
        };
        let result = env.step(&policy).unwrap();
        assert_eq!(seen.get(), 20);
        assert!((btc_delta.get() - 2.0).abs() < 1e-6);
        assert!((eth_delta.get() - 1.0).abs() < 1e-6);
        assert_eq!(result.observation.len(), 20);
        assert_eq!(
            env.egress().dispatched,
            vec![Action::Hold.to_outbound("BTCUSDT", "0.01", None)]
        );
    }

    #[test]
    fn multi_symbol_ladder_can_keep_primary_vx5() {
        let mut config = EnvConfig::new("BTCUSDT");
        config.window_frames = 1;
        config.use_ladder_observation();
        config.join_ladder_symbols = false;
        config.ladder.rung_count = 2;
        config.observe_symbols(["ETHUSDT"]);
        let mut env = Env::new(config, RecordingEgress::default());
        env.ingest_event(&depth_event("BTCUSDT", "100", "104"));
        env.ingest_event(&depth_event("ETHUSDT", "200", "202"));

        let seen = std::cell::Cell::new(0usize);
        let delta = std::cell::Cell::new(0.0f32);
        let policy = |obs: &[f32]| {
            seen.set(obs.len());
            delta.set(obs[1]);
            Action::Buy
        };
        let result = env.step(&policy).unwrap();
        assert_eq!(seen.get(), 10);
        assert!((delta.get() - 2.0).abs() < 1e-6);
        assert_eq!(result.observation.len(), 10);
        assert_eq!(
            env.egress().dispatched,
            vec![Action::Buy.to_outbound("BTCUSDT", "0.01", None)]
        );
    }

    #[test]
    fn multi_symbol_policy_can_dispatch_extra_symbol() {
        let mut config = EnvConfig::new("BTCUSDT");
        config.window_frames = 1;
        config.observe_symbols(["ETHUSDT"]);
        let mut env = Env::new(config, RecordingEgress::default());
        assert!(env.ingest_event(&depth_event("BTCUSDT", "100", "102")));
        assert!(env.ingest_event(&depth_event("ETHUSDT", "200", "204")));

        struct ExtraBookBuy;
        impl PolicyProvider for ExtraBookBuy {
            fn act(&self, obs: &[f32]) -> Action {
                if obs.len() >= 14 && obs[7] > 0.0 {
                    Action::Buy
                } else {
                    Action::Hold
                }
            }

            fn decide(&self, obs: &[f32]) -> ActionDecision {
                let action = self.act(obs);
                if action == Action::Buy {
                    action.on_symbol("ETHUSDT")
                } else {
                    action.into()
                }
            }
        }

        let result = env.step(&ExtraBookBuy).unwrap();
        assert_eq!(result.observation.len(), 14);
        assert_eq!(
            env.egress().dispatched,
            vec![Action::Buy.to_outbound("ETHUSDT", "0.01", None)]
        );
    }

    #[test]
    fn untracked_dispatch_symbol_falls_back_to_primary() {
        let mut env = Env::new(EnvConfig::new("BTCUSDT"), RecordingEgress::default());
        env.ingest_event(&depth_event("BTCUSDT", "100", "102"));
        env.step(Action::Sell.on_symbol("NOTALISTED")).unwrap();
        assert_eq!(
            env.egress().dispatched,
            vec![Action::Sell.to_outbound("BTCUSDT", "0.01", None)]
        );
    }

    #[test]
    fn config_dispatch_symbol_pins_when_policy_omits_one() {
        let mut config = EnvConfig::new("BTCUSDT");
        config.window_frames = 1;
        config.default_qty = "0.02".into();
        config.set_dispatch_symbol("ETHUSDT");
        assert_eq!(config.tracked_symbols(), vec!["BTCUSDT", "ETHUSDT"]);
        let mut env = Env::new(config, RecordingEgress::default());
        env.ingest_event(&depth_event("BTCUSDT", "100", "102"));
        env.ingest_event(&depth_event("ETHUSDT", "200", "204"));
        env.step(Action::Buy).unwrap();
        assert_eq!(
            env.egress().dispatched,
            vec![Action::Buy.to_outbound("ETHUSDT", "0.02", None)]
        );
    }

    #[test]
    fn extra_symbol_buy_uses_that_book_spread() {
        let mut config = EnvConfig::new("BTCUSDT");
        config.window_frames = 1;
        config.observe_symbols(["ETHUSDT"]);
        let mut env = Env::new(config, RecordingEgress::default());
        assert!(env.ingest_event(&depth_event("BTCUSDT", "100", "102")));
        assert!(env.ingest_event(&depth_event("ETHUSDT", "200", "204")));

        let result = env.step(Action::Buy.on_symbol("ETHUSDT")).unwrap();
        assert_eq!(result.reward, -4.0);
        assert_eq!(env.position(), 0);
        assert_eq!(env.position_for("ETHUSDT"), 1);
        assert_eq!(
            env.egress().dispatched,
            vec![Action::Buy.to_outbound("ETHUSDT", "0.01", None)]
        );
    }

    #[test]
    fn step_dispatches_buy_order() {
        let mut env = Env::new(EnvConfig::new("BTCUSDT"), RecordingEgress::default());
        env.ingest_event(&depth_event("BTCUSDT", "100", "102"));

        let result = env.step(Action::Buy).unwrap();
        assert!(!result.observation.is_empty());
        assert_eq!(result.reward, -2.0);
        assert_eq!(env.position(), 1);
        assert_eq!(env.replay_buffer().len(), 2); // ingest prefill + step
        assert_eq!(
            env.egress().dispatched[0],
            Action::Buy.to_outbound("BTCUSDT", "0.01", None)
        );
    }

    #[test]
    fn step_accepts_policy_provider_and_marks_done_at_horizon() {
        let mut config = EnvConfig::new("BTCUSDT");
        config.episode_steps = 2;
        config.reward.spread_cost_multiplier = 0.5;
        let mut env = Env::new(config, RecordingEgress::default());

        env.ingest_event(&depth_event("BTCUSDT", "100", "102"));
        let buy_policy = |_obs: &[f32]| Action::Buy;
        let first = env.step(&buy_policy).unwrap();
        assert!(!first.done);
        assert_eq!(first.reward, -1.0);

        env.ingest_event(&depth_event("BTCUSDT", "101", "103"));
        let hold_policy = |_obs: &[f32]| Action::Hold;
        let second = env.step(&hold_policy).unwrap();
        assert!(second.done);
        assert_eq!(second.reward, 1.0);
        assert_eq!(
            env.egress().dispatched,
            vec![
                Action::Buy.to_outbound("BTCUSDT", "0.01", None),
                Action::Hold.to_outbound("BTCUSDT", "0.01", None),
            ]
        );
    }

    #[test]
    fn ladder_layout_feeds_vx5_from_depth_and_inventory() {
        let mut config = EnvConfig::new("BTCUSDT");
        config.window_frames = 1;
        config.use_ladder_observation();
        config.ladder.rung_count = 4;
        let mut env = Env::new(config, RecordingEgress::default());
        env.ingest_event(&depth_event("BTCUSDT", "100", "104"));

        let seen = std::cell::Cell::new(0usize);
        let delta = std::cell::Cell::new(0.0f32);
        let inventory = std::cell::Cell::new(99.0f32);
        let policy = |obs: &[f32]| {
            seen.set(obs.len());
            delta.set(obs[1]);
            inventory.set(obs[4]);
            Action::Hold
        };
        let result = env.step(&policy).unwrap();
        assert_eq!(seen.get(), 20);
        assert!((delta.get() - 2.0).abs() < 1e-6);
        assert_eq!(inventory.get(), 0.0);
        assert_eq!(result.observation.len(), 20);
        assert_eq!(env.observation_window().flattened().len(), 7);
    }

    #[test]
    fn offline_policy_harness_steps_only_ingested_symbol() {
        let mut config = EnvConfig::new("BTCUSDT");
        config.window_frames = 1;
        let mut env = Env::new(config, RecordingEgress::default());
        let policy = |_obs: &[f32]| Action::Buy;

        let messages = [
            trolly_strategy::envelope_message(&depth_event("ETHUSDT", "100", "102")),
            trolly_strategy::envelope_message(&depth_event("BTCUSDT", "100", "102")),
        ];

        let steps = run_offline_policy_harness(&mut env, &policy, messages).unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(
            env.egress().dispatched,
            vec![Action::Buy.to_outbound("BTCUSDT", "0.01", None)]
        );
    }

    #[cfg(feature = "torch")]
    #[test]
    fn offline_policy_harness_loads_checkpoint_policy() {
        use crate::policy::CheckpointOrHoldPolicy;
        use crate::ppo::{ActorCritic, PpoConfig};
        use crate::train::{save_checkpoint, LATEST_CHECKPOINT};
        use tch::{nn, Device};

        let obs_dim = 7_i64;
        let ppo = PpoConfig::default();
        let vs = nn::VarStore::new(Device::Cpu);
        let _model = ActorCritic::new(&vs, obs_dim, Action::COUNT, &ppo);

        let dir = std::env::temp_dir().join(format!(
            "trolly_gym_env_checkpoint_harness_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        save_checkpoint(&vs, dir.join(LATEST_CHECKPOINT)).unwrap();

        let policy = CheckpointOrHoldPolicy::from_latest_checkpoint_dir(&dir, obs_dim, ppo)
            .expect("checkpoint loads");
        let mut config = EnvConfig::new("BTCUSDT");
        config.window_frames = 1;
        let mut env = Env::new(config, RecordingEgress::default());

        let steps = run_offline_policy_harness(
            &mut env,
            &policy,
            [trolly_strategy::envelope_message(&depth_event(
                "BTCUSDT", "100", "101",
            ))],
        )
        .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(env.egress().dispatched.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
