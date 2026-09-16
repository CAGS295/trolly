//! Guarded demo bridge from policy harness output to execution adapters.

use std::{
    collections::{BTreeMap, HashSet},
    env, fmt,
    future::Future,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use binance_spot_exec::{
    build_multiplexor as build_spot_multiplexor, ingest_user_data as ingest_spot_user_data,
    AccountBook, ApiCredentials as SpotCredentials, BinanceSpotUserStream,
    NativeTlsTransport as SpotNativeTlsTransport, PlaceOrderError as SpotPlaceOrderError,
    PlaceOrderRequest as SpotPlaceOrderRequest, PlaceOrderResponse as SpotPlaceOrderResponse,
    SpotExecContext, SpotOrderClient, SpotOrderEgress, SpotUserEvent, SPOT_DEMO_MARKET_STREAM_URL,
    SPOT_DEMO_ORDER_BASE_URL, SPOT_DEMO_REST_BASE_URL, spot_depth_rest_url,
};
use binance_usdm_exec::{
    build_multiplexor_with_context as build_usdm_multiplexor_with_context,
    ingest_user_data as ingest_usdm_user_data, ApiCredentials as UsdmCredentials, ListenKeyClient,
    ListenKeyError, NativeTlsTransport as UsdmNativeTlsTransport,
    PlaceOrderError as UsdmPlaceOrderError, PlaceOrderRequest as UsdmPlaceOrderRequest,
    PlaceOrderResponse as UsdmPlaceOrderResponse, UsdmExecContext, UsdmExecUpdate, UsdmOrderClient,
    UsdmOrderEgress, UsdmUserDataStream, USDM_DEMO_MARKET_STREAM_URL, USDM_DEMO_REST_BASE_URL,
    usdm_depth_rest_url,
};
use futures_util::{SinkExt, StreamExt};
use http::Uri;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::{timeout, Instant};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use trolly_gym::policy::DEFAULT_INVENTORY_DEADZONE;
use trolly_gym::{
    run_offline_policy_harness, CheckpointOrHoldPolicy, Env, EnvConfig, PolicyProvider,
};
use trolly_strategy::{
    envelope_message, parse_envelope, DepthUpdate, OrderOnlyEgress, PriceLevel, StreamEvent,
};
use trolly_stream::{Message, VenueEndpoints};

pub const DEFAULT_DEMO_ORDER_GUARD_VAR: &str = "RUN_BINANCE_DEMO_ORDERS";
pub const DEFAULT_DEMO_USER_DATA_TIMEOUT: Duration = Duration::from_secs(45);

type DemoWebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoVenue {
    Spot,
    Usdm,
}

impl fmt::Display for DemoVenue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spot => f.write_str("spot"),
            Self::Usdm => f.write_str("usdm"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PolicyDemoConfig {
    pub venue: DemoVenue,
    pub symbol: String,
    pub qty: String,
    pub window_frames: usize,
    pub max_steps: usize,
    pub execute_demo_orders: bool,
    pub demo_order_guard_var: String,
    pub client_order_id_prefix: String,
    pub wait_for_user_data: bool,
    pub user_data_timeout: Duration,
    /// Recorded tanh-Gaussian mean actions (`0.8,-0.8,0.1`). Quantized via WP-035.
    pub gaussian_mean_actions: Option<String>,
    /// Torch Gaussian ladder checkpoint dir (`microstructure/gaussian_mlp`).
    pub gaussian_checkpoint_dir: Option<String>,
    /// Optional ONNX Gaussian μ head (`[1, V×5] → [1]`). Requires `gym-ort`.
    pub onnx_gaussian_model_path: Option<String>,
    /// Deadzone for [`trolly_gym::Action::quantize_inventory`].
    pub inventory_hold_deadzone: f32,
    /// Captured depth JSON/NDJSON (normalized `StreamEvent` or Binance book).
    /// When unset, the runner feeds the synthetic fixture tape.
    pub captured_depth_json: Option<String>,
    /// Pre-parsed public depth frames from an injectable source (WP-042).
    /// Used when `captured_depth_json` is unset so a later live WS can reuse the hook.
    pub public_depth_messages: Option<Vec<Message>>,
    /// Connect to Binance public depth (demo market stream) matching `venue`.
    /// `--depth-json` / `captured_depth_json` still wins. Requires a bounded timeout.
    pub subscribe_public_depth: bool,
    /// Maximum time to collect subscribed public depth frames.
    pub public_depth_timeout: Duration,
    /// Optional REST-style depth snapshot JSON used to seed the local book
    /// before applying subscribed/injected `depthUpdate` diffs (WP-044).
    pub public_depth_snapshot_json: Option<String>,
}

impl PolicyDemoConfig {
    pub fn new(venue: DemoVenue, symbol: impl Into<String>) -> Self {
        Self {
            venue,
            symbol: symbol.into(),
            qty: "0.01".into(),
            window_frames: 1,
            max_steps: 3,
            execute_demo_orders: false,
            demo_order_guard_var: DEFAULT_DEMO_ORDER_GUARD_VAR.into(),
            client_order_id_prefix: "trolly-demo".into(),
            wait_for_user_data: false,
            user_data_timeout: DEFAULT_DEMO_USER_DATA_TIMEOUT,
            gaussian_mean_actions: None,
            gaussian_checkpoint_dir: None,
            onnx_gaussian_model_path: None,
            inventory_hold_deadzone: DEFAULT_INVENTORY_DEADZONE,
            captured_depth_json: None,
            public_depth_messages: None,
            subscribe_public_depth: false,
            public_depth_timeout: Duration::ZERO,
            public_depth_snapshot_json: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PolicyDemoReport {
    pub venue: DemoVenue,
    pub symbol: String,
    pub policy_source: String,
    pub depth_source: String,
    pub steps: usize,
    pub execute_demo_orders: bool,
    pub placed_orders: usize,
    pub receipts: Vec<PolicyDemoReceipt>,
    pub reconciliations: Vec<PolicyDemoReconciliation>,
    pub orders: PolicyDemoOrders,
}

impl PolicyDemoReport {
    pub fn order_count(&self) -> usize {
        self.orders.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDemoReceipt {
    pub venue: DemoVenue,
    pub symbol: String,
    pub order_id: i64,
    pub client_order_id: String,
    pub status: String,
    pub side: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDemoReconciliation {
    pub venue: DemoVenue,
    pub symbol: String,
    pub order_id: i64,
    pub client_order_id: String,
    pub status: String,
    pub side: String,
    pub terminal: bool,
}

#[derive(Debug, Clone)]
pub enum PolicyDemoOrders {
    Spot(Vec<SpotPlaceOrderRequest>),
    Usdm(Vec<UsdmPlaceOrderRequest>),
}

impl PolicyDemoOrders {
    pub fn len(&self) -> usize {
        match self {
            Self::Spot(orders) => orders.len(),
            Self::Usdm(orders) => orders.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug)]
pub enum PolicyDemoError {
    MissingDemoOrderGuard { var: String },
    MissingDemoCredentials,
    Harness(String),
    DepthInput(String),
    ReconciliationInput(String),
    LiveReconciliation(String),
    SpotPlaceOrder(SpotPlaceOrderError),
    UsdmPlaceOrder(UsdmPlaceOrderError),
}

impl fmt::Display for PolicyDemoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingDemoOrderGuard { var } => {
                write!(f, "refusing demo order placement: set {var}=1")
            }
            Self::MissingDemoCredentials => write!(
                f,
                "missing demo credentials: set DEMO_BINANCE_KEY and DEMO_BINANCE_SECRET"
            ),
            Self::Harness(err) => write!(f, "policy demo harness failed: {err}"),
            Self::DepthInput(err) => write!(f, "policy demo depth input failed: {err}"),
            Self::ReconciliationInput(err) => {
                write!(f, "policy demo reconciliation input failed: {err}")
            }
            Self::LiveReconciliation(err) => {
                write!(f, "policy demo live reconciliation failed: {err}")
            }
            Self::SpotPlaceOrder(err) => write!(f, "spot demo order placement failed: {err}"),
            Self::UsdmPlaceOrder(err) => write!(f, "USDM demo order placement failed: {err}"),
        }
    }
}

impl std::error::Error for PolicyDemoError {}

#[derive(Debug, Clone)]
struct DemoCredentials {
    api_key: String,
    secret_key: String,
}

impl DemoCredentials {
    fn from_env() -> Result<Self, PolicyDemoError> {
        let api_key =
            env::var("DEMO_BINANCE_KEY").map_err(|_| PolicyDemoError::MissingDemoCredentials)?;
        let secret_key =
            env::var("DEMO_BINANCE_SECRET").map_err(|_| PolicyDemoError::MissingDemoCredentials)?;
        Ok(Self {
            api_key,
            secret_key,
        })
    }
}

pub async fn run_policy_demo(
    config: PolicyDemoConfig,
) -> Result<PolicyDemoReport, PolicyDemoError> {
    let (policy, policy_source) = load_policy(&config);
    if should_subscribe_public_depth(&config) {
        ensure_public_depth_subscribe_config(&config)?;
        let venue = config.venue;
        let symbol = config.symbol.clone();
        let max_steps = config.max_steps;
        let timeout = config.public_depth_timeout;
        return run_policy_demo_with_public_depth(config, &policy, policy_source, move || {
            subscribe_binance_public_depth(venue, symbol, max_steps, timeout)
        })
        .await;
    }
    run_policy_demo_with_policy(config, &policy, policy_source).await
}

/// Run the demo harness with frames from an injectable public-depth source.
///
/// `--depth-json` still wins when `captured_depth_json` is set. The source is
/// the hook a later live/demo WebSocket should call.
pub async fn run_policy_demo_with_public_depth<P, S, Fut>(
    mut config: PolicyDemoConfig,
    policy: &P,
    policy_source: impl Into<String>,
    source: S,
) -> Result<PolicyDemoReport, PolicyDemoError>
where
    P: PolicyProvider + ?Sized,
    S: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<Message>, PolicyDemoError>>,
{
    ensure_public_depth_subscribe_config(&config)?;
    let messages = source().await?;
    if messages.is_empty() {
        return Err(PolicyDemoError::DepthInput(
            "public depth source returned no frames".into(),
        ));
    }
    config.public_depth_messages = Some(messages);
    run_policy_demo_with_policy(config, policy, policy_source).await
}

pub async fn run_policy_demo_with_policy<P>(
    config: PolicyDemoConfig,
    policy: &P,
    policy_source: impl Into<String>,
) -> Result<PolicyDemoReport, PolicyDemoError>
where
    P: PolicyProvider + ?Sized,
{
    ensure_live_reconciliation_config(&config)?;
    ensure_public_depth_subscribe_config(&config)?;

    let credentials = if config.execute_demo_orders {
        ensure_demo_order_guard(&config.demo_order_guard_var)?;
        Some(DemoCredentials::from_env()?)
    } else {
        None
    };

    match config.venue {
        DemoVenue::Spot => {
            run_spot_policy_demo(config, policy, policy_source.into(), credentials).await
        }
        DemoVenue::Usdm => {
            run_usdm_policy_demo(config, policy, policy_source.into(), credentials).await
        }
    }
}

async fn run_spot_policy_demo<P>(
    config: PolicyDemoConfig,
    policy: &P,
    policy_source: String,
    credentials: Option<DemoCredentials>,
) -> Result<PolicyDemoReport, PolicyDemoError>
where
    P: PolicyProvider + ?Sized,
{
    let (spot_egress, mut rx) = SpotOrderEgress::channel();
    let steps = run_env_policy_harness(&config, OrderOnlyEgress::new(spot_egress), policy)?;
    let mut orders = drain_spot_orders(&mut rx);
    assign_spot_client_order_ids(&mut orders, &config.client_order_id_prefix);

    let (receipts, mut live_socket) = if let Some(credentials) = credentials {
        let live_socket = if config.wait_for_user_data {
            Some(prepare_spot_live_user_data(&credentials, config.user_data_timeout).await?)
        } else {
            None
        };
        (
            place_spot_demo_orders(credentials, &orders).await?,
            live_socket,
        )
    } else {
        (Vec::new(), None)
    };

    let depth_source = policy_demo_depth_source_label(&config).to_string();
    let mut report = PolicyDemoReport {
        venue: DemoVenue::Spot,
        symbol: config.symbol,
        policy_source,
        depth_source,
        steps,
        execute_demo_orders: config.execute_demo_orders,
        placed_orders: receipts.len(),
        receipts,
        reconciliations: Vec::new(),
        orders: PolicyDemoOrders::Spot(orders),
    };

    if let Some(socket) = live_socket.as_mut() {
        report.reconciliations =
            wait_spot_live_reconciliations(socket, &report, config.user_data_timeout).await?;
    }

    Ok(report)
}

async fn run_usdm_policy_demo<P>(
    config: PolicyDemoConfig,
    policy: &P,
    policy_source: String,
    credentials: Option<DemoCredentials>,
) -> Result<PolicyDemoReport, PolicyDemoError>
where
    P: PolicyProvider + ?Sized,
{
    let (usdm_egress, mut rx) = UsdmOrderEgress::channel();
    let steps = run_env_policy_harness(&config, OrderOnlyEgress::new(usdm_egress), policy)?;
    let mut orders = drain_usdm_orders(&mut rx);
    assign_usdm_client_order_ids(&mut orders, &config.client_order_id_prefix);

    let (receipts, live_user_data) = if let Some(credentials) = credentials {
        let live_user_data = if config.wait_for_user_data {
            Some(prepare_usdm_live_user_data(&credentials).await?)
        } else {
            None
        };
        let receipts = match place_usdm_demo_orders(credentials, &orders).await {
            Ok(receipts) => receipts,
            Err(err) => {
                if let Some(live) = live_user_data {
                    let _ = live.listen_client.close().await;
                }
                return Err(err);
            }
        };
        (receipts, live_user_data)
    } else {
        (Vec::new(), None)
    };

    let depth_source = policy_demo_depth_source_label(&config).to_string();
    let mut report = PolicyDemoReport {
        venue: DemoVenue::Usdm,
        symbol: config.symbol,
        policy_source,
        depth_source,
        steps,
        execute_demo_orders: config.execute_demo_orders,
        placed_orders: receipts.len(),
        receipts,
        reconciliations: Vec::new(),
        orders: PolicyDemoOrders::Usdm(orders),
    };

    if let Some(mut live) = live_user_data {
        let reconciliations =
            wait_usdm_live_reconciliations(&mut live.socket, &report, config.user_data_timeout)
                .await;
        let _ = live.listen_client.close().await;
        report.reconciliations = reconciliations?;
    }

    Ok(report)
}

#[doc(hidden)]
pub async fn run_spot_policy_demo_with_placer<P, F, Fut>(
    config: PolicyDemoConfig,
    policy: &P,
    policy_source: impl Into<String>,
    mut place_order: F,
) -> Result<PolicyDemoReport, PolicyDemoError>
where
    P: PolicyProvider + ?Sized,
    F: FnMut(SpotPlaceOrderRequest) -> Fut,
    Fut: Future<Output = Result<SpotPlaceOrderResponse, PolicyDemoError>>,
{
    let (spot_egress, mut rx) = SpotOrderEgress::channel();
    let steps = run_env_policy_harness(&config, OrderOnlyEgress::new(spot_egress), policy)?;
    let mut orders = drain_spot_orders(&mut rx);
    assign_spot_client_order_ids(&mut orders, &config.client_order_id_prefix);

    let mut receipts = Vec::new();
    if config.execute_demo_orders {
        for order in orders.iter().cloned() {
            let response = place_order(order).await?;
            receipts.push(spot_receipt_from_response(response));
        }
    }

    let depth_source = policy_demo_depth_source_label(&config).to_string();
    Ok(PolicyDemoReport {
        venue: DemoVenue::Spot,
        symbol: config.symbol,
        policy_source: policy_source.into(),
        depth_source,
        steps,
        execute_demo_orders: config.execute_demo_orders,
        placed_orders: receipts.len(),
        receipts,
        reconciliations: Vec::new(),
        orders: PolicyDemoOrders::Spot(orders),
    })
}

#[doc(hidden)]
pub async fn run_usdm_policy_demo_with_placer<P, F, Fut>(
    config: PolicyDemoConfig,
    policy: &P,
    policy_source: impl Into<String>,
    mut place_order: F,
) -> Result<PolicyDemoReport, PolicyDemoError>
where
    P: PolicyProvider + ?Sized,
    F: FnMut(UsdmPlaceOrderRequest) -> Fut,
    Fut: Future<Output = Result<UsdmPlaceOrderResponse, PolicyDemoError>>,
{
    let (usdm_egress, mut rx) = UsdmOrderEgress::channel();
    let steps = run_env_policy_harness(&config, OrderOnlyEgress::new(usdm_egress), policy)?;
    let mut orders = drain_usdm_orders(&mut rx);
    assign_usdm_client_order_ids(&mut orders, &config.client_order_id_prefix);

    let mut receipts = Vec::new();
    if config.execute_demo_orders {
        for order in orders.iter().cloned() {
            let response = place_order(order).await?;
            receipts.push(usdm_receipt_from_response(response));
        }
    }

    let depth_source = policy_demo_depth_source_label(&config).to_string();
    Ok(PolicyDemoReport {
        venue: DemoVenue::Usdm,
        symbol: config.symbol,
        policy_source: policy_source.into(),
        depth_source,
        steps,
        execute_demo_orders: config.execute_demo_orders,
        placed_orders: receipts.len(),
        receipts,
        reconciliations: Vec::new(),
        orders: PolicyDemoOrders::Usdm(orders),
    })
}

#[doc(hidden)]
pub async fn run_spot_policy_demo_with_placer_and_user_data<P, F, Fut, S, SFut>(
    config: PolicyDemoConfig,
    policy: &P,
    policy_source: impl Into<String>,
    place_order: F,
    user_data_messages: S,
) -> Result<PolicyDemoReport, PolicyDemoError>
where
    P: PolicyProvider + ?Sized,
    F: FnMut(SpotPlaceOrderRequest) -> Fut,
    Fut: Future<Output = Result<SpotPlaceOrderResponse, PolicyDemoError>>,
    S: FnOnce(&PolicyDemoReport) -> SFut,
    SFut: Future<Output = Result<Vec<Message>, PolicyDemoError>>,
{
    ensure_live_reconciliation_config(&config)?;
    let should_wait = config.wait_for_user_data;
    let mut report =
        run_spot_policy_demo_with_placer(config, policy, policy_source, place_order).await?;
    if should_wait {
        let messages = user_data_messages(&report).await?;
        reconcile_policy_demo_report(&mut report, messages);
    }
    Ok(report)
}

#[doc(hidden)]
pub async fn run_usdm_policy_demo_with_placer_and_user_data<P, F, Fut, S, SFut>(
    config: PolicyDemoConfig,
    policy: &P,
    policy_source: impl Into<String>,
    place_order: F,
    user_data_messages: S,
) -> Result<PolicyDemoReport, PolicyDemoError>
where
    P: PolicyProvider + ?Sized,
    F: FnMut(UsdmPlaceOrderRequest) -> Fut,
    Fut: Future<Output = Result<UsdmPlaceOrderResponse, PolicyDemoError>>,
    S: FnOnce(&PolicyDemoReport) -> SFut,
    SFut: Future<Output = Result<Vec<Message>, PolicyDemoError>>,
{
    ensure_live_reconciliation_config(&config)?;
    let should_wait = config.wait_for_user_data;
    let mut report =
        run_usdm_policy_demo_with_placer(config, policy, policy_source, place_order).await?;
    if should_wait {
        let messages = user_data_messages(&report).await?;
        reconcile_policy_demo_report(&mut report, messages);
    }
    Ok(report)
}

pub fn reconcile_policy_demo_report(
    report: &mut PolicyDemoReport,
    messages: impl IntoIterator<Item = Message>,
) {
    report.reconciliations = match report.venue {
        DemoVenue::Spot => reconcile_spot_policy_demo_user_data(report, messages),
        DemoVenue::Usdm => reconcile_usdm_policy_demo_user_data(report, messages),
    };
}

pub fn policy_demo_user_data_messages_from_json(
    input: &str,
) -> Result<Vec<Message>, PolicyDemoError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return match value {
            Value::Array(values) => values
                .into_iter()
                .map(json_value_to_message)
                .collect::<Result<Vec<_>, _>>(),
            other => Ok(vec![json_value_to_message(other)?]),
        };
    }

    Ok(input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| Message::Text(line.to_owned().into()))
        .collect())
}

fn json_value_to_message(value: Value) -> Result<Message, PolicyDemoError> {
    match value {
        Value::String(text) => Ok(Message::Text(text.into())),
        other => serde_json::to_string(&other)
            .map(|text| Message::Text(text.into()))
            .map_err(|err| PolicyDemoError::ReconciliationInput(err.to_string())),
    }
}

fn reconcile_spot_policy_demo_user_data(
    report: &PolicyDemoReport,
    messages: impl IntoIterator<Item = Message>,
) -> Vec<PolicyDemoReconciliation> {
    let mut state = ReconciliationState::from_report(report);
    if state.is_empty() {
        return Vec::new();
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    let account = Arc::new(Mutex::new(AccountBook::default()));
    let ctx = SpotExecContext {
        events: tx,
        account,
    };
    let mut hub = build_spot_multiplexor(&[report.symbol.as_str()], ctx);

    for message in messages {
        ingest_spot_user_data(&mut hub, message);
        drain_spot_reconciliation_events(&mut rx, &mut state);
    }

    state.into_reconciliations()
}

fn reconcile_usdm_policy_demo_user_data(
    report: &PolicyDemoReport,
    messages: impl IntoIterator<Item = Message>,
) -> Vec<PolicyDemoReconciliation> {
    let mut state = ReconciliationState::from_report(report);
    if state.is_empty() {
        return Vec::new();
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    let ctx = UsdmExecContext::new(Some(tx));
    let mut hub = build_usdm_multiplexor_with_context(&[report.symbol.as_str()], ctx);

    for message in messages {
        ingest_usdm_user_data(&mut hub, message);
        drain_usdm_reconciliation_events(&mut rx, &mut state);
    }

    state.into_reconciliations()
}

struct ReconciliationState {
    targets: Vec<(i64, String)>,
    reconciliations: Vec<PolicyDemoReconciliation>,
    seen: HashSet<(i64, String)>,
}

impl ReconciliationState {
    fn from_report(report: &PolicyDemoReport) -> Self {
        Self {
            targets: report
                .receipts
                .iter()
                .map(|receipt| (receipt.order_id, receipt.client_order_id.clone()))
                .collect(),
            reconciliations: Vec::new(),
            seen: HashSet::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    fn is_terminal_complete(&self) -> bool {
        !self.targets.is_empty()
            && self.targets.iter().all(|(order_id, client_order_id)| {
                self.reconciliations.iter().any(|reconciliation| {
                    (reconciliation.order_id == *order_id
                        || reconciliation.client_order_id == *client_order_id)
                        && reconciliation.terminal
                })
            })
    }

    fn matches(&self, order_id: i64, client_order_id: &str) -> bool {
        self.targets
            .iter()
            .any(|(target_order_id, target_client_order_id)| {
                *target_order_id == order_id || target_client_order_id == client_order_id
            })
    }

    fn upsert(&mut self, reconciliation: PolicyDemoReconciliation) {
        let key = (
            reconciliation.order_id,
            reconciliation.client_order_id.clone(),
        );
        if !self.seen.insert(key) {
            if let Some(existing) = self.reconciliations.iter_mut().find(|existing| {
                existing.order_id == reconciliation.order_id
                    && existing.client_order_id == reconciliation.client_order_id
            }) {
                *existing = reconciliation;
            }
            return;
        }
        self.reconciliations.push(reconciliation);
    }

    fn into_reconciliations(self) -> Vec<PolicyDemoReconciliation> {
        self.reconciliations
    }
}

fn drain_spot_reconciliation_events(
    rx: &mut mpsc::UnboundedReceiver<SpotUserEvent>,
    state: &mut ReconciliationState,
) {
    while let Ok(event) = rx.try_recv() {
        let SpotUserEvent::ExecutionReport(execution) = event else {
            continue;
        };
        if !state.matches(execution.order_id, &execution.client_order_id) {
            continue;
        }
        state.upsert(PolicyDemoReconciliation {
            venue: DemoVenue::Spot,
            symbol: execution.symbol,
            order_id: execution.order_id,
            client_order_id: execution.client_order_id,
            status: execution.order_status.clone(),
            side: execution.side,
            terminal: is_terminal_order_status(&execution.order_status),
        });
    }
}

fn drain_usdm_reconciliation_events(
    rx: &mut mpsc::UnboundedReceiver<UsdmExecUpdate>,
    state: &mut ReconciliationState,
) {
    while let Ok(event) = rx.try_recv() {
        let UsdmExecUpdate::OrderTrade(execution) = event else {
            continue;
        };
        if !state.matches(execution.order_id, &execution.client_order_id) {
            continue;
        }
        state.upsert(PolicyDemoReconciliation {
            venue: DemoVenue::Usdm,
            symbol: execution.symbol,
            order_id: execution.order_id,
            client_order_id: execution.client_order_id,
            status: execution.order_status.clone(),
            side: execution.side,
            terminal: is_terminal_order_status(&execution.order_status),
        });
    }
}

fn is_terminal_order_status(status: &str) -> bool {
    matches!(status, "FILLED" | "CANCELED" | "EXPIRED" | "REJECTED")
}

fn run_env_policy_harness<E, P>(
    config: &PolicyDemoConfig,
    egress: E,
    policy: &P,
) -> Result<usize, PolicyDemoError>
where
    E: trolly_strategy::StreamEgress,
    E::Error: fmt::Debug,
    P: PolicyProvider + ?Sized,
{
    let mut env_config = EnvConfig::new(config.symbol.clone());
    env_config.default_qty = config.qty.clone();
    env_config.window_frames = config.window_frames;
    env_config.episode_steps = config.max_steps.max(1) as u64;
    if uses_gaussian_source(config) {
        env_config.use_ladder_observation();
    }

    let mut env = Env::new(env_config, egress);
    let messages = depth_messages_for_config(config)?;
    let steps = run_offline_policy_harness(&mut env, policy, messages)
        .map_err(|err| PolicyDemoError::Harness(err.to_string()))?;
    Ok(steps.len())
}

fn depth_messages_for_config(config: &PolicyDemoConfig) -> Result<Vec<Message>, PolicyDemoError> {
    if let Some(input) = &config.captured_depth_json {
        let messages = policy_demo_depth_messages_from_json(input, &config.symbol)?;
        if messages.is_empty() {
            return Err(PolicyDemoError::DepthInput(
                "captured depth JSON contained no usable frames".into(),
            ));
        }
        return Ok(truncate_depth_messages(messages, config.max_steps));
    }
    if let Some(messages) = &config.public_depth_messages {
        if messages.is_empty() {
            return Err(PolicyDemoError::DepthInput(
                "public depth source returned no frames".into(),
            ));
        }
        return Ok(truncate_depth_messages(messages.clone(), config.max_steps));
    }
    Ok(synthetic_depth_stream(&config.symbol, config.max_steps))
}

fn truncate_depth_messages(mut messages: Vec<Message>, max_steps: usize) -> Vec<Message> {
    if max_steps > 0 && messages.len() > max_steps {
        messages.truncate(max_steps);
    }
    messages
}

pub fn policy_demo_depth_source_label(config: &PolicyDemoConfig) -> &'static str {
    if config.captured_depth_json.is_some() {
        "captured-json"
    } else if config.public_depth_messages.is_some() && config.subscribe_public_depth {
        "subscribed"
    } else if config.public_depth_messages.is_some() {
        "injected"
    } else {
        "synthetic"
    }
}

fn should_subscribe_public_depth(config: &PolicyDemoConfig) -> bool {
    config.subscribe_public_depth && config.captured_depth_json.is_none()
}

/// Demo public-depth WebSocket URL for `--subscribe-public-depth` (never production hosts).
pub fn public_depth_stream_url(venue: DemoVenue) -> String {
    match venue {
        DemoVenue::Spot => SPOT_DEMO_MARKET_STREAM_URL.to_string(),
        DemoVenue::Usdm => format!(
            "{}/stream",
            USDM_DEMO_MARKET_STREAM_URL.trim_end_matches('/')
        ),
    }
}

/// Combined-stream name `{symbol}@depth` used in the SUBSCRIBE payload.
pub fn public_depth_stream_name(symbol: &str) -> String {
    format!("{}@depth", symbol.trim().to_ascii_lowercase())
}

/// Binance public depth `SUBSCRIBE` JSON matching the existing depth providers.
pub fn public_depth_subscribe_request_json(symbol: &str) -> String {
    format!(
        r#"{{"method":"SUBSCRIBE","params":["{}"],"id":1}}"#,
        public_depth_stream_name(symbol)
    )
}

/// Demo REST depth snapshot URL for `--subscribe-public-depth` (never production hosts).
pub fn public_depth_snapshot_url(venue: DemoVenue, symbol: &str) -> String {
    match venue {
        DemoVenue::Spot => spot_depth_rest_url(SPOT_DEMO_REST_BASE_URL, symbol, 100),
        DemoVenue::Usdm => usdm_depth_rest_url(USDM_DEMO_REST_BASE_URL, symbol, 100),
    }
}

/// Rebuild snapshot-style depth envelopes from a REST book plus WS diffs.
///
/// Qty `0` removes a level. Diffs with `u`/`lastUpdateId` ≤ the snapshot id are
/// skipped. Each applied snapshot or diff emits the current top of book so
/// `Env` `features_from_event` (`.first()` bid/ask) sees a coherent book.
pub fn policy_demo_messages_from_public_depth_book(
    snapshot_json: &str,
    diff_texts: impl IntoIterator<Item = impl AsRef<str>>,
    default_symbol: &str,
) -> Result<Vec<Message>, PolicyDemoError> {
    let snapshot_messages = policy_demo_depth_messages_from_json(snapshot_json, default_symbol)?;
    let Some(first) = snapshot_messages.into_iter().next() else {
        return Err(PolicyDemoError::DepthInput(
            "public depth snapshot contained no usable frames".into(),
        ));
    };
    let snapshot_event = parse_envelope(first).map_err(|err| {
        PolicyDemoError::DepthInput(format!("public depth snapshot envelope: {err}"))
    })?;
    let StreamEvent::Depth(snapshot) = snapshot_event else {
        return Err(PolicyDemoError::DepthInput(
            "public depth snapshot is not a depth frame".into(),
        ));
    };

    let mut book = PublicDepthBook::from_snapshot(snapshot);
    let mut messages = vec![envelope_message(&StreamEvent::Depth(book.to_depth_update()))];
    let diffs = policy_demo_public_depth_messages_from_texts(diff_texts, default_symbol)?;
    for message in diffs {
        let event = parse_envelope(message).map_err(|err| {
            PolicyDemoError::DepthInput(format!("public depth diff envelope: {err}"))
        })?;
        let StreamEvent::Depth(diff) = event else {
            continue;
        };
        if book.apply_diff(diff) {
            messages.push(envelope_message(&StreamEvent::Depth(book.to_depth_update())));
        }
    }
    Ok(messages)
}

#[derive(Debug, Clone)]
struct PublicDepthBook {
    symbol: String,
    last_update_id: Option<u64>,
    bids: BTreeMap<(i64, String), String>,
    asks: BTreeMap<(i64, String), String>,
}

impl PublicDepthBook {
    fn from_snapshot(snapshot: DepthUpdate) -> Self {
        let mut book = Self {
            symbol: snapshot.symbol,
            last_update_id: snapshot.update_id,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
        };
        book.replace_side(true, snapshot.bids);
        book.replace_side(false, snapshot.asks);
        book
    }

    fn apply_diff(&mut self, diff: DepthUpdate) -> bool {
        if let Some(update_id) = diff.update_id {
            if let Some(last) = self.last_update_id {
                if update_id <= last {
                    return false;
                }
            }
            self.last_update_id = Some(update_id);
        }
        if !diff.symbol.is_empty() {
            self.symbol = diff.symbol;
        }
        self.apply_side(true, diff.bids);
        self.apply_side(false, diff.asks);
        true
    }

    fn replace_side(&mut self, bids: bool, levels: Vec<PriceLevel>) {
        let side = if bids {
            &mut self.bids
        } else {
            &mut self.asks
        };
        side.clear();
        for level in levels {
            upsert_level(side, level);
        }
    }

    fn apply_side(&mut self, bids: bool, levels: Vec<PriceLevel>) {
        let side = if bids {
            &mut self.bids
        } else {
            &mut self.asks
        };
        for level in levels {
            upsert_level(side, level);
        }
    }

    fn to_depth_update(&self) -> DepthUpdate {
        DepthUpdate {
            symbol: self.symbol.clone(),
            bids: self
                .bids
                .iter()
                .rev()
                .map(|((_, price), qty)| PriceLevel {
                    price: price.clone(),
                    qty: qty.clone(),
                })
                .collect(),
            asks: self
                .asks
                .iter()
                .map(|((_, price), qty)| PriceLevel {
                    price: price.clone(),
                    qty: qty.clone(),
                })
                .collect(),
            update_id: self.last_update_id,
        }
    }
}

fn upsert_level(side: &mut BTreeMap<(i64, String), String>, level: PriceLevel) {
    if level.price.is_empty() {
        return;
    }
    let key = price_sort_key(&level.price);
    if is_removed_qty(&level.qty) {
        side.remove(&key);
        return;
    }
    side.insert(key, level.qty);
}

fn price_sort_key(price: &str) -> (i64, String) {
    let value = price.parse::<f64>().unwrap_or(0.0);
    let scaled = (value * 100_000_000.0).round() as i64;
    (scaled, price.to_string())
}

fn is_removed_qty(qty: &str) -> bool {
    qty.trim().is_empty() || qty.parse::<f64>().map(|value| value == 0.0).unwrap_or(false)
}

/// Parse raw public-depth WebSocket texts. Subscribe acks and empty frames are skipped.
pub fn policy_demo_public_depth_messages_from_texts<I, S>(
    frames: I,
    default_symbol: &str,
) -> Result<Vec<Message>, PolicyDemoError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut messages = Vec::new();
    for frame in frames {
        let text = frame.as_ref().trim();
        if text.is_empty() || is_public_depth_control_frame(text) {
            continue;
        }
        match policy_demo_depth_messages_from_json(text, default_symbol) {
            Ok(parsed) => messages.extend(parsed),
            Err(_) => continue,
        }
    }
    Ok(messages)
}

fn is_public_depth_control_frame(text: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    value.get("result").is_some()
        && value.get("id").is_some()
        && value.get("e").is_none()
        && value.get("data").is_none()
        && value.get("b").is_none()
        && value.get("bids").is_none()
        && value.get("kind").is_none()
}

/// Collect public depth through a mockable text-frame source (offline tests).
pub async fn collect_subscribed_public_depth<S, Fut>(
    symbol: &str,
    max_frames: usize,
    timeout_duration: Duration,
    source: S,
) -> Result<Vec<Message>, PolicyDemoError>
where
    S: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<String>, PolicyDemoError>>,
{
    if timeout_duration.is_zero() {
        return Err(PolicyDemoError::DepthInput(
            "--public-depth-timeout-secs must be greater than zero".into(),
        ));
    }
    let texts = source().await?;
    let messages = policy_demo_public_depth_messages_from_texts(texts, symbol)?;
    if messages.is_empty() {
        return Err(PolicyDemoError::DepthInput(
            "public depth source returned no frames".into(),
        ));
    }
    Ok(truncate_depth_messages(messages, max_frames))
}

/// Collect public depth texts and rebuild a local book from a REST-style snapshot.
pub async fn collect_subscribed_public_depth_book<S, Fut>(
    symbol: &str,
    snapshot_json: &str,
    max_frames: usize,
    timeout_duration: Duration,
    source: S,
) -> Result<Vec<Message>, PolicyDemoError>
where
    S: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<String>, PolicyDemoError>>,
{
    if timeout_duration.is_zero() {
        return Err(PolicyDemoError::DepthInput(
            "--public-depth-timeout-secs must be greater than zero".into(),
        ));
    }
    let texts = source().await?;
    let messages = policy_demo_messages_from_public_depth_book(snapshot_json, texts, symbol)?;
    if messages.is_empty() {
        return Err(PolicyDemoError::DepthInput(
            "public depth book produced no frames".into(),
        ));
    }
    Ok(truncate_depth_messages(messages, max_frames))
}

/// Live Binance public-depth collector used by `--subscribe-public-depth`.
pub async fn subscribe_binance_public_depth(
    venue: DemoVenue,
    symbol: impl AsRef<str>,
    max_frames: usize,
    timeout_duration: Duration,
) -> Result<Vec<Message>, PolicyDemoError> {
    if timeout_duration.is_zero() {
        return Err(PolicyDemoError::DepthInput(
            "--public-depth-timeout-secs must be greater than zero".into(),
        ));
    }
    let symbol = symbol.as_ref();
    let snapshot = fetch_public_depth_snapshot(venue, symbol).await?;
    let mut socket = connect_public_depth_websocket(&public_depth_stream_url(venue)).await?;
    socket
        .send(Message::Text(
            public_depth_subscribe_request_json(symbol).into(),
        ))
        .await
        .map_err(|err| {
            PolicyDemoError::DepthInput(format!("public depth subscribe send failed: {err}"))
        })?;

    let deadline = Instant::now() + timeout_duration;
    let want = max_frames.max(1);
    let mut texts = Vec::new();
    while texts.len() + 1 < want {
        let Some(frame) = next_public_depth_message(&mut socket, deadline).await? else {
            break;
        };
        let Message::Text(text) = frame else {
            continue;
        };
        if is_public_depth_control_frame(text.as_str()) {
            continue;
        }
        texts.push(text.to_string());
    }
    let messages = policy_demo_messages_from_public_depth_book(&snapshot, texts, symbol)?;
    if messages.is_empty() {
        return Err(PolicyDemoError::DepthInput(
            "timed out waiting for public depth frames".into(),
        ));
    }
    Ok(truncate_depth_messages(messages, max_frames))
}

async fn fetch_public_depth_snapshot(
    venue: DemoVenue,
    symbol: &str,
) -> Result<String, PolicyDemoError> {
    let url = public_depth_snapshot_url(venue, symbol);
    let response = reqwest::Client::new().get(&url).send().await.map_err(|err| {
        PolicyDemoError::DepthInput(format!("public depth snapshot request failed: {err}"))
    })?;
    if !response.status().is_success() {
        return Err(PolicyDemoError::DepthInput(format!(
            "public depth snapshot HTTP {}",
            response.status()
        )));
    }
    response.text().await.map_err(|err| {
        PolicyDemoError::DepthInput(format!("public depth snapshot body failed: {err}"))
    })
}

/// Run the harness from mock public-depth WebSocket texts (subscribe acks skipped).
pub async fn run_policy_demo_with_subscribed_public_depth_texts<P, S, Fut>(
    mut config: PolicyDemoConfig,
    policy: &P,
    policy_source: impl Into<String>,
    source: S,
) -> Result<PolicyDemoReport, PolicyDemoError>
where
    P: PolicyProvider + ?Sized,
    S: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<String>, PolicyDemoError>>,
{
    config.subscribe_public_depth = true;
    ensure_public_depth_subscribe_config(&config)?;
    let symbol = config.symbol.clone();
    let max_steps = config.max_steps;
    let timeout = config.public_depth_timeout;
    let snapshot = config.public_depth_snapshot_json.clone();
    run_policy_demo_with_public_depth(config, policy, policy_source, move || async move {
        if let Some(snapshot) = snapshot {
            collect_subscribed_public_depth_book(&symbol, &snapshot, max_steps, timeout, source)
                .await
        } else {
            collect_subscribed_public_depth(&symbol, max_steps, timeout, source).await
        }
    })
    .await
}

pub fn synthetic_depth_stream(symbol: &str, max_steps: usize) -> Vec<Message> {
    (0..max_steps)
        .map(|idx| {
            let bid = 100.0 + idx as f64 * 0.25;
            let ask = bid + 1.0;
            envelope_message(&StreamEvent::Depth(DepthUpdate {
                symbol: symbol.into(),
                bids: vec![PriceLevel {
                    price: format!("{bid:.2}"),
                    qty: "1.0".into(),
                }],
                asks: vec![PriceLevel {
                    price: format!("{ask:.2}"),
                    qty: "1.0".into(),
                }],
                update_id: Some(idx as u64 + 1),
            }))
        })
        .collect()
}

/// Parse captured depth JSON/NDJSON the same way `--reconcile-user-data-json` does:
/// one object, a JSON array, or newline-delimited frames.
///
/// Each frame may already be a normalized [`StreamEvent`] envelope, or a captured
/// Binance combined-stream / raw `depthUpdate` / REST snapshot book. Missing
/// symbols fall back to `default_symbol` so REST snapshots can feed `Env`.
pub fn policy_demo_depth_messages_from_json(
    input: &str,
    default_symbol: &str,
) -> Result<Vec<Message>, PolicyDemoError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(PolicyDemoError::DepthInput(
            "captured depth JSON is empty".into(),
        ));
    }

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return match value {
            Value::Array(values) => values
                .into_iter()
                .map(|value| json_value_to_depth_message(value, default_symbol))
                .collect::<Result<Vec<_>, _>>(),
            other => Ok(vec![json_value_to_depth_message(other, default_symbol)?]),
        };
    }

    input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let value = serde_json::from_str(line).map_err(|err| {
                PolicyDemoError::DepthInput(format!("invalid captured depth NDJSON: {err}"))
            })?;
            json_value_to_depth_message(value, default_symbol)
        })
        .collect()
}

fn json_value_to_depth_message(
    value: Value,
    default_symbol: &str,
) -> Result<Message, PolicyDemoError> {
    match value {
        Value::String(text) => {
            let inner = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
            if let Value::String(_) = &inner {
                return Err(PolicyDemoError::DepthInput(
                    "captured depth string frame is not JSON".into(),
                ));
            }
            json_value_to_depth_message(inner, default_symbol)
        }
        other => {
            let event = stream_event_from_captured_depth(&other, default_symbol)?;
            Ok(envelope_message(&event))
        }
    }
}

fn stream_event_from_captured_depth(
    value: &Value,
    default_symbol: &str,
) -> Result<StreamEvent, PolicyDemoError> {
    if value.get("kind").is_some() {
        return serde_json::from_value(value.clone()).map_err(|err| {
            PolicyDemoError::DepthInput(format!("invalid normalized depth envelope: {err}"))
        });
    }

    let data = value.get("data").unwrap_or(value);
    let symbol = data
        .get("s")
        .or_else(|| data.get("symbol"))
        .and_then(json_as_string)
        .filter(|symbol| !symbol.is_empty())
        .unwrap_or_else(|| default_symbol.to_owned());
    let bids = parse_captured_levels(
        data.get("b")
            .or_else(|| data.get("bids"))
            .or_else(|| value.get("bids")),
    );
    let asks = parse_captured_levels(
        data.get("a")
            .or_else(|| data.get("asks"))
            .or_else(|| value.get("asks")),
    );
    if bids.is_empty() && asks.is_empty() {
        return Err(PolicyDemoError::DepthInput(
            "captured depth frame has no bids or asks".into(),
        ));
    }
    let update_id = json_as_u64(
        data.get("u")
            .or_else(|| data.get("lastUpdateId"))
            .or_else(|| data.get("update_id"))
            .or_else(|| value.get("lastUpdateId")),
    );

    Ok(StreamEvent::Depth(DepthUpdate {
        symbol,
        bids,
        asks,
        update_id,
    }))
}

fn parse_captured_levels(value: Option<&Value>) -> Vec<PriceLevel> {
    let Some(Value::Array(levels)) = value else {
        return Vec::new();
    };
    levels
        .iter()
        .filter_map(|level| match level {
            Value::Array(pair) if pair.len() >= 2 => Some(PriceLevel {
                price: json_as_string(&pair[0]).unwrap_or_default(),
                qty: json_as_string(&pair[1]).unwrap_or_default(),
            }),
            Value::Object(map) => Some(PriceLevel {
                price: map
                    .get("price")
                    .and_then(json_as_string)
                    .unwrap_or_default(),
                qty: map.get("qty").and_then(json_as_string).unwrap_or_default(),
            }),
            _ => None,
        })
        .filter(|level| !level.price.is_empty())
        .collect()
}

fn json_as_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn json_as_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    })
}

fn ensure_demo_order_guard(var: &str) -> Result<(), PolicyDemoError> {
    match env::var(var) {
        Ok(value) if value == "1" => Ok(()),
        _ => Err(PolicyDemoError::MissingDemoOrderGuard { var: var.into() }),
    }
}

fn ensure_public_depth_subscribe_config(config: &PolicyDemoConfig) -> Result<(), PolicyDemoError> {
    if !config.subscribe_public_depth {
        return Ok(());
    }
    if config.public_depth_timeout.is_zero() {
        return Err(PolicyDemoError::DepthInput(
            "--public-depth-timeout-secs must be greater than zero".into(),
        ));
    }
    Ok(())
}

fn ensure_live_reconciliation_config(config: &PolicyDemoConfig) -> Result<(), PolicyDemoError> {
    if !config.wait_for_user_data {
        return Ok(());
    }
    if !config.execute_demo_orders {
        return Err(PolicyDemoError::LiveReconciliation(
            "--wait-for-user-data requires --execute-demo-orders".into(),
        ));
    }
    if config.user_data_timeout.is_zero() {
        return Err(PolicyDemoError::LiveReconciliation(
            "--user-data-timeout-secs must be greater than zero".into(),
        ));
    }
    Ok(())
}

fn drain_spot_orders(
    rx: &mut mpsc::UnboundedReceiver<SpotPlaceOrderRequest>,
) -> Vec<SpotPlaceOrderRequest> {
    let mut orders = Vec::new();
    while let Ok(order) = rx.try_recv() {
        orders.push(order);
    }
    orders
}

fn drain_usdm_orders(
    rx: &mut mpsc::UnboundedReceiver<UsdmPlaceOrderRequest>,
) -> Vec<UsdmPlaceOrderRequest> {
    let mut orders = Vec::new();
    while let Ok(order) = rx.try_recv() {
        orders.push(order);
    }
    orders
}

async fn place_spot_demo_orders(
    credentials: DemoCredentials,
    orders: &[SpotPlaceOrderRequest],
) -> Result<Vec<PolicyDemoReceipt>, PolicyDemoError> {
    let client = SpotOrderClient::new(
        SpotCredentials {
            api_key: credentials.api_key,
            secret_key: credentials.secret_key,
        },
        SpotNativeTlsTransport::new(),
    )
    .with_base_url(SPOT_DEMO_ORDER_BASE_URL);

    let mut receipts = Vec::new();
    for order in orders.iter().cloned() {
        let response = client
            .place_order(order)
            .await
            .map_err(PolicyDemoError::SpotPlaceOrder)?;
        receipts.push(spot_receipt_from_response(response));
    }
    Ok(receipts)
}

async fn place_usdm_demo_orders(
    credentials: DemoCredentials,
    orders: &[UsdmPlaceOrderRequest],
) -> Result<Vec<PolicyDemoReceipt>, PolicyDemoError> {
    let client = UsdmOrderClient::new(
        UsdmCredentials {
            api_key: credentials.api_key,
            secret_key: credentials.secret_key,
        },
        UsdmNativeTlsTransport::new(),
    )
    .with_base_url(USDM_DEMO_REST_BASE_URL);

    let mut receipts = Vec::new();
    for order in orders.iter().cloned() {
        let response = client
            .place_order(order)
            .await
            .map_err(PolicyDemoError::UsdmPlaceOrder)?;
        receipts.push(usdm_receipt_from_response(response));
    }
    Ok(receipts)
}

async fn prepare_spot_live_user_data(
    credentials: &DemoCredentials,
    timeout_duration: Duration,
) -> Result<DemoWebSocket, PolicyDemoError> {
    let stream = BinanceSpotUserStream::demo(SpotCredentials {
        api_key: credentials.api_key.clone(),
        secret_key: credentials.secret_key.clone(),
    });
    let mut socket = connect_demo_websocket(&stream.websocket_url()).await?;
    socket
        .send(Message::Text(stream.subscribe_request_json().into()))
        .await
        .map_err(|err| {
            PolicyDemoError::LiveReconciliation(format!(
                "spot demo user-data subscribe send failed: {err}"
            ))
        })?;
    wait_for_spot_subscribe_ack(&mut socket, timeout_duration).await?;
    Ok(socket)
}

async fn wait_for_spot_subscribe_ack(
    socket: &mut DemoWebSocket,
    timeout_duration: Duration,
) -> Result<(), PolicyDemoError> {
    timeout(timeout_duration, async {
        while let Some(frame) = socket.next().await {
            let frame = frame.map_err(|err| {
                PolicyDemoError::LiveReconciliation(format!(
                    "spot demo user-data subscribe ack read failed: {err}"
                ))
            })?;
            let Message::Text(text) = frame else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            if value.get("result").is_some() && value.get("id").is_some() {
                return Ok(());
            }
        }
        Err(PolicyDemoError::LiveReconciliation(
            "spot demo user-data stream closed before subscribe ack".into(),
        ))
    })
    .await
    .map_err(|_| {
        PolicyDemoError::LiveReconciliation(
            "timed out waiting for spot demo user-data subscribe ack".into(),
        )
    })?
}

struct UsdmLiveUserData {
    socket: DemoWebSocket,
    listen_client: ListenKeyClient,
}

async fn prepare_usdm_live_user_data(
    credentials: &DemoCredentials,
) -> Result<UsdmLiveUserData, PolicyDemoError> {
    let credentials = UsdmCredentials {
        api_key: credentials.api_key.clone(),
        secret_key: credentials.secret_key.clone(),
    };
    let listen_client = ListenKeyClient::demo(credentials);
    let listen_key = listen_client
        .create()
        .await
        .map_err(usdm_listen_key_error)?;
    let stream = UsdmUserDataStream::demo(&listen_key)
        .with_events_filter("ORDER_TRADE_UPDATE/ACCOUNT_UPDATE");
    let socket = connect_demo_websocket(&stream.websocket_url()).await?;
    listen_client
        .keepalive()
        .await
        .map_err(usdm_listen_key_error)?;
    Ok(UsdmLiveUserData {
        socket,
        listen_client,
    })
}

async fn connect_demo_websocket(url: &str) -> Result<DemoWebSocket, PolicyDemoError> {
    let uri: Uri = url.parse().map_err(|err| {
        PolicyDemoError::LiveReconciliation(format!("invalid demo user-data websocket URL: {err}"))
    })?;
    trolly_stream::connect(uri).await.map_err(|err| {
        PolicyDemoError::LiveReconciliation(format!(
            "demo user-data websocket connect failed: {err}"
        ))
    })
}

async fn connect_public_depth_websocket(url: &str) -> Result<DemoWebSocket, PolicyDemoError> {
    let uri: Uri = url.parse().map_err(|err| {
        PolicyDemoError::DepthInput(format!("invalid public depth websocket URL: {err}"))
    })?;
    trolly_stream::connect(uri).await.map_err(|err| {
        PolicyDemoError::DepthInput(format!("public depth websocket connect failed: {err}"))
    })
}

async fn next_public_depth_message(
    socket: &mut DemoWebSocket,
    deadline: Instant,
) -> Result<Option<Message>, PolicyDemoError> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        let remaining = deadline.saturating_duration_since(now);
        let frame = match timeout(remaining, socket.next()).await {
            Ok(Some(Ok(frame))) => frame,
            Ok(Some(Err(err))) => {
                return Err(PolicyDemoError::DepthInput(format!(
                    "public depth stream read failed: {err}"
                )));
            }
            Ok(None) | Err(_) => return Ok(None),
        };
        match frame {
            Message::Text(_) | Message::Binary(_) => return Ok(Some(frame)),
            _ => continue,
        }
    }
}

fn usdm_listen_key_error(err: ListenKeyError) -> PolicyDemoError {
    PolicyDemoError::LiveReconciliation(format!("USDM demo listenKey lifecycle failed: {err}"))
}

async fn wait_spot_live_reconciliations(
    socket: &mut DemoWebSocket,
    report: &PolicyDemoReport,
    timeout_duration: Duration,
) -> Result<Vec<PolicyDemoReconciliation>, PolicyDemoError> {
    let mut state = ReconciliationState::from_report(report);
    if state.is_empty() {
        return Ok(Vec::new());
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    let account = Arc::new(Mutex::new(AccountBook::default()));
    let ctx = SpotExecContext {
        events: tx,
        account,
    };
    let mut hub = build_spot_multiplexor(&[report.symbol.as_str()], ctx);
    let deadline = Instant::now() + timeout_duration;

    while !state.is_terminal_complete() {
        let Some(message) = next_live_user_data_message(socket, deadline, "spot").await? else {
            break;
        };
        ingest_spot_user_data(&mut hub, message);
        drain_spot_reconciliation_events(&mut rx, &mut state);
    }

    Ok(state.into_reconciliations())
}

async fn wait_usdm_live_reconciliations(
    socket: &mut DemoWebSocket,
    report: &PolicyDemoReport,
    timeout_duration: Duration,
) -> Result<Vec<PolicyDemoReconciliation>, PolicyDemoError> {
    let mut state = ReconciliationState::from_report(report);
    if state.is_empty() {
        return Ok(Vec::new());
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    let ctx = UsdmExecContext::new(Some(tx));
    let mut hub = build_usdm_multiplexor_with_context(&[report.symbol.as_str()], ctx);
    let deadline = Instant::now() + timeout_duration;

    while !state.is_terminal_complete() {
        let Some(message) = next_live_user_data_message(socket, deadline, "USDM").await? else {
            break;
        };
        ingest_usdm_user_data(&mut hub, message);
        drain_usdm_reconciliation_events(&mut rx, &mut state);
    }

    Ok(state.into_reconciliations())
}

async fn next_live_user_data_message(
    socket: &mut DemoWebSocket,
    deadline: Instant,
    venue: &str,
) -> Result<Option<Message>, PolicyDemoError> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        let remaining = deadline.saturating_duration_since(now);
        let frame = match timeout(remaining, socket.next()).await {
            Ok(Some(Ok(frame))) => frame,
            Ok(Some(Err(err))) => {
                return Err(PolicyDemoError::LiveReconciliation(format!(
                    "{venue} demo user-data stream read failed: {err}"
                )));
            }
            Ok(None) | Err(_) => return Ok(None),
        };
        match frame {
            Message::Text(_) | Message::Binary(_) => return Ok(Some(frame)),
            _ => continue,
        }
    }
}

fn assign_spot_client_order_ids(orders: &mut [SpotPlaceOrderRequest], prefix: &str) {
    for (idx, order) in orders.iter_mut().enumerate() {
        if order.new_client_order_id.is_none() {
            order.new_client_order_id = Some(demo_client_order_id(prefix, DemoVenue::Spot, idx));
        }
    }
}

fn assign_usdm_client_order_ids(orders: &mut [UsdmPlaceOrderRequest], prefix: &str) {
    for (idx, order) in orders.iter_mut().enumerate() {
        if order.new_client_order_id.is_none() {
            order.new_client_order_id = Some(demo_client_order_id(prefix, DemoVenue::Usdm, idx));
        }
    }
}

fn demo_client_order_id(prefix: &str, venue: DemoVenue, idx: usize) -> String {
    let venue = match venue {
        DemoVenue::Spot => "spot",
        DemoVenue::Usdm => "usdm",
    };
    let suffix = format!("{venue}-{idx:04}");
    let mut clean_prefix: String = prefix
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect();
    if clean_prefix.is_empty() {
        clean_prefix.push_str("trolly-demo");
    }

    let max_prefix_len = 36usize.saturating_sub(suffix.len() + 1);
    clean_prefix.truncate(max_prefix_len);
    format!("{clean_prefix}-{suffix}")
}

fn spot_receipt_from_response(response: SpotPlaceOrderResponse) -> PolicyDemoReceipt {
    PolicyDemoReceipt {
        venue: DemoVenue::Spot,
        symbol: response.symbol,
        order_id: response.order_id,
        client_order_id: response.client_order_id,
        status: response.status,
        side: response.side,
    }
}

fn usdm_receipt_from_response(response: UsdmPlaceOrderResponse) -> PolicyDemoReceipt {
    PolicyDemoReceipt {
        venue: DemoVenue::Usdm,
        symbol: response.symbol,
        order_id: response.order_id,
        client_order_id: response.client_order_id,
        status: response.status,
        side: response.side,
    }
}

fn load_policy(config: &PolicyDemoConfig) -> (CheckpointOrHoldPolicy, String) {
    let window_frames = config.window_frames;
    #[cfg(any(feature = "gym-ort", feature = "gym-torch"))]
    let obs_dim = trolly_gym::sim::microstructure_obs_dim(window_frames);
    #[cfg(not(any(feature = "gym-ort", feature = "gym-torch")))]
    let _ = window_frames;

    let hold_deadzone = inventory_hold_deadzone(config);

    #[cfg(feature = "gym-ort")]
    if let Some(path) = env::var_os("ONNX_MODEL_PATH") {
        return match CheckpointOrHoldPolicy::from_onnx_model(&path, obs_dim) {
            Ok(policy) => (policy, format!("onnx:{}", path.to_string_lossy())),
            Err(err) => (
                CheckpointOrHoldPolicy::hold(),
                format!("hold (ONNX load failed: {err})"),
            ),
        };
    }

    #[cfg(not(feature = "gym-ort"))]
    if env::var_os("ONNX_MODEL_PATH").is_some() {
        return (
            CheckpointOrHoldPolicy::hold(),
            "hold (ONNX_MODEL_PATH ignored; build with --features gym-ort)".into(),
        );
    }

    if let Some(path) = onnx_gaussian_model_path(config) {
        return load_onnx_gaussian_policy(&path, hold_deadzone);
    }

    if let Some(spec) = gaussian_mean_actions_spec(config) {
        return match CheckpointOrHoldPolicy::from_mean_actions_csv(&spec, hold_deadzone) {
            Ok(policy) => (policy, format!("gaussian-mean-actions:{spec}")),
            Err(err) => (
                CheckpointOrHoldPolicy::hold(),
                format!("hold (gaussian mean-actions parse failed: {err})"),
            ),
        };
    }

    if let Some(dir) = gaussian_checkpoint_dir(config) {
        if checkpoint_dir_is_retired(&dir) {
            return (
                CheckpointOrHoldPolicy::hold(),
                format!(
                    "hold (refusing retired unit-lot checkpoint {})",
                    dir.to_string_lossy()
                ),
            );
        }
        if let Some(mu) = exported_mu_onnx_path(&dir) {
            return load_onnx_gaussian_policy(&mu, hold_deadzone);
        }
        return load_gaussian_checkpoint_policy(&dir, hold_deadzone);
    }

    if let Some(mu) = weekday_exported_mu_onnx() {
        return load_onnx_gaussian_policy(&mu, hold_deadzone);
    }

    #[cfg(feature = "gym-torch")]
    {
        if let Some(dir) = env::var_os("CHECKPOINT_DIR") {
            if checkpoint_dir_is_retired(&dir) {
                return (
                    CheckpointOrHoldPolicy::hold(),
                    format!(
                        "hold (refusing retired unit-lot checkpoint {})",
                        dir.to_string_lossy()
                    ),
                );
            }
            if checkpoint_dir_looks_gaussian(&dir) {
                if let Some(mu) = exported_mu_onnx_path(&dir) {
                    return load_onnx_gaussian_policy(&mu, hold_deadzone);
                }
                return load_gaussian_checkpoint_policy(&dir, hold_deadzone);
            }
            return match CheckpointOrHoldPolicy::from_latest_checkpoint_dir(
                &dir,
                obs_dim,
                Default::default(),
            ) {
                Ok(policy) => (policy, format!("torch:{}", dir.to_string_lossy())),
                Err(err) => (
                    CheckpointOrHoldPolicy::hold(),
                    format!("hold (checkpoint load failed: {err})"),
                ),
            };
        }
    }

    #[cfg(not(feature = "gym-torch"))]
    if let Some(dir) = env::var_os("CHECKPOINT_DIR") {
        if checkpoint_dir_looks_gaussian(&dir) {
            if let Some(mu) = exported_mu_onnx_path(&dir) {
                return load_onnx_gaussian_policy(&mu, hold_deadzone);
            }
            return (
                CheckpointOrHoldPolicy::hold(),
                format!(
                    "hold (gaussian checkpoint {} ignored; build with --features gym-torch)",
                    dir.to_string_lossy()
                ),
            );
        }
        return (
            CheckpointOrHoldPolicy::hold(),
            "hold (CHECKPOINT_DIR ignored; build with --features gym-torch)".into(),
        );
    }

    (CheckpointOrHoldPolicy::hold(), "hold".into())
}

fn inventory_hold_deadzone(config: &PolicyDemoConfig) -> f32 {
    env::var("GAUSSIAN_HOLD_DEADZONE")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(config.inventory_hold_deadzone)
}

fn uses_gaussian_source(config: &PolicyDemoConfig) -> bool {
    gaussian_mean_actions_spec(config).is_some()
        || gaussian_checkpoint_dir(config).is_some()
        || onnx_gaussian_model_path(config).is_some()
        || weekday_exported_mu_onnx().is_some()
}

/// Weekday GPU job dir. Used only when `mu.onnx` is already on disk and no
/// other Gaussian source is selected.
pub const WEEKDAY_GAUSSIAN_MLP_DIR: &str =
    "checkpoints/gpu_train_orchestrator/microstructure/gaussian_mlp";

fn weekday_exported_mu_onnx() -> Option<std::ffi::OsString> {
    exported_mu_onnx_path(std::ffi::OsStr::new(WEEKDAY_GAUSSIAN_MLP_DIR))
}

fn exported_mu_onnx_path(dir: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
    let path = Path::new(dir).join("mu.onnx");
    path.is_file().then(|| path.into_os_string())
}

fn onnx_gaussian_model_path(config: &PolicyDemoConfig) -> Option<std::ffi::OsString> {
    if let Some(path) = &config.onnx_gaussian_model_path {
        if !path.trim().is_empty() {
            return Some(std::ffi::OsString::from(path));
        }
    }
    env::var_os("ONNX_GAUSSIAN_MODEL_PATH")
}

fn load_onnx_gaussian_policy(
    path: &std::ffi::OsStr,
    hold_deadzone: f32,
) -> (CheckpointOrHoldPolicy, String) {
    #[cfg(feature = "gym-ort")]
    {
        let obs_dim = trolly_gym::sim::ladder_obs_dim(8);
        return match CheckpointOrHoldPolicy::from_onnx_gaussian_model(path, obs_dim, hold_deadzone)
        {
            Ok(policy) => (policy, format!("onnx-gaussian:{}", path.to_string_lossy())),
            Err(err) => (
                CheckpointOrHoldPolicy::hold(),
                format!("hold (ONNX Gaussian load failed: {err})"),
            ),
        };
    }

    #[cfg(not(feature = "gym-ort"))]
    {
        let _ = hold_deadzone;
        (
            CheckpointOrHoldPolicy::hold(),
            format!(
                "hold (ONNX_GAUSSIAN_MODEL_PATH {} ignored; build with --features gym-ort)",
                path.to_string_lossy()
            ),
        )
    }
}

fn gaussian_mean_actions_spec(config: &PolicyDemoConfig) -> Option<String> {
    config
        .gaussian_mean_actions
        .clone()
        .or_else(|| env::var("GAUSSIAN_MEAN_ACTIONS").ok())
        .filter(|spec| !spec.trim().is_empty())
}

fn gaussian_checkpoint_dir(config: &PolicyDemoConfig) -> Option<std::ffi::OsString> {
    if let Some(dir) = &config.gaussian_checkpoint_dir {
        if !dir.trim().is_empty() {
            return Some(std::ffi::OsString::from(dir));
        }
    }
    env::var_os("GAUSSIAN_CHECKPOINT_DIR")
}

fn checkpoint_dir_looks_gaussian(dir: &std::ffi::OsStr) -> bool {
    let text = dir.to_string_lossy();
    text.contains("gaussian_mlp") || text.contains("gaussian_liquid")
}

fn checkpoint_dir_is_retired(dir: &std::ffi::OsStr) -> bool {
    dir.to_string_lossy()
        .contains("_retired_unit_lot_microstructure")
}

fn load_gaussian_checkpoint_policy(
    dir: &std::ffi::OsStr,
    hold_deadzone: f32,
) -> (CheckpointOrHoldPolicy, String) {
    if checkpoint_dir_is_retired(dir) {
        return (
            CheckpointOrHoldPolicy::hold(),
            format!(
                "hold (refusing retired unit-lot checkpoint {})",
                dir.to_string_lossy()
            ),
        );
    }

    #[cfg(feature = "gym-torch")]
    {
        return match CheckpointOrHoldPolicy::from_latest_gaussian_checkpoint_dir(dir, hold_deadzone)
        {
            Ok(policy) => (policy, format!("gaussian-torch:{}", dir.to_string_lossy())),
            Err(err) => (
                CheckpointOrHoldPolicy::hold(),
                format!("hold (gaussian checkpoint load failed: {err})"),
            ),
        };
    }

    #[cfg(not(feature = "gym-torch"))]
    {
        let _ = hold_deadzone;
        (
            CheckpointOrHoldPolicy::hold(),
            format!(
                "hold (gaussian checkpoint {} ignored; build with --features gym-torch)",
                dir.to_string_lossy()
            ),
        )
    }
}
