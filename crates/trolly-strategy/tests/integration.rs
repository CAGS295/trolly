//! Integration test: synthetic multi-symbol events through stream ingress and egress.

use std::collections::HashMap;

use trolly_strategy::{
    envelope_message, route_message, AccountUpdate, DepthUpdate, EventKind, ExecutionUpdate,
    OutboundMessage, PriceLevel, RecordingEgress, RecordingStrategy, StreamEvent,
    StrategyEventHandler, StrategyHub,
};

#[test]
fn synthetic_multi_symbol_depth_execution_account_flow() {
    let outbound = OutboundMessage::OrderRequest {
        symbol: "BTCUSDT".into(),
        side: "BUY".into(),
        qty: "0.01".into(),
        price: Some("100".into()),
        position_side: None,
    };
    let mut hub = StrategyHub::new(
        RecordingStrategy::with_responses(vec![outbound.clone()]),
        RecordingEgress::default(),
    );

    let events = [
        StreamEvent::Depth(DepthUpdate {
            symbol: "BTCUSDT".into(),
            bids: vec![PriceLevel {
                price: "99".into(),
                qty: "2".into(),
            }],
            asks: vec![PriceLevel {
                price: "101".into(),
                qty: "1".into(),
            }],
            update_id: Some(10),
        }),
        StreamEvent::Execution(ExecutionUpdate {
            symbol: "ETHUSDT".into(),
            order_id: "7".into(),
            side: "SELL".into(),
            fill_qty: "1".into(),
            fill_price: "3000".into(),
        }),
        StreamEvent::Account(AccountUpdate {
            asset: "USDT".into(),
            balance: "500".into(),
            symbol: Some("BTCUSDT".into()),
        }),
    ];

    for event in &events {
        hub.ingest_message(envelope_message(event)).unwrap();
    }

    let strategy = hub.runtime().strategy();
    assert_eq!(strategy.consumed.len(), 3);
    assert_eq!(strategy.consumed[0].kind(), EventKind::Depth);
    assert_eq!(strategy.consumed[1].kind(), EventKind::Execution);
    assert_eq!(strategy.consumed[2].kind(), EventKind::Account);

    let egress = hub.runtime().egress();
    assert_eq!(egress.dispatched.len(), 3);
    assert!(egress
        .dispatched
        .iter()
        .all(|msg| matches!(msg, OutboundMessage::OrderRequest { .. })));
    assert_eq!(egress.dispatched[0], outbound);
}

#[test]
fn multiplexor_style_routing_with_shared_strategy_runtime() {
    let handler =
        StrategyEventHandler::new(RecordingStrategy::default(), RecordingEgress::default());
    let shared = handler.shared_runtime();

    let mut writers: HashMap<String, StrategyEventHandler<RecordingStrategy, RecordingEgress>> =
        HashMap::new();
    for sym in ["BTCUSDT", "ETHUSDT", "SOLUSDT"] {
        writers.insert(sym.into(), handler.clone());
    }

    route_message(
        &mut writers,
        envelope_message(&StreamEvent::Depth(DepthUpdate {
            symbol: "BTCUSDT".into(),
            bids: vec![],
            asks: vec![],
            update_id: None,
        })),
    );
    route_message(
        &mut writers,
        envelope_message(&StreamEvent::Execution(ExecutionUpdate {
            symbol: "ETHUSDT".into(),
            order_id: "1".into(),
            side: "BUY".into(),
            fill_qty: "1".into(),
            fill_price: "1".into(),
        })),
    );
    route_message(
        &mut writers,
        envelope_message(&StreamEvent::Account(AccountUpdate {
            asset: "USDT".into(),
            balance: "1".into(),
            symbol: Some("SOLUSDT".into()),
        })),
    );

    let guard = shared.borrow();
    let consumed = &guard.strategy().consumed;
    assert_eq!(consumed.len(), 3);
    assert_eq!(
        consumed
            .iter()
            .map(StreamEvent::routing_id)
            .collect::<Vec<_>>(),
        vec!["BTCUSDT", "ETHUSDT", "SOLUSDT"]
    );
}
