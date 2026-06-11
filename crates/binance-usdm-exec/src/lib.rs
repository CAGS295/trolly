//! Binance USDM execution and account/position bookkeeping over user-data streams.
//!
//! Parsed execution and account events are pushed into [`trolly_stream`] ingress
//! alongside spot and depth feeds. REST trading endpoints are intentionally absent;
//! obtain a listen key externally and connect via [`endpoints::UsdmUserDataEndpoints`].

mod endpoints;
mod events;
mod ingress;
mod ledger;
mod parse;

pub use endpoints::UsdmUserDataEndpoints;
pub use events::{
    AccountUpdate, BalanceUpdate, ListenKeyExpired, MarginCall, OrderDetail, OrderTradeUpdate,
    PositionUpdate, RoutedUsdmEvent, SymbolLedgerSnapshot, UsdmExec, UsdmStreamEvent,
};
pub use ingress::{
    build_symbol_ledgers, expand_user_data, push_user_data_through_ingress, route_routed_events,
    route_user_data,
};
pub use ledger::{UsdmSymbolLedger, UsdmUserDataProvider};
pub use parse::{parse_user_data_bytes, parse_user_data_message, ParseError};
pub use trolly_stream;

/// Crate identifier for workspace wiring tests.
pub const CRATE: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod workspace_wiring {
    #[test]
    fn crate_name() {
        assert_eq!(super::CRATE, "binance-usdm-exec");
    }

    #[test]
    fn reexports_trolly_stream_ingress() {
        let (_ingress, _rx) = trolly_stream::message_ingress();
    }
}
