use lob::DepthUpdate;

#[test]
fn deserialize_usd_m_depth_update_fixture() {
    let json = include_str!("fixtures/binance_usd_m_depth_update.json");
    let update: DepthUpdate = serde_json::from_str(json).unwrap();

    assert_eq!(update.first_update_id, 4541941613);
    assert_eq!(update.last_update_id, 4541941620);
    assert_eq!(update.previous_update_id, Some(4541941610));
    assert_eq!(update.transaction_time, Some(1714060799999));
    assert_eq!(update.event.symbol, "BTCUSDT");
    assert_eq!(update.event.time, 1714060800000);
    assert_eq!(update.bids.len(), 3);
    assert_eq!(update.asks.len(), 2);
}

#[test]
fn spot_depth_update_defaults_futures_fields() {
    let json = r#"{
        "e": "depthUpdate",
        "E": 123456789,
        "s": "BNBBTC",
        "U": 157,
        "u": 160,
        "b": [["0.0024", "10"]],
        "a": [["0.0026", "100"]]
    }"#;
    let update: DepthUpdate = serde_json::from_str(json).unwrap();

    assert_eq!(update.first_update_id, 157);
    assert_eq!(update.last_update_id, 160);
    assert_eq!(update.previous_update_id, None);
    assert_eq!(update.transaction_time, None);
}
