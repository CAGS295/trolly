//! Guarded demo bridge from policy harness output to execution adapters.

use std::{
    collections::HashSet,
    env, fmt,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

use binance_spot_exec::{
    build_multiplexor as build_spot_multiplexor, ingest_user_data as ingest_spot_user_data,
    AccountBook, ApiCredentials as SpotCredentials, BinanceSpotUserStream,
    NativeTlsTransport as SpotNativeTlsTransport, PlaceOrderError as SpotPlaceOrderError,
    PlaceOrderRequest as SpotPlaceOrderRequest, PlaceOrderResponse as SpotPlaceOrderResponse,
    SpotExecContext, SpotOrderClient, SpotOrderEgress, SpotUserEvent, SPOT_DEMO_ORDER_BASE_URL,
};
use binance_usdm_exec::{
    build_multiplexor_with_context as build_usdm_multiplexor_with_context,
    ingest_user_data as ingest_usdm_user_data, ApiCredentials as UsdmCredentials, ListenKeyClient,
    ListenKeyError, NativeTlsTransport as UsdmNativeTlsTransport,
    PlaceOrderError as UsdmPlaceOrderError, PlaceOrderRequest as UsdmPlaceOrderRequest,
    PlaceOrderResponse as UsdmPlaceOrderResponse, UsdmExecContext, UsdmExecUpdate, UsdmOrderClient,
    UsdmOrderEgress, UsdmUserDataStream, USDM_DEMO_REST_BASE_URL,
};
use futures_util::{SinkExt, StreamExt};
use http::Uri;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::{timeout, Instant};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use trolly_gym::{
    run_offline_policy_harness, CheckpointOrHoldPolicy, Env, EnvConfig, PolicyProvider,
};
use trolly_strategy::{envelope_message, DepthUpdate, OrderOnlyEgress, PriceLevel, StreamEvent};
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
        }
    }
}

#[derive(Debug, Clone)]
pub struct PolicyDemoReport {
    pub venue: DemoVenue,
    pub symbol: String,
    pub policy_source: String,
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
    let (policy, policy_source) = load_policy(config.window_frames);
    run_policy_demo_with_policy(config, &policy, policy_source).await
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

    let mut report = PolicyDemoReport {
        venue: DemoVenue::Spot,
        symbol: config.symbol,
        policy_source,
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

    let mut report = PolicyDemoReport {
        venue: DemoVenue::Usdm,
        symbol: config.symbol,
        policy_source,
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

    Ok(PolicyDemoReport {
        venue: DemoVenue::Spot,
        symbol: config.symbol,
        policy_source: policy_source.into(),
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

    Ok(PolicyDemoReport {
        venue: DemoVenue::Usdm,
        symbol: config.symbol,
        policy_source: policy_source.into(),
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

    let mut env = Env::new(env_config, egress);
    let messages = synthetic_depth_stream(&config.symbol, config.max_steps);
    let steps = run_offline_policy_harness(&mut env, policy, messages)
        .map_err(|err| PolicyDemoError::Harness(err.to_string()))?;
    Ok(steps.len())
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

fn ensure_demo_order_guard(var: &str) -> Result<(), PolicyDemoError> {
    match env::var(var) {
        Ok(value) if value == "1" => Ok(()),
        _ => Err(PolicyDemoError::MissingDemoOrderGuard { var: var.into() }),
    }
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

fn load_policy(window_frames: usize) -> (CheckpointOrHoldPolicy, String) {
    #[cfg(any(feature = "gym-ort", feature = "gym-torch"))]
    let obs_dim = trolly_gym::sim::microstructure_obs_dim(window_frames);
    #[cfg(not(any(feature = "gym-ort", feature = "gym-torch")))]
    let _ = window_frames;

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

    #[cfg(feature = "gym-torch")]
    {
        if let Some(dir) = env::var_os("CHECKPOINT_DIR") {
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
    if env::var_os("CHECKPOINT_DIR").is_some() {
        return (
            CheckpointOrHoldPolicy::hold(),
            "hold (CHECKPOINT_DIR ignored; build with --features gym-torch)".into(),
        );
    }

    (CheckpointOrHoldPolicy::hold(), "hold".into())
}
