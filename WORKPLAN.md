# Workplan

Canonical artifact for the **Daily workplan orchestrator** automation.

## Goals

- Have a command to build a global order book.
- Stream-native execution and account bookkeeping (Binance spot + USDM, no REST).
- A strategy layer that consumes multi-symbol stream events and dispatches outbound messages.
- Groundwork for a libtorch.rs training gym fed by trolly streams.

## Meta

- owner: Daily workplan orchestrator
- last_run: 2026-06-12
- max_parallel: 3

## Orchestrator notes

- Build the **ready set**: items with `status: todo` and all `depends_on` entries `done`.
- Schedule up to `max_parallel` items per wave with **disjoint** `scope` paths.
- Mark selected items `in_progress` before spawning workers; only the orchestrator sets `done` or `blocked` after acceptance checks.
- Workers must not change item status; return the structured payload from the automation prompt.
- On completion: set `last_run`, append a `+` line to [`changelog.md`](changelog.md) **change log**, trim matching **WIP** bullets there.

## Crate architecture

Standalone workspace crates for compile-time isolation and spatial locality. Heavy or venue-specific code must not live in the `trolly` binary crate.

| Crate | Owns | Standalone? | Rationale |
|-------|------|-------------|-----------|
| `trolly-stream` | `EventHandler`, multiplexor, ws ingress/routing, shared stream event types | **yes** | Shared hot path for monitor, execution, strategy, and gym; refactor target for injectable websocket messages |
| `binance-spot-exec` | Spot execution + account bookkeeping over user-data streams only | **yes** | Venue boundary; compiles in parallel with USDM |
| `binance-usdm-exec` | USDM execution + account bookkeeping over user-data streams only | **yes** | Same; futures-specific types stay local |
| `trolly-strategy` | Strategy runtime: consume multi-symbol events, hold state, dispatch outbound stream messages | **yes** | Core state-handling unit; depends only on `trolly-stream` |
| `trolly-gym` | libtorch.rs training gym scaffold: observation windows, replay, inference hook over streams | **yes** | `torch` feature-gated; avoids rebuilding monitor/server on model edits |
| `trolly` (root) | CLI, depth monitor, global book hub, gRPC/SCALE servers | no (app) | Composes workspace crates; keeps `lob`/server features here for now |
| `patches/lob` | Order book merge | submodule | unchanged |

**Dependency DAG:** `trolly-stream` ← `{binance-spot-exec, binance-usdm-exec, trolly-strategy}` ← `trolly-gym` ← `trolly`.

**Stay in root for now:** `src/monitor/`, `src/servers/` (tightly coupled to `lob` and optional grpc/scale features). Migrate only when a second consumer needs them.

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

- status: in_progress
- repos: trolly
- depends_on: []
- scope: .env.example, tests/global_book.rs, WORKPLAN.md, changelog.md, README.md
- acceptance:
  - documented flow: copy `.env.example` → `.env`, set `RUN_GLOBAL_BOOK_INTEGRATION=1`, run live test
  - `cargo test --test global_book global_book_live_rest_merge -- --ignored` passes when env enabled
  - default `cargo test` still skips live network; fixture tests always run
- notes: complements WP-001; safe to run in parallel (disjoint scope).
- worker (2026-06-12): documented offline vs live flow in README, `.env.example`, and `tests/global_book.rs` module docs. Default `cargo test --test global_book` runs 3 fixture tests and ignores live REST; live test requires `cp .env.example .env`, `RUN_GLOBAL_BOOK_INTEGRATION=1`, and `--ignored`. Automation VM got HTTP 451 from Binance (geo/network); verify live pass on unrestricted egress.

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
- scope: src/providers/binance_usd_m.rs, src/bin/aggregated_depth_tui.rs, src/monitor/global_book.rs
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

### WP-006 — Workspace layout and crate scaffold

- status: in_progress
- repos: trolly
- depends_on: []
- scope: Cargo.toml, crates/trolly-stream/, crates/binance-spot-exec/, crates/binance-usdm-exec/, crates/trolly-strategy/, crates/trolly-gym/
- acceptance:
  - root `Cargo.toml` declares a workspace with the five crates above (empty or stub `lib.rs` each)
  - `cargo check --workspace` passes
  - crate architecture table in this file matches the created layout
  - `trolly` root crate lists workspace members as path dependencies (stubs ok)
- notes: coordinate with WP-003 — venue-specific depth code may later move into exec crates; do not block WP-006 on WP-003.

### WP-007 — Injectable multi-symbol stream (`trolly-stream`)

- status: todo
- repos: trolly
- depends_on: [WP-006]
- scope: crates/trolly-stream/, src/connectors/multiplexor.rs, src/connectors/handler.rs, src/net/
- acceptance:
  - extract multiplexor + `EventHandler` + ws adapter into `trolly-stream`
  - ingress API accepts injected `Message` values (not only socket reads) and routes by `EventHandler::to_id`
  - existing depth monitor paths compile against `trolly-stream` with unchanged behavior
  - unit test: push synthetic websocket text into hub → correct per-symbol handler invoked
  - `cargo test --workspace` passes
- notes: prerequisite for execution crates and strategy. Today `MonitorMultiplexor::stream` only reads from `subscribe()`; execution user-data events must fan in through the same router.

### WP-008 — Binance spot execution crate (`binance-spot-exec`)

- status: todo
- repos: trolly
- depends_on: [WP-007]
- scope: crates/binance-spot-exec/
- acceptance:
  - order execution and account bookkeeping driven by websocket user-data streams only (no REST trading endpoints)
  - parsed execution/account events pushed into `trolly-stream` ingress (reuse multiplexor routing)
  - stream subscription setup documented; multi-symbol subscription compatible with `trolly-stream`
  - fixture or mock-stream tests for order/trade/balance update parsing
  - `cargo test -p binance-spot-exec` passes
- notes: Binance spot user data stream + execution report events. REST remains allowed for read-only snapshots elsewhere in trolly, not in this crate.

### WP-009 — Binance USDM execution crate (`binance-usdm-exec`)

- status: todo
- repos: trolly
- depends_on: [WP-007]
- scope: crates/binance-usdm-exec/
- acceptance:
  - same constraints as WP-008 for USDM/futures user-data streams (execution + account/position updates)
  - events pushed into `trolly-stream` ingress alongside spot
  - fixture or mock-stream tests; `cargo test -p binance-usdm-exec` passes
- notes: parallel-safe with WP-008 (disjoint crate scopes). Shares patterns from WP-008 but keeps futures-specific types local.

### WP-010 — Strategy component (`trolly-strategy`)

- status: todo
- repos: trolly
- depends_on: [WP-007]
- scope: crates/trolly-strategy/
- acceptance:
  - strategy runtime subscribes to multi-symbol events from `trolly-stream` (depth, execution, account)
  - single core state-handling unit: consume updates, apply transitions, dispatch outbound messages back through stream egress API
  - `Strategy` trait (or equivalent) with test double that records consumed events and dispatched commands
  - integration test with synthetic injected events (no live network required)
  - `cargo test -p trolly-strategy` passes
- notes: parallel-safe with WP-008 / WP-009 once WP-007 is done. Does not embed venue-specific parsing — consumes normalized stream events.

### WP-011 — Libtorch gym groundwork (`trolly-gym`)

- status: todo
- repos: trolly
- depends_on: [WP-007, WP-010]
- scope: crates/trolly-gym/
- acceptance:
  - `torch` feature flag gates all libtorch.rs code; default `cargo check --workspace` does not require libtorch
  - scaffold: `Env` (or equivalent) stepping on stream-fed observations, action dispatch via `trolly-strategy` egress
  - replay buffer or ring buffer stub storing stream-derived feature windows
  - one offline smoke test with mock observations (no GPU required in CI)
  - README section in crate documents build (`--features torch`) and dependency on libtorch
- notes: training loop and model checkpoints out of scope; this WP is layout + stream integration hooks only.

## Completed milestones

- [x] **Cross-source merge:** [`BookSource`](src/monitor/global_book.rs) → [`GlobalBookHub`](src/monitor/global_book.rs) via [`LimitOrderBook::merge_aggregate`](patches/lob/src/limit_order_book/mod.rs).
- [x] **CLI serve:** `monitor depth --output global --sources binance:BTCUSDT,binance-usd-m:BTCUSDT --server-port 50051`
- [x] **Prometheus:** `GET /metrics` (`trolly_depth_updates_total`, `trolly_global_book_merge_refresh_total`).
- [x] **Submodule:** `patches/lob` wired for local `merge_aggregate` patch.
