# Workplan

Canonical artifact for the **Daily workplan orchestrator** automation.

## Goals

- Have a command to build a global order book.
- Stream-native execution and account bookkeeping (Binance spot + USDM); outbound order placement as follow-on work items (WP-012–WP-015).
- A strategy layer that consumes multi-symbol stream events and dispatches outbound messages.
- Groundwork for a libtorch.rs training gym fed by trolly streams; toolchain choice deferred to WP-016 analysis.

## Meta

- owner: Daily workplan orchestrator
- last_run: 2026-06-17
- max_parallel: 3

## Orchestrator notes

- Build the **ready set**: items with `status: todo` and all `depends_on` entries `done`.
- Schedule up to `max_parallel` items per wave with **disjoint** `scope` paths.
- Mark selected items `in_progress` before spawning workers; only the orchestrator sets `done` or `blocked` after acceptance checks.
- Workers must not change item status; return the structured payload from the automation prompt.
- On completion: set `last_run`, append a `+` line to [`changelog.md`](changelog.md) **change log**, trim matching **WIP** bullets there.
- **Patched dependencies** (`patches/lob`, root `[patch]` in [`Cargo.toml`](Cargo.toml)): whenever scope touches a patched crate or submodule:
  1. **Comment in** — uncomment the `[patch."https://github.com/CAGS295/lob.git"]` block so `lob = { path = "./patches/lob" }` is active before `cargo test` / `cargo check`.
  2. **Submodule commit** — commit and push lob changes on **`patches/lob` `main`**, then bump the submodule pointer in trolly (`git add patches/lob`).
  3. **Comment out** — re-comment the `[patch]` block before closing the item (default branch builds against git `lob`; patch-in is for local/submodule development only).
  4. **Commit in trolly** — include `Cargo.toml` (patch commented out) and `patches/lob` pointer in the same commit or PR as the work item; do not leave submodule bumps or patch toggles unstaged.
  Orchestrator acceptance: verify `[patch]` is commented out on `main`, submodule pointer matches lob `main` when lob changed, and `git submodule update --init patches/lob && cargo test` passes.

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
| `patches/lob` | Order book merge | submodule | track `main`; **patch in** `[patch]` for dev/tests, **patch out** + commit pointer on ship (see Orchestrator notes) |

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

- status: done
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

- status: done
- repos: trolly
- depends_on: [WP-003]
- scope: src/providers/binance_usd_m.rs, src/bin/aggregated_depth_tui.rs, src/monitor/global_book.rs
- acceptance:
  - RPI stream routing (`binance-usd-m:RPI:SYMBOL`) works end-to-end
  - TUI `Δ` tab shows overlay without polluting canonical global merge
  - `cargo test --features tui` passes when TUI is touched
  - RPI subscription behavior documented in this file
- notes: |
    RPI stays optional. **Subscription:** prefix symbol with `RPI:` (`binance-usd-m:RPI:BTCUSDT`); WebSocket maps to `{symbol}@rpiDepth@500ms` on combined stream; `SET_PROPERTY combined=true` sent before `SUBSCRIBE` when batch includes RPI. **REST:** bare symbol only. **Routing:** `rpiDepth` envelopes get `RPI:` prepended (`RPI:BTCUSDT`), separate from standard `@depth`. **Global merge:** RPI uses merge key `RPI:BTCUSDT`, not cross-source `BTCUSDT`. **TUI Δ tab:** groups `@depth` and `@rpiDepth` under bare instrument; shows `@depth − @rpiDepth` per price when both legs are in `--sources`.

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

- status: done
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

- status: done
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

- status: done
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

- status: done
- repos: trolly
- depends_on: [WP-007]
- scope: crates/binance-usdm-exec/
- acceptance:
  - same constraints as WP-008 for USDM/futures user-data streams (execution + account/position updates)
  - events pushed into `trolly-stream` ingress alongside spot
  - fixture or mock-stream tests; `cargo test -p binance-usdm-exec` passes
- notes: parallel-safe with WP-008 (disjoint crate scopes). Shares patterns from WP-008 but keeps futures-specific types local.

### WP-010 — Strategy component (`trolly-strategy`)

- status: done
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

- status: done
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

### WP-012 — USDM position bookkeeping (`binance-usdm-exec`)

- status: done
- repos: trolly
- depends_on: [WP-009]
- scope: crates/binance-usdm-exec/
- acceptance:
  - `ACCOUNT_UPDATE` position rows persisted in account-wide state (not only per-symbol handler side effects)
  - `SymbolBookkeeping.positions` reflects latest `PositionChange` per `(symbol, position_side)` with clear zero/close semantics
  - balance rows from the same event remain routable to `__account__` without duplicating position state
  - fixture tests cover multi-leg `ACCOUNT_UPDATE` (LONG/SHORT/BOTH) and position flatten
  - `cargo test -p binance-usdm-exec` passes
- notes: WP-009 parses and routes positions; this WP completes durable bookkeeping and query API for strategy / CLI consumers.

### WP-013 — USDM margin-call handling (`binance-usdm-exec`)

- status: done
- repos: trolly
- depends_on: [WP-012]
- scope: crates/binance-usdm-exec/
- acceptance:
  - `MARGIN_CALL` events applied to account state (cross wallet balance + affected positions snapshot)
  - margin-call updates forwarded on the same outbound channel as other `UsdmExecUpdate` variants
  - `__account__` handler records latest margin-call payload (timestamp + positions) for strategy inspection
  - fixture test for `MARGIN_CALL` parse → route → state; `cargo test -p binance-usdm-exec` passes
- notes: parsing exists today; this WP adds persistence, lifecycle (supersede on newer call), and documented semantics for downstream alerts.

### WP-014 — USDM order placement (`binance-usdm-exec`)

- status: done
- repos: trolly
- depends_on: [WP-013]
- scope: crates/binance-usdm-exec/, crates/trolly-strategy/
- acceptance:
  - signed outbound order API (REST `POST /fapi/v1/order` or Binance WebSocket trading API — pick one, document in crate README)
  - request builder covers market/limit basics: symbol, side, quantity, price (limit), `positionSide` where required
  - placement errors surfaced as typed results; no silent fallback
  - integration with `trolly-strategy` egress: strategy can dispatch a normalized place-order command consumed by USDM exec
  - mock or recorded HTTP/WS tests (no live keys in CI); `cargo test -p binance-usdm-exec` passes
- notes: extends WP-009 beyond stream-native bookkeeping. Listen-key create/keepalive may live here or in a small helper module; document caller responsibilities.

### WP-015 — Spot order execution (`binance-spot-exec`)

- status: done
- repos: trolly
- depends_on: [WP-008]
- scope: crates/binance-spot-exec/, crates/trolly-strategy/, src/cli/mod.rs
- acceptance:
  - signed outbound order API (REST `POST /api/v3/order` or Binance WebSocket trading API — pick one, document in crate README)
  - request builder covers market/limit basics: symbol, side, quantity, price (limit), time-in-force
  - fills and rejects still reconciled via existing user-data `executionReport` path (no duplicate state machines)
  - integration with `trolly-strategy` egress and/or `Execute` CLI subcommand stub replaced with a minimal place-order entrypoint
  - mock or recorded HTTP/WS tests (no live keys in CI); `cargo test -p binance-spot-exec` passes
- notes: WP-008 is ingest-only today. This WP adds outbound execution while keeping account book updates stream-driven.

### WP-016 — RL training and inference toolchain analysis (`trolly-gym`)

- status: done
- repos: trolly
- depends_on: [WP-011]
- scope: crates/trolly-gym/, docs/ or crates/trolly-gym/docs/
- acceptance:
  - written analysis (ADR or design doc in-repo) comparing Rust-native and hybrid ML stacks for **training** and **live inference** on stream-fed RL
  - evaluate at minimum: `tch`/libtorch.rs (current `torch` feature), Candle, Burn, ONNX Runtime (`ort`), and a Python/PyTorch sidecar or IPC bridge — with notes on GPU/CPU, libtorch install burden, and CI feasibility
  - map each candidate to `trolly-gym` integration points: [`Env`](crates/trolly-gym/src/env.rs) stepping, [`ObservationWindow`](crates/trolly-gym/src/observation.rs), [`ReplayBuffer`](crates/trolly-gym/src/replay.rs), [`Action`](crates/trolly-gym/src/action.rs) → `trolly-strategy` egress, and stream latency / backpressure constraints
  - cover RL algorithm families relevant to market making / execution (on-policy e.g. PPO, off-policy e.g. DQN/SAC, offline/batch from replay) and which stacks support them without a full rewrite
  - separate recommendations for **offline training** (batch replay, checkpoints, experiment tracking) vs **online inference** (sub-ms to low-ms action loop, model hot-swap, deterministic fallbacks)
  - explicit decision: primary toolchain, optional fallback, and what stays feature-gated in `trolly-gym`; list follow-on implementation WPs (training loop, checkpoint I/O, inference hook) without implementing them here
  - no new runtime dependency required in default `cargo check --workspace`; analysis-only deliverable linked from [`crates/trolly-gym/README.md`](crates/trolly-gym/README.md)
- notes: WP-011 landed the scaffold with an optional `torch`/`tch` gate. This WP is research and architecture — pick stacks before committing to a training loop, GPU CI, or production inference path.

### WP-017 — Binance demo integration tests (spot + USDM)

- status: in_progress
- repos: trolly
- depends_on: [WP-002, WP-008, WP-009]
- scope: .env.example, tests/, crates/binance-spot-exec/, crates/binance-usdm-exec/, README.md
- acceptance:
  - extend [`.env.example`](.env.example) with demo credentials and opt-in flags (pattern matches WP-002): at minimum `DEMO_BINANCE_KEY`, `DEMO_BINANCE_SECRET`, `RUN_BINANCE_DEMO_INTEGRATION=0`, optional `TROLLY_DEMO_SYMBOL` (default `BTCUSDT`); document `cp .env.example .env` — this file is the repo env sample (no separate `.env.sample`)
  - document demo base URLs in README and/or test module docs:
    - **Spot demo** — [Spot Demo general info](https://developers.binance.com/docs/binance-spot-api-docs/demo-mode/general-info): REST `https://demo-api.binance.com/api`, WS API `wss://demo-ws-api.binance.com/ws-api/v3`, market streams `wss://demo-stream.binance.com/ws` (map from production hosts in [`src/providers/depth/binance/spot.rs`](src/providers/depth/binance/spot.rs) and [`crates/binance-spot-exec`](crates/binance-spot-exec))
    - **USDM demo** — [Derivatives docs](https://developers.binance.com/docs/derivatives/): REST `https://demo-fapi.binance.com`, market streams `wss://fstream.binancefuture.com` per [USDM general info](https://developers.binance.com/docs/derivatives/usds-margined-futures/general-info); user-data via `POST /fapi/v1/listenKey` on demo REST + private WS per [`crates/binance-usdm-exec`](crates/binance-usdm-exec)
  - `#[ignore]` integration tests (require `RUN_BINANCE_DEMO_INTEGRATION=1`, `--ignored`, and demo keys in `.env`):
    - spot: demo REST depth snapshot + signed user-data subscribe on demo WS API; assert parsed `executionReport` / account events when demo account activity exists (or skip with clear message if idle)
    - USDM: demo REST depth + listenKey lifecycle on `demo-fapi.binance.com` + user-data stream; assert `ORDER_TRADE_UPDATE` / `ACCOUNT_UPDATE` parsing against live demo payloads when available
  - default `cargo test --workspace` stays offline; demo tests skip cleanly when flag unset or keys missing
  - optional follow-on (after WP-014 / WP-015): demo order place → user-stream reconcile round-trip for spot and USDM — document as sub-checklist in test module, not blocking this WP
- notes: uses Binance **demo/testnet** endpoints only — never production keys. Complements WP-002 (public REST merge); this WP adds authenticated streams and venue-specific demo host wiring. Geo/network restrictions may skip in CI; verify on unrestricted egress like WP-002.

## Completed milestones

- [x] **Cross-source merge:** [`BookSource`](src/monitor/global_book.rs) → [`GlobalBookHub`](src/monitor/global_book.rs) via [`LimitOrderBook::merge_aggregate`](patches/lob/src/limit_order_book/mod.rs).
- [x] **CLI serve:** `monitor depth --output global --sources binance:BTCUSDT,binance-usd-m:BTCUSDT --server-port 50051`
- [x] **Prometheus:** `GET /metrics` (`trolly_depth_updates_total`, `trolly_global_book_merge_refresh_total`).
- [x] **Submodule:** `patches/lob` wired for local `merge_aggregate` patch.
