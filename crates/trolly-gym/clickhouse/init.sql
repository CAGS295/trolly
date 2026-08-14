CREATE DATABASE IF NOT EXISTS trolly;

CREATE TABLE IF NOT EXISTS trolly.ticks
(
    ts_ms Int64,
    venue LowCardinality(String),
    symbol LowCardinality(String),
    event_kind LowCardinality(String),
    side LowCardinality(String),
    price Float64,
    size Float64,
    bid Float64,
    ask Float64,
    bid_qty Float64,
    ask_qty Float64,
    mid Float64,
    spread Float64,
    update_id UInt64,
    source LowCardinality(String),
    session_id String
)
ENGINE = MergeTree
ORDER BY (symbol, ts_ms, session_id);
