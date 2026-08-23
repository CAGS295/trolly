# Workplan

Canonical artifact for the **Daily workplan orchestrator** automation.

## Broad goal (north star)

**Close the loop from GPU-trained WoLF-PPO policies to demo/live Binance trading.** A checkpointed MLP or LNN policy must consume multi-symbol `trolly-stream` observations, select actions through `trolly-strategy`, and place/reconcile orders via `binance-spot-exec` / `binance-usdm-exec` (demo first). Weekday GPU training on this host improves those checkpoints; matrix-game NES distance stays a regression gate, not the destination.

Done means: a saved gym checkpoint can be loaded, run against injected or demo streams, dispatch the same typed place-order commands `Action::dispatch` already fulfills, and leave newer checkpoint metrics or a moved WP as evidence. Do not block on sudo ROCm.

Shipped so far (not the destination): global book CLI; stream-native spot/USDM bookkeeping + outbound placement; strategy egress; gym scaffold; WoLF-PPO + LNN; matrix-game and microstructure trainers.

## Goals

- Have a command to build a global order book.
- Stream-native execution and account bookkeeping (Binance spot + USDM); outbound order placement as follow-on work items (WP-012–WP-015).
- A strategy layer that consumes multi-symbol stream events and dispatches outbound messages.
- Groundwork for a libtorch.rs training gym fed by trolly streams; toolchain choice deferred to WP-016 analysis.
- Nash-equilibrium-oriented RL via **WoLF-PPO** ([Ratcliffe et al., IEEE CoG 2019](https://ieee-cog.org/2019/papers/paper_176.pdf)) on the primary `tch`/libtorch.rs stack (`torch` feature); validate on matrix games before stream-backed trading policies.

## Meta

- owner: Daily workplan orchestrator
- last_run: 2026-08-23
- max_parallel: 3
- ship_branch: integrate/orchestrator-branches

## Orchestrator notes

- **Git ship workflow (no PRs):** checkout `ship_branch` from Meta (`integrate/orchestrator-branches`). At run start, `git fetch origin` then `git rebase @{u}` (or `git pull --rebase`) when the working tree is clean so this host matches what cloud agents already pushed. If rebase would destroy uncommitted work, fetch, warn, and continue — never `reset --hard`, never `git rebase -i`, never `--no-verify`, never force-push. All commits from this run land on the ship branch. Push `ship_branch` to origin at end of run. Do **not** open pull requests, run `gh pr create`, or create `cursor/workplan-orchestrator-process-*` branches.
- **Daily progress (mandatory):** each run must leave a real increment toward the **Broad goal**. A health check, empty ready-set log, or “WP-001–WP-022 all done” line is a **failed run**. Before any local train slice, fetch-first as above so cloud and this host do not diverge. Assess current state vs the goal (`WORKPLAN.md` statuses, [`changelog.md`](changelog.md) WIP, `Env` reward stub / missing `PolicyProvider`, GPU sidecars under `checkpoints/gpu_train_orchestrator/progress.json`, ClickHouse `trolly.ticks`, `latest.fingerprint.json`, failing tests). Then pick the highest-leverage slice that fits working hours and local constraints (no sudo ROCm; do not replace the weekday GPU trainer — it already trains; you may *direct* the gap it should close).
- **Continue training locally:** when the user asks to continue training locally (or this daily run is on the gym/policy path), **fetch first** (`git fetch origin` then `git rebase @{u}` if the tree is clean; otherwise fetch + warn and continue — see `crates/trolly-gym/scripts/continue-training-locally.sh` and the weekday systemd unit). Then run the local loop: ensure ClickHouse (`crates/trolly-gym/clickhouse/docker-compose.yml` or `TROLLY_CLICKHOUSE_URL`), ingest sim ticks into `trolly.ticks`, train a time-boxed WoLF-PPO slice (`gpu_train_orchestrator --continue-local`, or `--once` inside Mon–Fri 09:00–17:00), write `latest.safetensors` plus `latest.fingerprint.json` (weights / data-window / config SHA-256), and record the increment in `progress.json`. Do not skip the train slice.
- Build the **ready set**: items with `status: todo` and all `depends_on` entries `done`.
- If the ready set is **empty**, author the next free `WP-XXX` that unblocks the broad goal (prefer: stream `Env` reward + `PolicyProvider`; gym→strategy→exec join; demo place-order reconcile; then ONNX/`ort` inference). Do not invent chores (docs-only, third venue, drive-by refactors) unless they unblock the loop. Mark it `todo`, then schedule it in the same run.
- Schedule up to `max_parallel` items per wave with **disjoint** `scope` paths.
- Mark selected items `in_progress` before spawning workers; only the orchestrator sets `done` or `blocked` after acceptance checks.
- Workers must not change item status; return the structured payload from the automation prompt.
- On completion: set `last_run`, append a `+` line to [`changelog.md`](changelog.md) **change log** that names the increment (WP moved, checkpoint/`progress.json` improved, or a blocking test fixed), trim matching **WIP** bullets there.
- **Patched dependencies** (`patches/lob`, root `[patch]` in [`Cargo.toml`](Cargo.toml)): whenever scope touches a patched crate or submodule:
  1. **Comment in** — uncomment the `[patch."https://github.com/CAGS295/lob.git"]` block so `lob = { path = "./patches/lob" }` is active before `cargo test` / `cargo check`.
  2. **Submodule commit** — commit and push lob changes on **`patches/lob` `main`**, then bump the submodule pointer in trolly (`git add patches/lob`).
  3. **Comment out** — re-comment the `[patch]` block before closing the item (default branch builds against git `lob`; patch-in is for local/submodule development only).
  4. **Commit in trolly** — include `Cargo.toml` (patch commented out) and `patches/lob` pointer in the same commit on `ship_branch` as the work item; do not leave submodule bumps or patch toggles unstaged.
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
- notes: Binance spot at `providers::depth::binance::spot`; `other` venue scaffold registered. See `src/providers/.todo` for remaining migrations.

### WP-004 — Intra-provider overlays (Binance RPI)

- status: done
- repos: trolly
- depends_on: [WP-003]
- scope: src/providers/binance/usd_m.rs, src/bin/aggregated_depth_tui.rs, src/monitor/global_book.rs
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
- notes: WP-011 landed the scaffold with an optional `torch`/`tch` gate. This WP is research and architecture — pick stacks before committing to a training loop, GPU CI, or production inference path. Follow-on implementation WPs for WoLF-PPO are **WP-018–WP-020** (assumes primary stack `tch`/libtorch.rs unless this ADR chooses otherwise).

### WP-017 — Binance demo integration tests (spot + USDM)

- status: done
- repos: trolly
- depends_on: [WP-002, WP-008, WP-009]
- scope: .env.example, tests/, crates/binance-spot-exec/, crates/binance-usdm-exec/, README.md
- acceptance:
  - extend [`.env.example`](.env.example) with demo credentials and opt-in flags (pattern matches WP-002): at minimum `DEMO_BINANCE_KEY`, `DEMO_BINANCE_SECRET`, optional `TROLLY_DEMO_SYMBOL` (default `BTCUSDT`); document `cp .env.example .env` — this file is the repo env sample (no separate `.env.sample`)
  - document demo base URLs in README and/or test module docs:
    - **Spot demo** — [Spot Demo general info](https://developers.binance.com/docs/binance-spot-api-docs/demo-mode/general-info): REST `https://demo-api.binance.com/api`, WS API `wss://demo-ws-api.binance.com/ws-api/v3`, market streams `wss://demo-stream.binance.com/ws` (map from production hosts in [`src/providers/depth/binance/spot.rs`](src/providers/depth/binance/spot.rs) and [`crates/binance-spot-exec`](crates/binance-spot-exec))
    - **USDM demo** — [Derivatives docs](https://developers.binance.com/docs/derivatives/): REST `https://demo-fapi.binance.com`, market streams `wss://fstream.binancefuture.com` per [USDM general info](https://developers.binance.com/docs/derivatives/usds-margined-futures/general-info); user-data via `POST /fapi/v1/listenKey` on demo REST + private WS per [`crates/binance-usdm-exec`](crates/binance-usdm-exec)
  - `#[ignore]` integration tests (require `--ignored` and demo keys in `.env`):
    - spot: demo REST depth snapshot + signed user-data subscribe on demo WS API; assert parsed `executionReport` / account events when demo account activity exists (or skip with clear message if idle)
    - USDM: demo REST depth + listenKey lifecycle on `demo-fapi.binance.com` + user-data stream; assert `ORDER_TRADE_UPDATE` / `ACCOUNT_UPDATE` parsing against live demo payloads when available
  - default `cargo test --workspace` stays offline; demo tests skip cleanly when keys missing
  - optional follow-on (after WP-014 / WP-015): demo order place → user-stream reconcile round-trip for spot and USDM — document as sub-checklist in test module, not blocking this WP
- notes: uses Binance **demo/testnet** endpoints only — never production keys. Complements WP-002 (public REST merge); this WP adds authenticated streams and venue-specific demo host wiring. Geo/network restrictions may skip in CI; verify on unrestricted egress like WP-002.

### WP-018 — WoLF-PPO core algorithm (`trolly-gym`)

- status: done
- repos: trolly
- depends_on: [WP-011, WP-016]
- scope: crates/trolly-gym/src/ppo/, crates/trolly-gym/src/libtorch.rs, crates/trolly-gym/README.md
- acceptance:
  - implement **PPO** clipped surrogate objective (Eq. 1: `L^CLIP`, value loss `L^VF`, entropy bonus `S`) behind the existing `torch` feature using `tch`
  - implement **WoLF-PPO** extension per [paper_176](https://ieee-cog.org/2019/papers/paper_176.pdf): rolling **average payoff** as estimated NES payoff; dual learning rates `α_WIN` and `α_LOSE` with `α_WIN = α_LOSE / 4`; select `α_WIN` when current expected payoff exceeds the estimate, else `α_LOSE`
  - actor–critic MLP policy head (stochastic categorical actions) + value head; default hidden layers `[20, 20]` matching paper matrix-game experiments; SGD optimizer as default (Adam optional, documented)
  - configurable hyperparameters: clip ε, entropy coef `c2`, value coef `c1`, PPO epochs per rollout, `α_LOSE`
  - public API surface: e.g. `PpoConfig`, `WolfPpoConfig`, `ActorCritic`, `WolfPpoTrainer::policy_update` (or equivalent) usable from offline harness and later stream `Env`
  - CPU unit tests with `--features torch`: forward-pass shapes, loss computes without NaN on synthetic batch, WoLF LR switches on payoff vs estimate
  - default `cargo test -p trolly-gym` unchanged (no libtorch); `cargo test -p trolly-gym --features torch` passes
  - README section documents WoLF-PPO rationale (NES convergence), hyperparameters, and paper citation
- notes: primary stack is `tch`/libtorch.rs per WP-011 scaffold; WP-016 ADR may adjust fallback only. Does not include full training driver or market `Env` wiring — see WP-019 / WP-020.
- worker (2026-06-20): `crates/trolly-gym/src/ppo/` — PpoConfig, WolfPpoConfig, ActorCritic MLP [20,20], PpoTrainer (L^CLIP − c1·L^VF + c2·S), WolfPpoTrainer (rolling NES payoff, α_WIN = α_LOSE/4). README WoLF-PPO section. Default tests pass without libtorch; `--features torch` requires LIBTORCH (see README).

### WP-019 — Matrix-game validation harness (WoLF-PPO paper reproduction)

- status: done
- repos: trolly
- depends_on: [WP-018]
- scope: crates/trolly-gym/src/games/, crates/trolly-gym/tests/matrix_games.rs, crates/trolly-gym/README.md
- acceptance:
  - offline two-player zero-sum matrix games from the paper: **Matching Pennies** (standard + weighted payoff Table IIa, NES `P(H)=0.4`) and **Rock–Paper–Scissors** (standard + weighted Table IIb, NES `P(R)=0.2`, `P(P)=0.4`)
  - self-play training loop driving WP-018 `PPO` and `WoLF-PPO` with shared experimental setup (50-run capability; CI may use fewer seeds)
  - metric: Euclidean distance of learned policy from known NES; report max distance over last 10 policy updates per run (paper Table I methodology)
  - smoke test (always runs offline): short seeded run proves WoLF-PPO training step completes and distance metric is finite
  - `#[ignore]` extended benchmark (optional): reproduce paper trend — WoLF-PPO closer to NES than PPO on **weighted** Matching Pennies at `α_LOSE ∈ {0.1, 0.01}`; document how to run locally
  - `cargo test -p trolly-gym` passes default; matrix-game tests that need `tch` gated behind `torch` feature
- notes: validates algorithm before stream latency and reward engineering. Weighted games are the critical regression case (NES ≠ max-entropy policy).
- worker (2026-06-20): `crates/trolly-gym/src/games/` — Matching Pennies + RPS (standard/weighted), NES distance metric, self-play trainer for PPO/WoLF-PPO; `tests/matrix_games.rs` smoke + `#[ignore]` benchmark. Detached `old_log_probs` in rollout batch for multi-epoch PPO stability.

### WP-020 — WoLF-PPO training loop and checkpoint I/O (`trolly-gym`)

- status: done
- repos: trolly
- depends_on: [WP-018, WP-019]
- scope: crates/trolly-gym/src/train/, crates/trolly-gym/src/replay.rs, crates/trolly-gym/README.md
- acceptance:
  - rollout collection API (on-policy trajectories: obs, action, log-prob, value, reward, done) compatible with WP-018 update step and existing [`ReplayBuffer`](crates/trolly-gym/src/replay.rs) layout or documented parallel buffer
  - `WolfPpoTrainer` (or equivalent) driver: collect rollouts → multi-epoch PPO/WoLF-PPO updates → log scalar metrics (policy loss, value loss, entropy, NES distance when available, active WoLF LR)
  - checkpoint save/load for actor–critic weights (file format documented; round-trip test restores forward pass outputs on CPU)
  - hook to feed rollouts from [`Env::ingest_event`](crates/trolly-gym/src/env.rs) / [`Env::step`](crates/trolly-gym/src/env.rs) (reward still stub ok) without requiring live Binance streams in CI
  - `cargo test -p trolly-gym --features torch` includes checkpoint round-trip and short end-to-end train loop test
- notes: inference hot-path integration with `trolly-strategy` egress and production reward shaping remain follow-on after WP-014 / WP-015 order placement.
- worker (2026-06-20): `crates/trolly-gym/src/train/` — RolloutCollector + GAE, WolfPpoTrainDriver with TrainMetrics, checkpoint save/load via VarStore (safetensors). Env step hook via StepOutput closure. README training/checkpoint sections.

### WP-021 — Liquid Neural Network policy head (`trolly-gym`)

- status: done
- repos: trolly
- depends_on: [WP-018, WP-019, WP-020, WP-022]
- scope: crates/trolly-gym/src/ (new LNN actor-critic module), crates/trolly-gym/tests/matrix_games.rs, crates/trolly-gym/tests/microstructure_train.rs, crates/trolly-gym/README.md
- acceptance:
  - implement a **Liquid Neural Network (LNN)** actor–critic as an alternative to the existing MLP policy head; keep the MLP-based model unchanged and selectable alongside LNN
  - shared training harness: both MLP and LNN variants train through the same WoLF-PPO / matrix-game self-play path (`run_wolf_ppo_self_play`, `WolfPpoTrainDriver`, checkpoint I/O) **and** the WP-022 microstructure benchmark (`run_microstructure_train_with_checkpoints`)
  - train MLP and LNN **in parallel** (separate runs / configs) on the matrix-game benchmark suite (Matching Pennies + RPS, standard + weighted) **and** the synthetic microstructure env
  - both variants must pass the **correctness game**: short smoke runs produce finite NES distances; extended benchmark trend (WoLF-PPO closer to NES than PPO on weighted Matching Pennies) holds for LNN or is documented with rationale if not
  - checkpoint save/load round-trip works for LNN weights (architecture metadata sidecar or equivalent)
  - `cargo test -p trolly-gym --features torch` includes LNN smoke tests; default `cargo test -p trolly-gym` unchanged
  - README documents LNN vs MLP trade-offs, selection API, and how to run parallel training
- notes: |
    LNN is exploratory — do not remove or replace the MLP model. Primary WoLF-PPO validation remains WP-019 matrix games; WP-022 microstructure validates the stream-shaped training pipeline with real rewards. Parallel training means independent experiment configs/seeds, not necessarily a single multi-GPU job.
    Worker/orchestrator (2026-07-07): added selectable `ActorCriticArchitecture::{Mlp,Liquid}`, fixed-step `LiquidActorCritic`, LNN checkpoint round-trip, train-driver smoke, matrix-game LNN correctness coverage across Matching Pennies + RPS, and ignored LNN trend benchmark. Updated stale torch train-loop integration test to current `ppo`/`train` APIs.
    Acceptance: `cargo test -p trolly-gym` passes. `cargo +stable test -p trolly-gym --features torch --locked` passes with `LIBTORCH_USE_PYTORCH=1`, `LIBTORCH_BYPASS_VERSION_CHECK=1`, `CXX=g++`, PyTorch 2.3.0, and `LD_LIBRARY_PATH` pointing at Python torch libs (VM default Cargo 1.83 lacks edition-2024 support for locked `time-core`; VM default `c++`/latest PyTorch were incompatible with `torch-sys 0.16.1`).

### WP-022 — Synthetic microstructure training benchmark (`trolly-gym`)

- status: done
- repos: trolly
- depends_on: [WP-020]
- scope: crates/trolly-gym/src/sim/, crates/trolly-gym/src/train/microstructure.rs, crates/trolly-gym/tests/microstructure_train.rs, crates/trolly-gym/examples/microstructure_train_snapshots.rs, crates/trolly-gym/README.md
- acceptance:
  - offline [`MicrostructureSim`](crates/trolly-gym/src/sim/microstructure.rs): latent mid random walk, fixed half-spread, unit position `{-1,0,1}` on hold/buy/sell, mark-to-market reward minus trade cost
  - observations use the same depth feature layout as stream [`Env`](crates/trolly-gym/src/env.rs) (`features_from_event` / 7 features per frame)
  - episodic `done` at configurable horizon; deterministic seed for tests
  - [`run_microstructure_train_with_checkpoints`](crates/trolly-gym/src/train/microstructure.rs) drives `WolfPpoTrainDriver` and saves safetensors after each update
  - default `cargo test -p trolly-gym` includes sim unit tests (no libtorch); `cargo test -p trolly-gym --features torch --test microstructure_train` passes
  - README documents microstructure vs matrix-game roles; example binary for timed checkpoint runs
- notes: bridges WP-019 (algorithm correctness) and stream-backed trading. Complements matrix games — not a replacement for WoLF-PPO NES validation. CartPole intentionally skipped in favour of stream-shaped obs.
- worker (2026-07-06): `sim/microstructure`, `train/microstructure`, tests + example.

### WP-023 — Stream Env PolicyProvider and market reward (`trolly-gym`)

- status: done
- repos: trolly
- depends_on: [WP-014, WP-015, WP-020, WP-022]
- scope: crates/trolly-gym/src/env.rs, crates/trolly-gym/src/policy.rs, crates/trolly-gym/src/action.rs, crates/trolly-gym/tests/, crates/trolly-gym/README.md
- acceptance:
  - `trait PolicyProvider { fn act(&self, obs: &[f32]) -> Action; }` with `HoldPolicy` default
  - `Env::step` can use an injected provider; `Action::dispatch` remains the single egress path (no parallel order builder)
  - replace `reward_stub` with a configurable reward aligned with `MicrostructureSim` (inventory × Δmid − spread); episode `done` at a configurable horizon
  - offline test: inject synthetic stream events → policy returns Buy/Sell/Hold → `RecordingEgress` sees the same outbound messages `Action::dispatch` already produces
  - optional `--features torch`: load `latest.safetensors` from a microstructure checkpoint dir into a `CheckpointPolicy` and act once on a dummy obs (skip cleanly without libtorch)
  - `cargo test -p trolly-gym` passes; default workspace check still has no libtorch
- notes: Highest-leverage gap on the broad goal. Do not add ONNX/`ort` here. GPU host may lack sudo ROCm — CPU checkpoints are enough. Advertise/fulfill: `PolicyProvider::act` must return `Action` values that `Action::dispatch` already fulfills.
- worker (2026-08-16): added `PolicyProvider`, `HoldPolicy`, torch-gated `CheckpointPolicy`, provider-backed `Env::step`, configurable inventory × Δmid minus spread-cost reward, episode horizon, and offline dispatch smoke coverage. Acceptance: `cargo +stable test -p trolly-gym --locked` passes; `cargo +stable test --workspace --locked` passes after installing `protobuf-compiler` for `lob` build.rs.

### WP-024 — Demo place-order → user-stream reconcile

- status: done
- repos: trolly
- depends_on: [WP-014, WP-015, WP-017]
- scope: tests/, crates/binance-spot-exec/, crates/binance-usdm-exec/, .env.example
- acceptance:
  - `#[ignore]` demo tests: place a tiny order on spot demo and USDM demo, then assert the matching user-data fill/reject arrives on the existing `executionReport` / `ORDER_TRADE_UPDATE` path
  - no duplicate state machines; reconcile through current bookkeeping
  - skip cleanly without keys; never production hosts or keys
  - `cargo test --workspace` stays offline
- notes: Listed as optional follow-on on WP-017. Unblocks trusting demo execution before a trained policy is allowed to place. Disjoint from WP-023 (exec/tests vs gym).
- worker (2026-08-16): added guarded ignored spot/USDM demo market-order reconciliation tests requiring demo keys plus `RUN_BINANCE_DEMO_ORDERS=1`; user-stream frames flow through existing exec bookkeeping and terminal orders are cleared there. Acceptance: `cargo +stable test --no-default-features --test binance_demo` and `cargo +stable test --workspace --locked` pass.

### WP-025 — Checkpoint policy strategy execution harness

- status: done
- repos: trolly
- depends_on: [WP-023, WP-024]
- scope: crates/trolly-gym/src/policy.rs, crates/trolly-gym/src/env.rs, crates/trolly-gym/examples/, crates/trolly-gym/tests/, crates/trolly-gym/README.md
- acceptance:
  - example or test harness loads a saved `latest.safetensors` checkpoint through `CheckpointPolicy` when `--features torch` is enabled, feeds injected stream observations into `Env`, and dispatches through `Action::dispatch`
  - default `HoldPolicy` / injected policy path remains available without libtorch and without new default runtime dependencies
  - offline test proves checkpoint-or-hold policy consumes a synthetic depth stream and emits the same normalized `OutboundMessage::OrderRequest` values strategy exec adapters consume
  - README documents how to run the harness with a microstructure checkpoint dir and how to point it at demo execution only after WP-024 keys/guards are enabled
  - `cargo test -p trolly-gym` passes; `cargo test -p trolly-gym --features torch` includes checkpoint load/act smoke when libtorch is available
- notes: Next bridge after WP-023/WP-024: turn saved policy checkpoints into a reproducible injected-stream action loop before adding ONNX/`ort` or live automation. Keep the harness offline by default and route all order intents through existing strategy `OutboundMessage`/exec adapters.
- worker (2026-08-16): added `CheckpointOrHoldPolicy`, `run_offline_policy_harness`, a synthetic stream example, default hold/injected-policy tests, and torch-gated checkpoint harness coverage. Acceptance: `cargo +stable test -p trolly-gym --locked`, `cargo +stable run -p trolly-gym --example checkpoint_policy_harness --locked`, `cargo +stable test -p trolly-gym --features torch --lib --locked`, and `cargo +stable test --workspace --locked` pass.

### WP-026 — Order-only egress bridge for policy harness

- status: done
- repos: trolly
- depends_on: [WP-025]
- scope: crates/trolly-strategy/src/egress.rs, crates/trolly-strategy/src/lib.rs, tests/policy_execution_bridge.rs, crates/trolly-gym/README.md
- acceptance:
  - add a reusable order-only `StreamEgress` bridge/filter that forwards `OutboundMessage::OrderRequest` to an inner egress and ignores non-order policy side effects such as `Hold`/`Subscribe`
  - offline integration test feeds injected depth observations through `run_offline_policy_harness` with a Hold/Buy/Sell policy and proves Buy/Sell enqueue spot and USDM place-order requests through the existing exec egress adapters
  - no live network, keys, or production hosts are used; demo order placement remains behind the existing `RUN_BINANCE_DEMO_ORDERS=1` guard
  - README documents the bridge as the safe handoff from checkpoint policy harness output to demo execution adapters
  - `cargo test -p trolly-strategy` and `cargo test --test policy_execution_bridge --locked` pass
- notes: Closes the adapter seam left by WP-025 before a checkpoint policy is allowed near demo execution: `Action::dispatch` still emits the messages, but execution adapters should only receive real order intents.
- worker (2026-08-19): added `OrderOnlyEgress`, exported it from `trolly-strategy`, covered spot/USDM queue adapters with an offline policy harness integration test, and documented the safe demo handoff. Acceptance: `cargo test -p trolly-strategy` and `cargo test --test policy_execution_bridge --locked` pass.

### WP-027 — ONNX Runtime policy provider (`trolly-gym`)

- status: done
- repos: trolly
- depends_on: [WP-025, WP-026]
- scope: crates/trolly-gym/Cargo.toml, crates/trolly-gym/src/lib.rs, crates/trolly-gym/src/policy.rs, crates/trolly-gym/src/onnx.rs, crates/trolly-gym/examples/checkpoint_policy_harness.rs, crates/trolly-gym/tests/, crates/trolly-gym/README.md
- acceptance:
  - add an optional `ort` feature with an ONNX-backed `PolicyProvider` that loads a static actor model, accepts the existing flat `&[f32]` observation slice, and returns `Action::{Hold,Buy,Sell}` by argmax without requiring libtorch
  - default builds and the safe hold fallback remain unchanged: no ONNX Runtime dependency in `cargo test -p trolly-gym` unless `--features ort` is requested
  - checkpoint/offline policy harness can choose ONNX via an explicit model path environment variable while preserving the existing torch `latest.safetensors` path and hold fallback
  - feature-gated tests cover action decoding, missing/invalid model fallback or error reporting, and one offline harness step with an ONNX provider or deterministic test double; tests skip cleanly if the native ORT runtime cannot be loaded in this environment
  - README documents how to export/use a microstructure policy as ONNX, how to run the harness with `--features ort`, and that demo order placement still requires the WP-024 guards plus `OrderOnlyEgress`
- notes: This is the live-inference follow-on called out by WP-016/WP-025. Keep training on the existing torch/GPU path; ONNX is inference-only and must not broaden default build requirements.
- worker/orchestrator (2026-08-20): added optional `ort` feature with dynamically loaded ONNX Runtime, `OnnxPolicy`, `CheckpointOrHoldPolicy::Onnx`, `ONNX_MODEL_PATH` support in the offline harness, feature-gated ONNX tests, and README export/run docs. Acceptance: `cargo test -p trolly-gym --locked` and `cargo test -p trolly-gym --features ort --locked` pass.

### WP-028 — Guarded demo policy execution runner

- status: done
- repos: trolly
- depends_on: [WP-024, WP-026, WP-027]
- scope: Cargo.toml, src/cli/mod.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - root CLI exposes an explicit `execute policy-demo` runner that feeds injected policy observations into `Env`, wraps spot/USDM exec adapters in `OrderOnlyEgress`, and defaults to dry-run order request logging
  - optional `--execute-demo-orders` path refuses to place unless `RUN_BINANCE_DEMO_ORDERS=1` and demo credentials are present; it targets demo REST bases only
  - ONNX and torch policy loading remain opt-in via root feature passthroughs to `trolly-gym`; default builds use the hold fallback and add no default model runtime
  - offline tests cover dry-run spot and USDM request generation plus the demo-order guard without live network or keys
  - `cargo test --test policy_demo_runner --locked` and `cargo test --workspace --locked` pass
- notes: Bridges WP-027 inference and WP-024 demo execution into a single guarded command path. Keep production hosts unreachable from this runner.
- worker/orchestrator (2026-08-21): added root `execute policy-demo` dry-run runner, root `gym-ort` / `gym-torch` feature passthroughs, guarded demo REST placement path, offline spot/USDM request-generation tests, and docs. Acceptance: `cargo +stable test --test policy_demo_runner --locked` and `cargo +stable test --workspace --locked` pass after installing `protobuf-compiler` for `lob` build.rs.

### WP-029 — Policy demo placement receipts

- status: done
- repos: trolly
- depends_on: [WP-028]
- scope: src/policy_demo.rs, src/cli/mod.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - `execute policy-demo` assigns deterministic demo `newClientOrderId` values before guarded placement so user-stream reconciliation can match REST acks to policy-generated orders
  - `PolicyDemoReport` exposes typed spot/USDM placement receipts (order id, client order id, status, side, symbol) instead of only a count
  - dry-run behavior remains offline and safe; actual REST placement still requires both `--execute-demo-orders` and `RUN_BINANCE_DEMO_ORDERS=1`
  - offline tests cover spot and USDM receipt reporting through a mock placement path; no live network or credentials required
  - `cargo +stable test --test policy_demo_runner --locked` and `cargo +stable test --workspace --locked` pass
- notes: Completes the report seam left by WP-028. This does not subscribe to live user streams yet; it makes the guarded runner preserve the identifiers needed for the WP-024 reconciliation path.
- worker/orchestrator (2026-08-22): added deterministic demo client order IDs, typed placement receipts on `PolicyDemoReport`, a `--client-order-id-prefix` CLI option, receipt printing, offline mock spot/USDM placement tests, and README docs. Acceptance: `cargo +stable test --test policy_demo_runner --locked` and `cargo +stable test --workspace --locked` pass after generating a local ignored `Cargo.lock` and installing `protobuf-compiler` for `lob` build.rs.

### WP-030 — Policy demo user-stream reconciliation report

- status: done
- repos: trolly
- depends_on: [WP-029]
- scope: src/policy_demo.rs, src/cli/mod.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - `PolicyDemoReport` exposes typed reconciliation rows matched from policy-generated placement receipts to spot `executionReport` / USDM `ORDER_TRADE_UPDATE` user-data events by order id or deterministic client order id
  - reconciliation fans raw user-data frames through the existing `binance-spot-exec` / `binance-usdm-exec` ingest and bookkeeping paths; no duplicate parser or state machine
  - CLI can print reconciliation rows from an explicit captured user-data JSON file while default dry-run behavior remains offline and safe
  - offline tests cover spot and USDM mock placement plus matching terminal user-data frames; no live network or credentials required
  - `cargo +stable test --test policy_demo_runner --locked` and `cargo +stable test --workspace --locked` pass
- notes: Follow-on from WP-029. This closes the reporting seam between demo REST placement receipts and the WP-024 user-stream reconcile path before live/demo automation loops rely on policy-generated order IDs.
- worker/orchestrator (2026-08-23): added typed `PolicyDemoReconciliation` rows, captured JSON/NDJSON user-data frame reconciliation through existing spot/USDM ingest/bookkeeping paths, CLI `--reconcile-user-data-json` printing, offline mock spot/USDM reconciliation tests, and docs. Acceptance: `cargo +stable test --test policy_demo_runner --locked` and `cargo +stable test --workspace --locked` pass after installing `protobuf-compiler` for `lob` build.rs.

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
