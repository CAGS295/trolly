//! Guarded demo bridge from policy harness output to execution adapters.

use std::{env, fmt, future::Future};

use binance_spot_exec::{
    ApiCredentials as SpotCredentials, NativeTlsTransport as SpotNativeTlsTransport,
    PlaceOrderError as SpotPlaceOrderError, PlaceOrderRequest as SpotPlaceOrderRequest,
    PlaceOrderResponse as SpotPlaceOrderResponse, SpotOrderClient, SpotOrderEgress,
    SPOT_DEMO_ORDER_BASE_URL,
};
use binance_usdm_exec::{
    ApiCredentials as UsdmCredentials, NativeTlsTransport as UsdmNativeTlsTransport,
    PlaceOrderError as UsdmPlaceOrderError, PlaceOrderRequest as UsdmPlaceOrderRequest,
    PlaceOrderResponse as UsdmPlaceOrderResponse, UsdmOrderClient, UsdmOrderEgress,
    USDM_DEMO_REST_BASE_URL,
};
use tokio::sync::mpsc;
use trolly_gym::{
    run_offline_policy_harness, CheckpointOrHoldPolicy, Env, EnvConfig, PolicyProvider,
};
use trolly_strategy::{envelope_message, DepthUpdate, OrderOnlyEgress, PriceLevel, StreamEvent};
use trolly_stream::Message;

pub const DEFAULT_DEMO_ORDER_GUARD_VAR: &str = "RUN_BINANCE_DEMO_ORDERS";

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
    let receipts = if let Some(credentials) = credentials {
        place_spot_demo_orders(credentials, &orders).await?
    } else {
        Vec::new()
    };
    let placed_orders = receipts.len();

    Ok(PolicyDemoReport {
        venue: DemoVenue::Spot,
        symbol: config.symbol,
        policy_source,
        steps,
        execute_demo_orders: config.execute_demo_orders,
        placed_orders,
        receipts,
        orders: PolicyDemoOrders::Spot(orders),
    })
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
    let receipts = if let Some(credentials) = credentials {
        place_usdm_demo_orders(credentials, &orders).await?
    } else {
        Vec::new()
    };
    let placed_orders = receipts.len();

    Ok(PolicyDemoReport {
        venue: DemoVenue::Usdm,
        symbol: config.symbol,
        policy_source,
        steps,
        execute_demo_orders: config.execute_demo_orders,
        placed_orders,
        receipts,
        orders: PolicyDemoOrders::Usdm(orders),
    })
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
        orders: PolicyDemoOrders::Usdm(orders),
    })
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
