# Workplan

Canonical artifact for the **Daily workplan orchestrator** automation.

## Goals

- Have a command to build a global order book.

## Meta

- owner: Daily workplan orchestrator
- last_run: 2026-06-07T21:30:00Z
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
- notes: `refresh_merged_for` uses `merge_into` on read guards for multi-source merge (no per-source clone); single-source path still clones once for `MergedOp::Replace`.

### WP-002 — Integration test hygiene

- status: done
- repos: trolly
- depends_on: []
- scope: .env.example, tests/global_book.rs, WORKPLAN.md, changelog.md, README.md
- acceptance:
  - documented flow: copy `.env.example` → `.env`, set `RUN_GLOBAL_BOOK_INTEGRATION=1`, run live test
  - `cargo test --test global_book global_book_live_rest_merge -- --ignored` passes when env enabled
  - default `cargo test` still skips live network; fixture tests always run
- notes: complements WP-001; safe to run in parallel (disjoint scope). Opt-in live flow — `cp .env.example .env`, set `RUN_GLOBAL_BOOK_INTEGRATION=1`, run `cargo test --test global_book global_book_live_rest_merge -- --ignored`. Default `cargo test` skips live network (`#[ignore]`); fixture tests in `tests/global_book.rs` always run. When Binance REST is geo-blocked, the live test falls back to a loopback REST stub.

### WP-003 — Provider expansion scaffold

- status: done
- repos: trolly
- depends_on: []
- scope: src/providers/, src/monitor/mod.rs, src/providers/.todo
- acceptance:
  - Binance spot refactor toward `depth::binance::spot` (per `.todo`) or documented equivalent layout
  - third venue can register in `--sources provider:SYMBOL` without breaking binance / binance-usd-m
  - `parse_book_sources` unit tests cover new layout; `cargo test` passes
- notes: Binance spot at `providers::depth::binance::spot`; `other` venue scaffold registered. See `src/providers/.todo` for remaining migrations.

### WP-004 — Intra-provider overlays (Binance RPI)

- status: done
- repos: trolly
- depends_on: [WP-003]
- scope: src/providers/binance_usd_m.rs, src/bin/aggregated_depth_tui.rs, src/monitor/global_book.rs
- acceptance:
  - RPI stream routing (`binance-usd-m:RPI:SYMBOL`) works end-to-end
  - TUI `Δ` tab shows overlay without polluting canonical global merge
  - `cargo test --features tui` passes when TUI is touched
  - RPI subscription behavior documented in this file
- notes: see `src/providers/.todo` — `add rpi support`. RPI stays optional.
  - **Subscription:** pass `binance-usd-m:RPI:SYMBOL` in `--sources` (symbol field is `RPI:SYMBOL` after the first `:`). `BinanceUsdM::ws_subscriptions` maps it to `{symbol}@rpiDepth@500ms`, sends `SET_PROPERTY combined=true` when any RPI leg is present, and REST snapshots strip the `RPI:` prefix.
  - **Routing:** combined-stream envelopes with `rpiDepth` in the stream name get `RPI:` prepended to `DepthUpdate.event.symbol` (`depth_parse`) so updates route to `binance-usd-m:RPI:SYMBOL`, distinct from `binance-usd-m:SYMBOL` (`order_book::to_id`).
  - **Global merge:** `BookSource::canonical_instrument` keeps `RPI:SYMBOL` as its own merge lane; standard `SYMBOL` merge never includes the RPI leg unless you deliberately use the same symbol name for both.
  - **TUI Δ tab:** `aggregated_depth_tui` computes `@depth − @rpiDepth` per price from the two hub legs only; it does not write back into the canonical `MERGED·SYMBOL` book. Example: `--sources binance-usd-m:BTCUSDT,binance-usd-m:RPI:BTCUSDT`.

### WP-005 — Cleanup

- status: done
- repos: trolly
- depends_on: []
- scope: src/servers/mod.rs, src/cli/mod.rs
- acceptance:
  - no `Hook::new` / `Hook::register` dead_code warning in `src/servers/mod.rs`
  - `long_about` in `src/cli/mod.rs` describes project goals (not a TODO placeholder)
  - `cargo test` passes
- notes: `Hook::new`/`register` wired into serve paths; CLI `long_about` describes LOB monitoring and serving goals.

### WP-006 — USDM provider layout migration

- status: done
- repos: trolly
- depends_on: []
- scope: src/providers/binance_usd_m.rs, src/providers/depth/binance/, src/providers/mod.rs, tests/binance_usd_m.rs
- acceptance:
  - `BinanceUsdM` lives at `providers::depth::binance::usd_m` (re-export from `providers` unchanged)
  - all `binance_usd_m` unit/integration tests pass
  - `src/providers/.todo` updated to mark migration done
  - `cargo test` passes
- notes: `BinanceUsdM` at `providers::depth::binance::usd_m`; public re-exports unchanged; RPI intact.

### WP-007 — Single-source merge without clone

- status: done
- repos: trolly, patches/lob
- depends_on: [WP-001]
- scope: src/monitor/global_book.rs, patches/lob/src/limit_order_book/mod.rs
- acceptance:
  - `refresh_merged_for` single-source path avoids full `LimitOrderBook` clone when possible
  - merge semantics unchanged (`tests/global_book.rs`, `patches/lob` merge tests)
  - `cargo test` passes
- notes: unified refresh path uses `merge_into` on read guards for all source counts; `replace_from` avoids clone in `MergedOp::sync_with`.

### WP-008 — Venue onboarding checklist

- status: done
- repos: trolly
- depends_on: [WP-006]
- scope: README.md, src/providers/.todo, WORKPLAN.md, tests/global_book.rs
- acceptance:
  - README documents steps to add a new exchange provider (module, labels, multiplexor, tests)
  - `src/providers/.todo` reflects current state (no stale unchecked items for done work)
  - at least one unit test references the checklist layout (e.g. `REGISTERED_LABELS`)
  - `cargo test` passes
- notes: complements provider scaffold; orchestrator updates WORKPLAN status only.
  - **README:** new [Adding a new exchange provider](README.md#adding-a-new-exchange-provider) section (module, `REGISTERED_LABELS`, `Provider::from_label`, `run_global_book_stream` multiplexor arm, tests).
  - **`.todo`:** USDM migration and RPI marked done; only `Provider::Other` live-stream wiring remains open.
  - **Tests:** `registered_labels_match_provider_onboarding_checklist` in `tests/global_book.rs` asserts each `REGISTERED_LABELS` entry round-trips through `Provider::from_label` and `parse_book_sources`.

## Completed milestones

- [x] **Cross-source merge:** [`BookSource`](src/monitor/global_book.rs) → [`GlobalBookHub`](src/monitor/global_book.rs) via [`LimitOrderBook::merge_aggregate`](patches/lob/src/limit_order_book/mod.rs).
- [x] **CLI serve:** `monitor depth --output global --sources binance:BTCUSDT,binance-usd-m:BTCUSDT --server-port 50051`
- [x] **Prometheus:** `GET /metrics` (`trolly_depth_updates_total`, `trolly_global_book_merge_refresh_total`).
- [x] **Submodule:** `patches/lob` wired for local `merge_aggregate` patch.
