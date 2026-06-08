# Workplan

Canonical artifact for the **Daily workplan orchestrator** automation.

## Goals

- Have a command to build a global order book.

## Meta

- owner: Daily workplan orchestrator
- last_run: 2026-06-08T08:35:00Z
- max_parallel: 3

## Orchestrator notes

- Build the **ready set**: items with `status: todo` and all `depends_on` entries `done`.
- Schedule up to `max_parallel` items per wave with **disjoint** `scope` paths.
- Mark selected items `in_progress` before spawning workers; only the orchestrator sets `done` or `blocked` after acceptance checks.
- Workers must not change item status; return the structured payload from the automation prompt.
- On completion: set `last_run`, append a `+` line to [`changelog.md`](changelog.md) **change log**, trim matching **WIP** bullets there.

## Items

### WP-001 — Hot-path allocation optimization

- status: done
- repos: trolly, patches/lob
- depends_on: []
- scope: src/monitor/global_book.rs, patches/lob/src/limit_order_book/mod.rs, benches/
- acceptance:
  - `git submodule update --init patches/lob && cargo test` passes
  - merge semantics unchanged (`tests/global_book.rs`, `patches/lob` merge tests)
  - fewer full-book clones on `GlobalBookHub::refresh_merged_for` hot path
- notes: `refresh_merged_for` currently clones every source book before `merge_aggregate`.

### WP-002 — Integration test hygiene

- status: done
- repos: trolly
- depends_on: []
- scope: .env.example, tests/global_book.rs, WORKPLAN.md, changelog.md, README.md
- acceptance:
  - documented flow: copy `.env.example` → `.env`, set `RUN_GLOBAL_BOOK_INTEGRATION=1`, run live test
  - `cargo test --test global_book global_book_live_rest_merge -- --ignored` passes when env enabled
  - default `cargo test` still skips live network; fixture tests always run
- notes: complements WP-001; safe to run in parallel (disjoint scope).

### WP-003 — Provider expansion scaffold

- status: done
- repos: trolly
- depends_on: []
- scope: src/providers/, src/monitor/mod.rs, src/providers/.todo
- acceptance:
  - Binance spot refactor toward `depth::binance::spot` (per `.todo`) or documented equivalent layout
  - third venue can register in `--sources provider:SYMBOL` without breaking binance / binance-usd-m
  - `parse_book_sources` unit tests cover new layout; `cargo test` passes
- notes: see `src/providers/.todo` — `move binance to depth::binance::spot`.

### WP-004 — Intra-provider overlays (Binance RPI)

- status: in_progress
- repos: trolly
- depends_on: [WP-003]
- scope: src/providers/binance/usd_m.rs, src/bin/aggregated_depth_tui.rs, src/monitor/global_book.rs
- acceptance:
  - RPI stream routing (`binance-usd-m:RPI:SYMBOL`) works end-to-end
  - TUI `Δ` tab shows overlay without polluting canonical global merge
  - `cargo test --features tui` passes when TUI is touched
  - RPI subscription behavior documented in this file
- notes: see `src/providers/.todo` — `add rpi support`. RPI stays optional.

### WP-005 — Cleanup

- status: done
- repos: trolly
- depends_on: []
- scope: src/servers/mod.rs, src/cli/mod.rs
- acceptance:
  - no `Hook::new` / `Hook::register` dead_code warning in `src/servers/mod.rs`
  - `long_about` in `src/cli/mod.rs` describes project goals (not a TODO placeholder)
  - `cargo test` passes
- notes: safe to run in parallel with WP-001 / WP-002 / WP-003 (disjoint scope).

## Integration test reference

The global-book integration test (`tests/global_book.rs`) has two layers:

| Layer | Runs on `cargo test` | Network required |
|-------|---------------------|-----------------|
| Fixture tests (parse, merge, stream-ID) | Always | No |
| `global_book_live_rest_merge` (`#[ignore]`) | Only with `--ignored` + env | Yes (Binance REST) |

**Enabling the live test:**

```bash
cp .env.example .env
# set RUN_GLOBAL_BOOK_INTEGRATION=1 in .env
cargo test --test global_book global_book_live_rest_merge -- --ignored
```

The env guard (`RUN_GLOBAL_BOOK_INTEGRATION`) ensures the test body exits early even if accidentally invoked without the flag, so CI remains network-free by default.

## RPI subscription behavior

**RPI** (Retail Price Improvement) is an optional Binance USDM overlay stream (`@rpiDepth@500ms`)
that includes RPI-only liquidity layers. It runs alongside the standard `@depth` stream on the
same combined WebSocket connection.

### Stream routing

| CLI source | Stream ID | WS subscription | Canonical instrument |
|---|---|---|---|
| `binance-usd-m:BTCUSDT` | `binance-usd-m:BTCUSDT` | `btcusdt@depth` | `BTCUSDT` |
| `binance-usd-m:RPI:BTCUSDT` | `binance-usd-m:RPI:BTCUSDT` | `btcusdt@rpiDepth@500ms` | `RPI:BTCUSDT` |

### Subscription protocol

When any symbol in the subscription list carries the `RPI:` prefix:

1. A `SET_PROPERTY` message (`{"method":"SET_PROPERTY","params":["combined",true],"id":0}`) is sent first to enable the combined stream envelope format.
2. The `SUBSCRIBE` message lists all streams (both `@depth` and `@rpiDepth`) in a single params array.

Standard-only subscriptions skip the `SET_PROPERTY` step.

### Isolation from canonical merge

RPI sources use `canonical_instrument() == "RPI:SYMBOL"` which is distinct from the standard
`"SYMBOL"`. This means:

- The `GlobalBookHub` merged lane for `BTCUSDT` only aggregates non-RPI sources.
- RPI books get their own merged lane (`RPI:BTCUSDT`) and never pollute the canonical instrument.
- The TUI `Δ·INSTRUMENT` tab computes `@depth − @rpiDepth` per price without touching the merge.

### REST snapshot

The REST API URL always strips the `RPI:` prefix — both `binance-usd-m:BTCUSDT` and
`binance-usd-m:RPI:BTCUSDT` fetch the same `/fapi/v1/depth?symbol=BTCUSDT&limit=1000` snapshot
as their initial book state. The divergence happens only on the WebSocket diff stream.

### Depth parse (envelope detection)

Messages arriving with a `"stream"` field containing `"rpiDepth"` have their symbol prefixed
with `RPI:` during parsing (`depth_parse.rs`). This ensures `EventHandler::to_id()` routes
RPI updates to the `RPI:SYMBOL` shard and standard updates to the `SYMBOL` shard, even when
both coexist on the same multiplexed WebSocket connection.

### TUI Δ tab

The `Δ·INSTRUMENT` tab in the TUI binary shows per-price quantity differences:
`qty(@depth) − qty(@rpiDepth)`. Both `binance-usd-m:SYMBOL` and `binance-usd-m:RPI:SYMBOL`
must be present in `--sources` for the Δ tab to render data; otherwise it displays a diagnostic
message. Positive Δ indicates more size on the public depth stream than the RPI stream at that
price level.

### Usage example

```bash
cargo run --features tui --bin aggregated_depth_tui -- \
  --sources binance-usd-m:BTCUSDT,binance-usd-m:RPI:BTCUSDT
```

This subscribes to both the standard and RPI depth streams. The TUI shows:
- `MERGED·BTCUSDT` — canonical merged book (standard only)
- `binance-usd-m:BTCUSDT` — per-source standard depth
- `binance-usd-m:RPI:BTCUSDT` — per-source RPI depth
- `Δ·BTCUSDT` — public depth minus RPI depth overlay
- `MERGED·RPI:BTCUSDT` — merged RPI book (single source)
- `Δ·RPI:BTCUSDT` — undefined (no standard+RPI pair for that canonical)

## Completed milestones

- [x] **Cross-source merge:** [`BookSource`](src/monitor/global_book.rs) → [`GlobalBookHub`](src/monitor/global_book.rs) via [`LimitOrderBook::merge_aggregate`](patches/lob/src/limit_order_book/mod.rs).
- [x] **CLI serve:** `monitor depth --output global --sources binance:BTCUSDT,binance-usd-m:BTCUSDT --server-port 50051`
- [x] **Prometheus:** `GET /metrics` (`trolly_depth_updates_total`, `trolly_global_book_merge_refresh_total`).
- [x] **Submodule:** `patches/lob` wired for local `merge_aggregate` patch.
- [x] **RPI overlay:** `binance-usd-m:RPI:SYMBOL` routing end-to-end, TUI Δ tab, isolation from canonical merge.
