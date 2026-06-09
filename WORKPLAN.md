# Workplan

Canonical artifact for the **Daily workplan orchestrator** automation.

## Goals

- Have a command to build a global order book.

## Meta

- owner: Daily workplan orchestrator
- last_run: 2026-06-09T08:15:00Z
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
  - **Subscription:** add `binance-usd-m:RPI:SYMBOL` to `--sources` (comma-separated with standard legs). Example: `binance-usd-m:BTCUSDT,binance-usd-m:RPI:BTCUSDT`. The provider strips `RPI:` for REST snapshot and subscribes to `@rpiDepth@500ms` (with `SET_PROPERTY combined` when any RPI leg is present). WebSocket routing keys updates as `RPI:SYMBOL` via [`depth_parse`](src/monitor/depth_parse.rs).
  - **Global merge:** [`BookSource::canonical_instrument`](src/monitor/global_book.rs) keeps `RPI:SYMBOL` as a separate merge bucket, so `@rpiDepth` never folds into the canonical `SYMBOL` book unless you subscribe both under the same symbol name.
  - **TUI Δ tab:** groups by base instrument (`BTCUSDT`); shows `@depth − @rpiDepth` per price when both `binance-usd-m:SYMBOL` and `binance-usd-m:RPI:SYMBOL` are in the hub. The diff is computed in the TUI only and does not alter merged books.

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

## Completed milestones

- [x] **Cross-source merge:** [`BookSource`](src/monitor/global_book.rs) → [`GlobalBookHub`](src/monitor/global_book.rs) via [`LimitOrderBook::merge_aggregate`](patches/lob/src/limit_order_book/mod.rs).
- [x] **CLI serve:** `monitor depth --output global --sources binance:BTCUSDT,binance-usd-m:BTCUSDT --server-port 50051`
- [x] **Prometheus:** `GET /metrics` (`trolly_depth_updates_total`, `trolly_global_book_merge_refresh_total`).
- [x] **Submodule:** `patches/lob` wired for local `merge_aggregate` patch.
