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
- last_run: 2026-09-25
- max_parallel: 3
- ship_branch: integrate/orchestrator-branches

## Orchestrator notes

- **Git ship workflow (no PRs):** checkout `ship_branch` from Meta (`integrate/orchestrator-branches`). At run start, `git fetch origin` then `git rebase @{u}` (or `git pull --rebase`) when the working tree is clean so this host matches what cloud agents already pushed. If rebase would destroy uncommitted work, fetch, warn, and continue — never `reset --hard`, never `git rebase -i`, never `--no-verify`, never force-push. All commits from this run land on the ship branch. Push `ship_branch` to origin at end of run. Do **not** open pull requests, run `gh pr create`, or create `cursor/workplan-orchestrator-process-*` branches.
- **Daily progress (mandatory):** each run must leave a real increment toward the **Broad goal**. A health check, empty ready-set log, or “WP-001–WP-022 all done” line is a **failed run**. Before any local train slice, fetch-first as above so cloud and this host do not diverge. Assess current state vs the goal (`WORKPLAN.md` statuses, [`changelog.md`](changelog.md) WIP, `Env` reward stub / missing `PolicyProvider`, GPU sidecars under `checkpoints/gpu_train_orchestrator/progress.json`, ClickHouse `trolly.ticks`, `latest.fingerprint.json`, failing tests). Then pick the highest-leverage slice that fits working hours and local constraints (no sudo ROCm; do not replace the weekday GPU trainer — it already trains; you may *direct* the gap it should close).
- **Continue training locally:** when the user asks to continue training locally (or this daily run is on the gym/policy path), **fetch first** (`git fetch origin` then `git rebase @{u}` if the tree is clean; otherwise fetch + warn and continue — see `crates/trolly-gym/scripts/continue-training-locally.sh` and the weekday systemd unit). Then run the local loop: ensure ClickHouse (`crates/trolly-gym/clickhouse/docker-compose.yml` or `TROLLY_CLICKHOUSE_URL`), ingest sim ticks into `trolly.ticks`, train a time-boxed WoLF-PPO slice (`gpu_train_orchestrator --continue-local`, or `--once` inside Mon–Fri 09:00–17:00), write `latest.safetensors` plus `latest.fingerprint.json` (weights / data-window / config SHA-256), and record the increment in `progress.json`. Do not skip the train slice.
- Build the **ready set**: items with `status: todo` and all `depends_on` entries `done`.
- If the ready set is **empty**, author the next free `WP-XXX` that unblocks the broad goal (prefer: stream-shaped microstructure that can actually improve — WP-032–WP-034; gym→strategy→exec join; demo place-order reconcile; then ONNX/`ort` inference). Do not invent chores (docs-only, third venue, drive-by refactors) unless they unblock the loop. Mark it `todo`, then schedule it in the same run.
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
- notes: bridges WP-019 (algorithm correctness) and stream-backed trading. Complements matrix games — not a replacement for WoLF-PPO NES validation. CartPole intentionally skipped in favour of stream-shaped obs. Successor that replaces the flat-fee unit-lot MDP is **WP-032** (do not reopen this item).
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

### WP-031 — Live demo policy reconciliation listener

- status: done
- repos: trolly
- depends_on: [WP-030]
- scope: src/policy_demo.rs, src/cli/mod.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - `execute policy-demo` can optionally wait on the relevant Binance demo user-data stream after guarded demo placement and populate `PolicyDemoReport.reconciliations` without requiring a captured JSON file
  - the live listener is explicit and safe: it only runs with `--execute-demo-orders`, `RUN_BINANCE_DEMO_ORDERS=1`, demo credentials, and a bounded timeout; dry-run and `--reconcile-user-data-json` paths remain offline/default-safe
  - spot uses demo WebSocket API signed user-data subscribe; USDM uses demo listenKey lifecycle/private stream; both fan raw frames through existing `binance-spot-exec` / `binance-usdm-exec` ingest and bookkeeping paths with no duplicate parser or state machine
  - offline tests cover option/guard behavior and mocked frame-source reconciliation for spot and USDM; no live network or credentials required
  - `cargo +stable test --test policy_demo_runner --locked` and `cargo +stable test --workspace --locked` pass
- notes: This is the next bridge after WP-030: captured files prove report wiring, but demo/live Binance trading needs the guarded runner to collect its own user-stream receipts for policy-generated order IDs.
- worker/orchestrator (2026-09-07): added explicit `--wait-for-user-data` live demo reconciliation with bounded timeout, spot signed demo WebSocket subscribe, USDM demo listenKey/private-stream lifecycle, shared exec-ingest reconciliation state, CLI wiring, docs, and offline mock frame-source tests. Acceptance: generated the ignored local `Cargo.lock`, installed `protobuf-compiler` for the `lob` build script, then `cargo +stable test --test policy_demo_runner --locked` and `cargo +stable test --workspace --locked` passed.

### WP-032 — Microstructure depth-ladder sim (`trolly-gym`)

- status: done
- repos: trolly
- depends_on: [WP-022]
- scope: crates/trolly-gym/src/sim/microstructure.rs, crates/trolly-gym/src/observation.rs, crates/trolly-gym/README.md
- acceptance:
  - replace flat `trade_cost` on `{Hold,Buy,Sell}` unit lots with a linear bid/ask **depth ladder**: offset at depth `v` is `α(v) = δ + λ v` on each side (`v` = cumulative size already taken on that side, not mid and not time)
  - trading cost of inventory `q` is the **integral over levels** (`∫ α(v) dv` / discrete rung sum); mid is used only to mark inventory (`r = q_new * Δs − Δcost`), not inside the cost
  - observations expose rungs, current `q`, and **level-indexed** `Δα` (do not break the stream `Env` 7-D depth extractor; use a parallel ladder frame if needed)
  - train episodes **resample seeds**; do not use a single 64-tick Hold tape as the only mid path
  - default `cargo test -p trolly-gym` covers cost integral / rung layout without libtorch; WP-022 discrete snap tests either remain as a compatibility path or are updated and documented
  - README documents ladder vs old unit-lot MDP; design notes live in the Cursor microstructure plan (inventory ladder, Gaussian policy, Liquid-on-rungs)
- notes: Successor to WP-022, not a rewrite of that item. Directs the weekday GPU trainer at a learnable cost structure `(δ, λ)`. Live `Action::{Hold,Buy,Sell}` / `PolicyProvider` stay discrete until a later quantize WP.
- worker/orchestrator (2026-09-08): trainer guidance missing (silent). Replaced flat `trade_cost` with `α(v)=δ+λv` walk integral; parallel `V×[v,α_ask,α_bid,Δα,q]` frame; 7-D stream extractor unchanged; episodes resample seeds; `unit_lot_compat()` / `λ=0` keeps WP-022 snap. Acceptance: `cargo +stable test -p trolly-gym` passes (50 lib + matrix/smoke).

### WP-033 — Gaussian inventory policy for ladder MDP (`trolly-gym`)

- status: done
- repos: trolly
- depends_on: [WP-020, WP-032]
- scope: crates/trolly-gym/src/ppo/, crates/trolly-gym/src/train/, crates/trolly-gym/src/bin/gpu_train_orchestrator.rs, crates/trolly-gym/README.md
- acceptance:
  - microstructure actor is a **tanh-Gaussian** on target inventory `a ∈ (-1,1)`; walking `q → a` pays the WP-032 level integral (not a constant fee)
  - on-policy rollouts store `action: f32` for this path; matrix self-play stays categorical 3-logit MLP/Liquid
  - first function approximator is a **Gaussian MLP** on the flattened ladder (prove the MDP); do not add a time-transformer in this WP
  - `gpu_train_orchestrator` microstructure job trains this policy; log held-out **mean-action** eval vs Hold (`a=0`), not last in-episode reward; mean-reverting `|q|` under planted `λ > 0`
  - new checkpoint dir (old 3-logit microstructure weights will not load). Host fossils live in `checkpoints/gpu_train_orchestrator/_retired_unit_lot_microstructure/` — do not resume them.
  - `cargo test -p trolly-gym --features torch` covers Gaussian log-prob shapes and a short ladder train/eval; default `cargo test -p trolly-gym` unchanged
- notes: WoLF-PPO stays the algorithm; only the policy class (Gaussian vs categorical) and FA heads change. Do not rewire live `PolicyProvider` / ONNX / Binance to floats here. Do not resume `_retired_unit_lot_microstructure`.
- worker/orchestrator (2026-09-09): trainer guidance missing (silent). Added `step_target` inventory walk, tanh-Gaussian MLP on flattened ladder, `action: f32` rollouts, `microstructure/gaussian_mlp` job with mean-action vs Hold eval. Live 3-way `Action` unchanged. Acceptance: `cargo +stable test -p trolly-gym --lib` 53 pass; `cargo +stable test -p trolly-gym --features torch --lib gaussian` 6 pass (CPU torch 2.7.0).

### WP-034 — Liquid rung-trajectory function approximator (`trolly-gym`)

- status: done
- repos: trolly
- depends_on: [WP-021, WP-033]
- scope: crates/trolly-gym/src/ppo/lnn_actor_critic.rs, crates/trolly-gym/src/ppo/, crates/trolly-gym/tests/, crates/trolly-gym/README.md
- acceptance:
  - microstructure Liquid consumes ladder obs as `[batch, V, F]` rungs along `v`: `x_k = [v_k, α_ask, α_bid, Δα, q]`
  - liquid cell is driven with **`x_k` each step** (`liquid_steps = V`); do not repeat one flattened `x`; do not persist `h` across env steps
  - optional liquid gate scaled by rung width `Δv` so the unroll tracks `∫ α(v) dv`
  - readout is Gaussian `μ, logσ, V` (same policy class as WP-033); matrix-game Liquid stays categorical on a flat vector
  - `cargo test -p trolly-gym --features torch` includes Liquid-on-rungs smoke; default `cargo test -p trolly-gym` unchanged
- notes: State transform so the Liquid FA integrates along the same axis as trading cost. Transformer-over-time and transformer-over-rungs stay out of this WP (rungs transformer only if MLP and Liquid-on-rungs fail to recover `λ`).
- worker/orchestrator (2026-09-09): `RungLiquidGaussian` unrolls `x_k` along `V` rungs with `Δv`-scaled gate; Gaussian readout; matrix Liquid unchanged. Trainer job writes `microstructure/gaussian_liquid/`. Acceptance: `cargo +stable test -p trolly-gym --features torch --lib gaussian` and `--lib rung_liquid` pass.

### WP-035 — Quantize Gaussian inventory onto `Action::dispatch`

- status: done
- repos: trolly
- depends_on: [WP-033]
- scope: crates/trolly-gym/src/action.rs, crates/trolly-gym/src/policy.rs, crates/trolly-gym/README.md
- acceptance:
  - `quantize_inventory(a, deadzone)` maps `a ∈ [-1, 1]` to `{Hold,Buy,Sell}` (`|a| ≤ deadzone` → Hold)
  - a `PolicyProvider` can wrap a target-inventory source and still call `Action::dispatch` (no parallel order builder)
  - default `cargo test -p trolly-gym` covers the mapping and dispatch smoke without libtorch
  - live ONNX / 3-logit `CheckpointPolicy` paths stay unchanged
- notes: Closes the gym→strategy seam after WP-033/WP-034. Full Gaussian checkpoint load into the demo runner can follow; this WP only guarantees the typed action join.
- worker/orchestrator (2026-09-09): added deadzone quantize + `QuantizeInventoryPolicy`; Buy/Sell still go through `Action::dispatch`. Default lib tests cover mapping and egress.

### WP-036 — Load Gaussian ladder checkpoint in policy-demo

- status: done
- repos: trolly
- depends_on: [WP-028, WP-033, WP-035]
- scope: src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/src/policy.rs, crates/trolly-gym/README.md
- acceptance:
  - `execute policy-demo` can load `microstructure/gaussian_mlp` (torch) or a recorded mean-action vector, quantize via WP-035, and emit the same `OutboundMessage::OrderRequest` values `Action::dispatch` already produces
  - default dry-run and Hold/ONNX paths stay unchanged; no production hosts
  - `cargo test --test policy_demo_runner` stays offline
- notes: Next join after WP-035. Do not resume `_retired_unit_lot_microstructure`. Do not train here.
- worker/orchestrator (2026-09-10): trainer guidance missing (silent). Added `RecordedMeanActionPolicy` and `execute policy-demo` load of `GAUSSIAN_MEAN_ACTIONS` / `GAUSSIAN_CHECKPOINT_DIR`; torch `GaussianCheckpointPolicy` loads `gaussian_mlp`/`gaussian_liquid` and quantizes via WP-035. Hold/ONNX unchanged. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 14 pass; `cargo +stable test -p trolly-gym --lib --locked` 59 pass.

### WP-037 — Stream depth → ladder observation for Gaussian policy-demo

- status: done
- repos: trolly
- depends_on: [WP-032, WP-036]
- scope: crates/trolly-gym/src/observation.rs, crates/trolly-gym/src/env.rs, src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - `Env` can expose a parallel WP-032 ladder frame from ingested `DepthUpdate` plus current inventory; the 7-D stream extractor stays unchanged
  - `execute policy-demo` Gaussian sources (`GAUSSIAN_MEAN_ACTIONS` / `GAUSSIAN_CHECKPOINT_DIR`) feed that `V×5` layout to `PolicyProvider::act`
  - Hold / ONNX / 3-logit `CheckpointPolicy` stay on 7-D stream windows
  - `cargo test --test policy_demo_runner` and `cargo test -p trolly-gym` stay offline
- notes: Closes the obs-layout gap left by WP-036 (Gaussian FA trained on `V×5`, demo Env still passed padded 7-D). Do not train. Do not resume `_retired_unit_lot_microstructure`.
- worker/orchestrator (2026-09-10): trainer guidance missing (silent). `EnvConfig::use_ladder_observation` feeds `V×5` from ingested depth half-spread (δ) + inventory; Gaussian policy-demo sources select it. Hold stays 7-D. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 16 pass; `cargo +stable test -p trolly-gym --lib --locked` 61 pass.

### WP-038 — ONNX Gaussian mean-action provider

- status: done
- repos: trolly
- depends_on: [WP-027, WP-035, WP-037]
- scope: crates/trolly-gym/src/onnx.rs, crates/trolly-gym/src/policy.rs, src/policy_demo.rs, crates/trolly-gym/README.md, tests/policy_demo_runner.rs
- acceptance:
  - optional `ort` path loads a static Gaussian μ head (`[1, V×5] → [1]` or `[1,1]`), quantizes via WP-035, and dispatches through `Action::dispatch`
  - default Hold / 3-logit ONNX / recorded mean-action paths stay unchanged; no libtorch in default tests
  - `cargo test -p trolly-gym` and `cargo test --test policy_demo_runner` stay offline
- notes: Lets `execute policy-demo` consume an exported `gaussian_mlp` actor without `--features gym-torch`. Do not train. Do not resume `_retired_unit_lot_microstructure`.
- worker/orchestrator (2026-09-11): trainer guidance missing (silent). Added `OnnxGaussianMeanPolicy` + `decode_gaussian_mean_output` (`[1]` / `[1,1]`), WP-035 quantize, `CheckpointOrHoldPolicy::from_onnx_gaussian_model`, and `ONNX_GAUSSIAN_MODEL_PATH` / `PolicyDemoConfig.onnx_gaussian_model_path` (3-logit `ONNX_MODEL_PATH` still wins). Gaussian ONNX selects the WP-032 ladder frame. Acceptance: `cargo +stable test -p trolly-gym --lib --locked` 64 pass; `cargo +stable test --test policy_demo_runner --locked` 18 pass.

### WP-039 — Export weekday Gaussian μ head to ONNX

- status: done
- repos: trolly
- depends_on: [WP-033, WP-038]
- scope: crates/trolly-gym/scripts/, crates/trolly-gym/README.md, crates/trolly-gym/src/onnx.rs, tests/policy_demo_runner.rs
- acceptance:
  - documented offline path writes a static `[1, V×5] → [1]` ONNX μ graph from `microstructure/gaussian_mlp` (or a recorded mean stand-in) that `OnnxGaussianMeanPolicy` / `ONNX_GAUSSIAN_MODEL_PATH` can load
  - default Hold / 3-logit ONNX / recorded mean-action paths stay unchanged; no libtorch in default tests
  - do not train; do not resume `_retired_unit_lot_microstructure`
  - `cargo test -p trolly-gym` and `cargo test --test policy_demo_runner` stay offline
- notes: Closes the remaining gym→demo gap after WP-038: weekday GPU checkpoints are safetensors, and policy-demo still needs an exported μ graph to run without `--features gym-torch`.
- worker/orchestrator (2026-09-14): trainer guidance missing (silent). Added `write_recorded_mean_mu_onnx` / `inspect_gaussian_mu_onnx` (always compiled), `scripts/export_gaussian_mu_onnx.py` (stand-in Gemm or torch export from `gaussian_mlp` `latest.safetensors`), and an offline policy-demo path that points at the written graph. Refuses `_retired_unit_lot_microstructure`. Acceptance: `cargo +stable test -p trolly-gym --lib --locked` 68 pass; `cargo +stable test --test policy_demo_runner --locked` 19 pass.

### WP-040 — Auto-load exported μ ONNX from gaussian checkpoint dir

- status: done
- repos: trolly
- depends_on: [WP-039]
- scope: src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - when `ONNX_GAUSSIAN_MODEL_PATH` is unset, `execute policy-demo` loads `mu.onnx` from `GAUSSIAN_CHECKPOINT_DIR` (or the weekday `microstructure/gaussian_mlp` dir) if that file exists
  - explicit `ONNX_GAUSSIAN_MODEL_PATH` still wins; Hold / 3-logit `ONNX_MODEL_PATH` / recorded mean-action paths stay unchanged
  - refuse `_retired_unit_lot_microstructure`; no production hosts; default tests stay offline and do not need libtorch
  - `cargo test --test policy_demo_runner` and `cargo test -p trolly-gym` stay offline
- notes: Join after WP-039. The export script writes `mu.onnx`; policy-demo should pick it up beside `latest.safetensors` so weekday artifacts do not require a second env var. Do not train.
- worker/orchestrator (2026-09-14): trainer guidance missing (silent). `GAUSSIAN_CHECKPOINT_DIR/mu.onnx` and weekday `microstructure/gaussian_mlp/mu.onnx` auto-load after recorded mean-actions; explicit `ONNX_GAUSSIAN_MODEL_PATH` still wins; retired unit-lot still refused. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 22 pass.

### WP-041 — Policy-demo captured depth tape

- status: done
- repos: trolly
- depends_on: [WP-040]
- scope: src/policy_demo.rs, src/cli/mod.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - `execute policy-demo` can ingest captured depth JSON/NDJSON (same envelope style as `--reconcile-user-data-json`) into `Env` instead of only the synthetic fixture tape
  - Gaussian / Hold / 3-logit ONNX / recorded mean-action selection is unchanged; default fixture remains when the flag is unset
  - offline tests cover a captured depth frame through `Action::dispatch`; no live network or keys
  - `cargo test --test policy_demo_runner` stays offline
- notes: Next join after WP-040. Weekday μ graphs should act on injected demo/live book snapshots, not only the built-in synthetic depth. Do not train. Do not place live orders here.
- worker/orchestrator (2026-09-15): trainer guidance missing (silent). `--depth-json` feeds captured StreamEvent / Binance combined-stream / raw `depthUpdate` / REST snapshot frames into `Env`; `--max-steps` still caps; unset keeps the synthetic 100/101 tape. Recorded mean-actions still quantize onto `Action::dispatch`. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 28 pass.

### WP-042 — Policy-demo public depth source hook

- status: done
- repos: trolly
- depends_on: [WP-041]
- scope: src/policy_demo.rs, src/cli/mod.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - `execute policy-demo` can ingest public depth frames from an injectable in-process source (same parser as `--depth-json`) so a later live/demo WebSocket can reuse the hook
  - `--depth-json` and the synthetic fixture stay the default paths; policy selection (Hold / 3-logit ONNX / recorded mean-actions / Gaussian μ) is unchanged
  - offline tests push Binance-style frames through the hook into `Env` and through `Action::dispatch`; no live network or keys
  - `cargo test --test policy_demo_runner` stays offline
- notes: Next join after WP-041. Captured files prove injected books; the loop still needs a reusable public-depth source so demo streams are not a second parser. Do not train. Do not place live orders here.
- worker/orchestrator (2026-09-15): trainer guidance missing (silent). Added `run_policy_demo_with_public_depth` plus `public_depth_messages`; report prints `depth=synthetic|captured-json|injected`; `--depth-json` still wins. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 31 pass.

### WP-043 — Policy-demo live public depth subscribe

- status: done
- repos: trolly
- depends_on: [WP-042]
- scope: src/policy_demo.rs, src/cli/mod.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - `execute policy-demo --subscribe-public-depth` connects to Binance public depth (spot or USDM matching `--venue`) and feeds frames through the WP-042 hook into `Env`
  - `--depth-json` still wins; synthetic fixture remains the default; a bounded `--public-depth-timeout-secs` is required
  - offline tests mock the connector; default `cargo test --test policy_demo_runner` stays offline and needs no keys
  - policy selection unchanged; do not place live orders here; do not train
- notes: Next join after WP-042. Captured files and the injectable hook are in; demo/live books still need a public depth WebSocket (not ClickHouse ingest). Do not block on sudo ROCm.
- worker/orchestrator (2026-09-16): trainer guidance missing (silent). `--subscribe-public-depth` plus required `--public-depth-timeout-secs` collects demo public depth (spot `wss://demo-stream.binance.com/ws`, USDM `wss://fstream.binancefuture.com/stream`) through the WP-042 hook; report prints `depth=subscribed`; `--depth-json` still wins; offline tests mock WS texts and skip SUBSCRIBE acks. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 37 pass.

### WP-044 — Policy-demo public depth snapshot + diffs

- status: done
- repos: trolly
- depends_on: [WP-043]
- scope: src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - subscribed public depth can seed a local book from a demo REST-style snapshot, apply WS `depthUpdate` diffs (qty `0` removes a level; stale `u` skipped), and feed reconstructed top-of-book frames into `Env`
  - `--depth-json` still wins; synthetic fixture remains the default; live REST/WS stay behind `--subscribe-public-depth` and a bounded timeout
  - offline tests mock the snapshot and diffs; default `cargo test --test policy_demo_runner` stays offline and needs no keys
  - policy selection unchanged; do not place live orders here; do not train
- notes: Next join after WP-043. Raw `@depth` diffs are not snapshots; `Env` `features_from_event` reads `.first()` bid/ask. Rebuild the book so a saved policy sees a coherent demo/live top of book. Do not block on sudo ROCm.
- worker/orchestrator (2026-09-16): trainer guidance missing (silent). Local book from REST-style snapshot + WS diffs; qty `0` removes; stale `u` skipped; live subscribe fetches demo REST snapshot then rebuilds. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 40 pass.

### WP-045 — Policy-demo multi-symbol stream observations

- status: done
- repos: trolly
- depends_on: [WP-037, WP-044]
- scope: crates/trolly-gym/src/env.rs, crates/trolly-gym/src/observation.rs, src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - `Env` can ingest public depth for more than one symbol and expose a joined observation (per-symbol 7-D or ladder frames) that a `PolicyProvider` can act on
  - `execute policy-demo` accepts multiple symbols (CLI or config) and subscribes/injects public depth for each; `--depth-json` still wins; synthetic default remains single-symbol
  - Buy/Sell still dispatch through `Action::dispatch` (document per-symbol qty/side if needed); no live orders in this item
  - offline tests inject two symbols; default `cargo test --test policy_demo_runner` and `cargo test -p trolly-gym` stay offline
  - do not train; do not resume `_retired_unit_lot_microstructure`
- notes: Broad goal calls for multi-symbol `trolly-stream` observations. WP-043/044 are single-symbol demo books. This is the next gym→strategy join, not a third venue or docs-only chore.
- worker/orchestrator (2026-09-17): trainer guidance missing (silent). `--symbol BTCUSDT,ETHUSDT` joins latest 7-D or per-symbol `V×5` frames; extras are observation-only; Buy/Sell still `Action::dispatch` the primary pair; synthetic tape stays single-symbol; live subscribe sends every `{symbol}@depth`. Acceptance: `cargo +stable test -p trolly-gym --lib --locked` 71 pass; `cargo +stable test --test policy_demo_runner --locked` 43 pass.

### WP-046 — Primary-symbol Gaussian ladder on multi-symbol streams

- status: done
- repos: trolly
- depends_on: [WP-036, WP-045]
- scope: crates/trolly-gym/src/env.rs, src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - when a Gaussian source is selected and extra symbols are configured, `PolicyProvider::act` receives the primary-symbol WP-032 `V×5` ladder (weekday `mu.onnx` / `gaussian_mlp` dim), not the concatenated `N×V×5` join
  - extra symbols are still ingested; Hold / 3-logit stay on the joined 7-D frames
  - Buy/Sell still dispatch the primary symbol through `Action::dispatch`
  - offline tests; no live orders; do not train; do not resume `_retired_unit_lot_microstructure`
- notes: WP-045 joins both layouts. Weekday Gaussian artifacts are still `[1, V×5]`. This keeps load-and-dispatch working when `--symbol` lists more than one book.
- worker/orchestrator (2026-09-17): trainer guidance missing (silent). `EnvConfig.join_ladder_symbols = false` on Gaussian policy-demo so `act()` stays `V×5` on the dispatch symbol; extras still ingest. Report `symbol` stays the primary pair (user-stream reconcile). Acceptance: `cargo +stable test -p trolly-gym --lib --locked` 72 pass; `cargo +stable test --test policy_demo_runner --locked` 45 pass.

### WP-047 — Multi-symbol public-depth snapshot list

- status: done
- repos: trolly
- depends_on: [WP-044, WP-045]
- scope: src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - mocked / captured `public_depth_snapshot_json` can seed one local book per symbol (JSON array of REST-style snapshots, or one object as today)
  - WS diffs still route by `s` / symbol; qty `0` removes; stale `u` skipped
  - `--depth-json` still wins; synthetic default remains single-symbol; no live orders; do not train
  - offline tests inject two snapshots plus diffs; `cargo test --test policy_demo_runner` stays offline
- notes: Live subscribe already fetches a REST snapshot per configured symbol. The mocked WP-044 field is still one JSON object, so two-symbol book rebuild cannot be tested or injected offline.
- worker/orchestrator (2026-09-17): trainer guidance missing (silent). `policy_demo_public_depth_snapshot_jsons` accepts one REST object or an array; diffs still route by symbol. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 47 pass.

### WP-048 — Per-symbol Action::dispatch from joined observations

- status: done
- repos: trolly
- depends_on: [WP-035, WP-045]
- scope: crates/trolly-gym/src/action.rs, crates/trolly-gym/src/env.rs, src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - a policy acting on joined multi-symbol observations can emit `Action::dispatch` for a non-primary tracked symbol (document how qty/side are chosen)
  - default remains primary-symbol dispatch so existing single-symbol and WP-046 Gaussian paths stay unchanged
  - offline tests; no live orders; do not train
- notes: WP-045/046 join extra books but still place only on the first `--symbol`. The broad goal's strategy→exec loop needs a typed path to act on more than the primary pair without inventing a third venue.
- worker/orchestrator (2026-09-18): trainer guidance missing (silent). `PolicyProvider::decide` / `Action::on_symbol` emit `Action::dispatch` for a tracked extra pair; qty stays `EnvConfig.default_qty` / `--qty`; side stays Buy/Sell/Hold; unknown names fall back to primary. `--dispatch-symbol` and `DispatchSymbolPolicy` pin when the policy omits a name. Gaussian `V×5` still acts and (unless pinned) dispatches the primary pair. Acceptance: `cargo +stable test -p trolly-gym --lib --locked` 77 pass; `cargo +stable test --test policy_demo_runner --locked` 49 pass.

### WP-049 — Policy-demo reconcile tracks dispatched order symbols

- status: done
- repos: trolly
- depends_on: [WP-030, WP-048]
- scope: src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - user-stream ingest / reconcile matching includes every symbol that appears on placed `OrderRequest`s, not only the primary `--symbol`
  - default single-symbol and primary-only Gaussian paths stay unchanged
  - offline tests inject a non-primary dispatch plus a matching user-data frame; `cargo test --test policy_demo_runner` stays offline
  - no live orders; do not train
- notes: WP-048 can `Action::dispatch` ETHUSDT while reconcile hubs still subscribe/match `report.symbol` (the first `--symbol`). The strategy→exec loop cannot confirm extra-symbol fills until reconcile follows the typed orders.
- worker/orchestrator (2026-09-18): trainer guidance missing (silent). Spot/USDM user-data hubs register primary, `--dispatch-symbol`, observation, receipt, and `OrderRequest` symbols so extra-pair `executionReport` / `ORDER_TRADE_UPDATE` frames route. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 51 pass.

### WP-050 — Per-symbol inventory when dispatching extra pairs

- status: done
- repos: trolly
- depends_on: [WP-023, WP-048]
- scope: crates/trolly-gym/src/env.rs, crates/trolly-gym/src/action.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - `Env` inventory / market reward for a Buy/Sell on a tracked extra symbol uses that book's mid/spread, not only the primary pair
  - default single-symbol and primary-only Gaussian paths stay unchanged
  - offline tests; no live orders; do not train
- notes: WP-048/049 can place and reconcile ETHUSDT while `RewardState` still tracks the first `--symbol`. The gym→strategy loop needs book-local inventory so a checkpoint acting on joined observations is scored on the pair it traded.
- worker/orchestrator (2026-09-18): trainer guidance missing (silent). Extra-pair Buy/Sell scores mid/spread on that book's window; `Env::position()` stays primary; `position_for` reads the traded slot. Acceptance: `cargo +stable test -p trolly-gym --lib --locked` 78 pass.

### WP-051 — Write extra-symbol fills back into Env inventory

- status: done
- repos: trolly
- depends_on: [WP-049, WP-050]
- scope: src/policy_demo.rs, crates/trolly-gym/src/env.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - a typed extra-symbol reconciliation row can update `Env::position_for` for that pair (not only the primary book)
  - default single-symbol reconcile/inventory paths stay unchanged
  - offline tests; no live orders; do not train
- notes: WP-049 confirms extra-pair user-data fills and WP-050 scores the dispatched book, but Env inventory is still stepped only from the policy action. Closing gym←exec needs the fill to land on the same per-symbol slot.
- worker/orchestrator (2026-09-21): trainer guidance missing (silent). `Env::apply_fill` / `apply_reconciliation_fill` write extra-pair FILLED rows into `position_for`; primary `Env::position` stays the policy-step path. `reconcile_policy_demo_report` seeds `extra_symbol_inventory`. Acceptance: `cargo +stable test -p trolly-gym --lib --locked` 81 pass; `cargo +stable test --test policy_demo_runner --locked` 51 pass.

### WP-052 — Extra-symbol fill inventory in the next observation

- status: done
- repos: trolly
- depends_on: [WP-037, WP-051]
- scope: crates/trolly-gym/src/env.rs, crates/trolly-gym/src/observation.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - after an extra-symbol fill updates `Env::position_for`, the next `PolicyProvider::act` observation uses that book's inventory (`q` on joined ladder frames)
  - primary-only and Gaussian `join_ladder_symbols = false` paths stay on primary `q`
  - stream 7-D extractor stays unchanged; offline tests; no live orders; do not train
- notes: WP-051 writes extra-pair fills into the inventory slot, but joined ladder frames still stamp primary `RewardState.position` on every book. The gym←exec→gym loop needs the next act() to see the filled pair.
- worker/orchestrator (2026-09-21): trainer guidance missing (silent). Joined ladder frames use `slot.position`; `apply_fill` refreshes `last_observation`. Gaussian `join_ladder_symbols=false` stays primary `q`. Acceptance: `cargo +stable test -p trolly-gym --lib --locked` 83 pass; `cargo +stable test --test policy_demo_runner --locked` 51 pass.

### WP-053 — Keep policy-demo Env through extra-symbol fill write-back

- status: done
- repos: trolly
- depends_on: [WP-051, WP-052]
- scope: src/policy_demo.rs, src/cli/mod.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - `execute policy-demo` can reconcile captured user-data (`--reconcile-user-data-json` / config) before the harness `Env` is dropped and apply extra-symbol FILLED rows to that same Env
  - default dry-run and primary-only reconcile paths stay unchanged
  - offline tests; no live orders; do not train
- notes: WP-051/052 prove fill → `position_for` → next `act()` on a constructed Env. The runner still drops the harness Env before captured JSON reconcile, so gym←exec is not the same object that stepped.
- worker/orchestrator (2026-09-21): trainer guidance missing (silent). `captured_user_data_json` reconciles inside the runner; `finish_policy_demo_report` applies extra-symbol FILLED rows to the harness Env. CLI `--reconcile-user-data-json` sets the config field. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 52 pass; `cargo +stable test -p trolly-gym --lib --locked` 83 pass.

### WP-054 — Dry-run captured user-data matches assigned client order ids

- status: done
- repos: trolly
- depends_on: [WP-053]
- scope: src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - captured user-data reconcile can match assigned `newClientOrderId` values when receipts are empty (dry-run, no `--execute-demo-orders`)
  - extra-symbol FILLED rows still update `Env::position_for` / `extra_symbol_inventory`
  - default placement+receipt matching stays unchanged; offline tests; no live orders
- notes: WP-053 still needs receipts (placement) before `ReconciliationState` has targets. Offline gym←exec should work from the deterministic client order ids the runner already assigns.
- worker/orchestrator (2026-09-21): trainer guidance missing (silent). Empty receipts fall back to assigned `newClientOrderId` targets. Dry-run extra-symbol captured FILLED rows write `extra_symbol_inventory`. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 53 pass; `cargo +stable test -p trolly-gym --lib --locked` 83 pass.

### WP-055 — Continue Env steps after extra-symbol fill write-back

- status: done
- repos: trolly
- depends_on: [WP-052, WP-054]
- scope: src/policy_demo.rs, crates/trolly-gym/src/env.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - after captured extra-symbol fills update the harness Env, a subsequent injected depth step uses that book's fill-backed `q` / `position_for`
  - default dry-run and primary-only paths stay unchanged
  - offline tests; no live orders; do not train
- notes: WP-051–054 write extra-pair fills into Env and the next isolated `act()`. The runner still ends the harness at reconcile, so a continued tape cannot consume the filled slot. Next gym←exec→gym join.
- worker/orchestrator (2026-09-22): trainer guidance missing (silent). `--continued-depth-json` / `continued_depth_messages` step the same harness Env after fill write-back; joined ladder `q` is fill-backed (`position_for`). Unset / primary-only stay unchanged. Acceptance: `cargo +stable test -p trolly-gym --lib --locked` 84 pass; `cargo +stable test --test policy_demo_runner --locked` 57 pass.

### WP-056 — Drain continued-tape Action::dispatch onto the report

- status: done
- repos: trolly
- depends_on: [WP-055]
- scope: src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - Buy/Sell emitted on the post-fill continued tape appear on `PolicyDemoReport.orders` with the next deterministic `newClientOrderId`
  - default dry-run and paths without continued depth stay unchanged; continued orders are not placed
  - offline tests; no live orders; do not train
- notes: WP-055 steps the continued tape through `Action::dispatch` but those OrderRequests were already drained before fill write-back. gym←exec→gym→strategy needs the post-fill orders on the same report.
- worker/orchestrator (2026-09-22): trainer guidance missing (silent). After continue, leftover spot/USDM channel orders are appended with `*-0002` ids. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 57 pass.

### WP-057 — Apply wait-for-user-data fills before the continued tape

- status: done
- repos: trolly
- depends_on: [WP-055, WP-031]
- scope: src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - mock `--wait-for-user-data` extra-symbol FILLED rows update the harness Env before `--continued-depth-json` steps
  - default captured-JSON and dry-run continue paths stay unchanged
  - offline tests; no live orders; do not train
- notes: WP-053/055 apply captured JSON fills on the live Env. The placer+user-data helper still reconciled after Env drop, so a continued tape could not see wait-path fills.
- worker/orchestrator (2026-09-22): trainer guidance missing (silent). Prepare/place, then mock user-data reconcile, then finish+continue on the same Env. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 57 pass.

### WP-058 — Guarded placement of continued-tape orders

- status: done
- repos: trolly
- depends_on: [WP-056, WP-028]
- scope: src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - when `--execute-demo-orders` is set, OrderRequests drained from the post-fill continued tape can be placed through the same guarded spot/USDM adapters (offline mock placer is enough)
  - default dry-run records continued orders without placing; first-tape receipts stay unchanged
  - offline tests; no live network; do not train
- notes: WP-056 records continued-tape `Action::dispatch` on the report but only the pre-fill tape is placed. The gym←exec→gym→exec loop still drops the post-fill hop.
- worker/orchestrator (2026-09-23): trainer guidance missing (silent). After continue, leftover spot/USDM orders are placed through the same mock/live adapters; first-tape receipts stay first. Dry-run still records only. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 59 pass; `cargo +stable test -p trolly-gym --lib --locked` 84 pass.

### WP-059 — Reconcile continued-tape placement receipts

- status: done
- repos: trolly
- depends_on: [WP-058, WP-054]
- scope: src/policy_demo.rs, src/cli/mod.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - after continued-tape orders are assigned (dry-run) or placed, captured user-data can match those new receipts / `newClientOrderId`s
  - extra-symbol FILLED rows from that second hop update the same harness Env; first-tape reconcile rows stay on the report
  - default paths without `--continued-user-data-json` stay unchanged; offline tests; no live orders; do not train
- notes: WP-058 places the post-fill hop. First-tape captured/wait reconcile still runs before continue, so the gym←exec→gym→exec loop cannot confirm the continued receipts.
- worker/orchestrator (2026-09-23): trainer guidance missing (silent). `--continued-user-data-json` matches continued receipts / assigned ids after place; extra-symbol FILLED rows write the same Env; first-tape rows stay. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 61 pass.

### WP-060 — Wait-for-user-data after continued-tape placement

- status: done
- repos: trolly
- depends_on: [WP-057, WP-059]
- scope: src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - mock `--wait-for-user-data` can supply frames after continued-tape orders are placed so those receipts reconcile without `--continued-user-data-json`
  - first-tape wait still runs before the continued depth tape; dry-run / captured-JSON paths stay unchanged
  - offline tests; no live network; do not train
- notes: WP-057 waits only on first-tape receipts. WP-059 covers captured JSON after continue. Demo/live trust still needs the wait helper to see the post-fill hop.
- worker/orchestrator (2026-09-23): trainer guidance missing (silent). Mock wait callback is `FnMut` and runs again after continued place when extra orders exist; new rows merge by client order id. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 62 pass.

### WP-061 — Live user-data wait after continued-tape demo placement

- status: done
- repos: trolly
- depends_on: [WP-031, WP-060]
- scope: src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - when `--wait-for-user-data` and `--execute-demo-orders` are set, the live demo user-data listener can collect frames after continued-tape REST placement (offline mock socket/source is enough)
  - first-tape live wait still runs before the continued depth tape; dry-run / captured-JSON / mock-callback paths stay unchanged
  - offline tests; no production hosts; do not train
- notes: WP-060 covers the mock placer callback. The live spot/USDM sockets still wait only on first-tape receipts, so demo/live trust cannot confirm the post-fill hop.
- worker/orchestrator (2026-09-24): trainer guidance missing (silent). Live spot/USDM sockets stay open through continued REST place; second wait seeds first-tape rows and merges new receipts; USDM listenKey closes after the last wait. Offline mock source (`run_*_with_live_user_data_source`) drains one shared frame queue through the same wait helper. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 64 pass; `cargo +stable test -p trolly-gym --lib --locked` 84 pass.

### WP-062 — Continue Env after continued-tape live fills

- status: done
- repos: trolly
- depends_on: [WP-055, WP-061]
- scope: src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - after the second live/mock user-data wait writes continued-tape extra-symbol FILLED rows into the harness Env, a subsequent injected depth step uses that book's fill-backed `q` / `position_for`
  - default dry-run, first-tape continue, and paths without a second wait stay unchanged
  - offline tests; no live orders; do not train
- notes: WP-055 continues after first-tape fills. WP-061 confirms the post-fill hop on the live socket, but the runner still ends there, so the next `act()` cannot consume second-hop inventory.
- worker/orchestrator (2026-09-25): trainer guidance missing (silent). `--second-continued-depth-json` / `second_continued_depth_messages` step the same harness Env after continued-tape live/mock fills; report `second_continued_observation` carries fill-backed extra-symbol `q`. Unset / first-tape continue stay unchanged. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 67 pass; `cargo +stable test -p trolly-gym --lib --locked` 84 pass.

### WP-063 — Drain second-continued-tape Action::dispatch onto the report

- status: done
- repos: trolly
- depends_on: [WP-056, WP-062]
- scope: src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - Buy/Sell emitted on the post-second-fill tape appear on `PolicyDemoReport.orders` with the next deterministic `newClientOrderId`
  - default dry-run and paths without `--second-continued-depth-json` stay unchanged; second-continued orders are not placed
  - offline tests; no live orders; do not train
- notes: WP-062 steps the second-continued tape through `Action::dispatch` but those OrderRequests sit in the channel after the first-continue drain. gym←exec→gym→strategy needs the third-hop orders on the same report.
- worker/orchestrator (2026-09-25): trainer guidance missing (silent). After the second-continued tape, leftover spot/USDM channel orders append with the next `newClientOrderId` (`*-0003`). Dry-run and first-continue placement stay unchanged. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 69 pass; `cargo +stable test -p trolly-gym --lib --locked` 84 pass.

### WP-064 — Guarded placement of second-continued-tape orders

- status: done
- repos: trolly
- depends_on: [WP-058, WP-063]
- scope: src/policy_demo.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - when `--execute-demo-orders` is set, OrderRequests drained from the post-second-fill tape can be placed through the same guarded spot/USDM adapters (offline mock placer is enough)
  - default dry-run records second-continued orders without placing; first-tape and first-continue receipts stay first
  - offline tests; no live network; do not train
- notes: WP-063 records third-hop `Action::dispatch` on the report but only the first-continue tape is placed. The gym←exec→gym→exec loop still drops the second-continued hop.
- worker/orchestrator (2026-09-25): trainer guidance missing (silent). After the second-continued tape, leftover spot/USDM orders place through the same mock/live adapters; first-tape and first-continue receipts stay first. Dry-run still records only. Acceptance: `cargo +stable test --test policy_demo_runner --locked` 69 pass; `cargo +stable test -p trolly-gym --lib --locked` 84 pass.

### WP-065 — Reconcile second-continued-tape placement receipts

- status: todo
- repos: trolly
- depends_on: [WP-059, WP-064]
- scope: src/policy_demo.rs, src/cli/mod.rs, tests/policy_demo_runner.rs, crates/trolly-gym/README.md
- acceptance:
  - after second-continued-tape orders are assigned (dry-run) or placed, captured user-data can match those new receipts / `newClientOrderId`s
  - extra-symbol FILLED rows from that third hop update the same harness Env; first-tape and first-continue reconcile rows stay on the report
  - default paths without `--second-continued-user-data-json` stay unchanged; offline tests; no live orders; do not train
- notes: WP-064 places the third hop. First-continue captured/wait reconcile still runs before the second-continued tape, so the gym←exec→gym→exec loop cannot confirm the second-continued receipts.

## Integration test reference

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
