# trolly

A streaming limit order book (LOB) monitor for cryptocurrency exchanges, written in Rust. Trolly connects to exchange WebSocket feeds, maintains real-time order book state using a lock-free concurrent data structure, and serves snapshots over gRPC and HTTP.

## How it works

```
Exchange WS feed ──► WebSocket stream ──► left-right LOB writer
                                                │
                REST snapshot (init) ───────────┘
                                                │
                          ┌─────────────────────┤
                          ▼                     ▼
                   gRPC endpoint       HTTP /scale/depth/:symbol
                  (protobuf LOB)          (binary + gzip)
```

1. On startup, a REST request fetches the initial depth snapshot for each symbol.
2. A WebSocket connection subscribes to real-time depth update streams.
3. Updates are applied to an in-process `LimitOrderBook` (from the [`lob`](https://github.com/CAGS295/lob) crate) through a [`left-right`](https://crates.io/crates/left-right) single-writer / multi-reader structure -- writers never block readers.
4. A server thread exposes the book to consumers via two optional APIs:
   - **gRPC** (`GetLimitOrderBook`) -- returns a protobuf-encoded snapshot for a given pair.
   - **HTTP codec** (`GET /scale/depth/:symbol`) -- returns a compact binary-encoded snapshot with gzip compression.

## Supported exchanges

| Exchange | CLI label | Status |
|----------|-----------|--------|
| Binance spot | `binance` | Implemented (`wss://stream.binance.com:9443`, REST depth API v3) |
| Binance USD-M futures | `binance-usd-m` | Implemented (optional `RPI:` symbol prefix for `@rpiDepth`) |
| Third venue (scaffold) | `other` | Parseable via `--sources`; no live WebSocket wired yet |

The provider abstraction (`Endpoints` trait) and [`REGISTERED_LABELS`](src/providers/depth/mod.rs) make it straightforward to add more exchanges. See [Adding a new exchange provider](#adding-a-new-exchange-provider) below.

## Building

Requires a **protobuf compiler** (`protoc`) for gRPC codegen:

```bash
# Debian/Ubuntu
sudo apt-get install protobuf-compiler

# macOS
brew install protobuf
```

Then build with Cargo:

```bash
cargo build --release
```

### Feature flags

| Feature | Default | Description |
|---------|---------|-------------|
| `grpc`  | yes     | gRPC server and protobuf types via tonic/prost |
| `codec` | yes     | Binary LOB encoding served over HTTP |

Build without defaults if you only need one transport:

```bash
cargo build --release --no-default-features --features grpc
```

## Usage

```bash
cargo run --release -- monitor depth \
    --provider binance \
    --symbols btcusdt ethusdt \
    --server-port 50051
```

| Flag | Description |
|------|-------------|
| `--provider` | Exchange provider (`binance`) |
| `--symbols`  | One or more lowercase trading pairs |
| `--server-port` | Port for the API server (default: `50051`, binds to `[::1]`) |
| `--enable-telemetry` | Enable OpenTelemetry tracing export to Jaeger |

### Querying the book

**gRPC** (see `examples/depth_sampler_grpc.rs`):

```bash
cargo run --example depth_sampler_grpc
```

**HTTP codec** (see `examples/depth_sampler_scale.rs`):

```bash
cargo run --example depth_sampler_scale
```

Both examples connect to `[::1]:50051` and poll the book at random intervals.

## Docker

```bash
docker build -t trolly .
docker run trolly depth_monitor monitor depth --provider binance --symbols btcusdt
```

The multi-stage Alpine Dockerfile produces a minimal image containing only the `depth_monitor` binary and OpenSSL libraries.

## Adding a new exchange provider

Use this checklist when onboarding a real venue (replace the `other` scaffold or add a sibling module). The scaffold lives at [`src/providers/depth/other.rs`](src/providers/depth/other.rs); Binance spot and USD-M are the reference implementations.

### 1. Module under `src/providers/depth/`

- Add `depth::<exchange>::<market>.rs` (or `depth::<exchange>/mod.rs` + submodules) implementing [`Endpoints<Depth>`](src/providers/mod.rs).
- Implement `websocket_url`, `ws_subscriptions`, and `rest_api_url` for the venue’s depth feed.
- Export the type from [`src/providers/depth/mod.rs`](src/providers/depth/mod.rs) and re-export from [`src/providers/mod.rs`](src/providers/mod.rs) if it should be public.
- For intra-venue overlays (like Binance RPI), use a symbol prefix in `ws_subscriptions` / `DepthUpdate.event.symbol` so routing stays distinct from the canonical leg.

### 2. Register CLI labels

- Append the kebab-case label to [`REGISTERED_LABELS`](src/providers/depth/mod.rs) in the same order you expect `--sources` parsing tests to cover.
- Add a [`Provider`](src/monitor/mod.rs) enum variant.
- Extend [`Provider::from_label`](src/monitor/global_book.rs) and [`Provider::label`](src/monitor/global_book.rs) with the new label (and common aliases if needed).

### 3. Wire the global-book multiplexor

In [`run_global_book_stream`](src/monitor/global_book.rs), add a match arm that calls `MonitorMultiplexor::<GlobalBookShard, Depth>::stream` with your `Endpoints` type and `(hub, Provider::YourVenue)` context — mirror the `Provider::Binance` / `Provider::BinanceUsdM` arms. Until this arm exists, `--sources your-label:SYMBOL` parses but only logs a scaffold warning.

### 4. Tests

- **Unit:** assert the label appears in `REGISTERED_LABELS` and round-trips through `Provider::from_label` / `BookSource::parse` (see [`src/providers/mod.rs`](src/providers/mod.rs) and [`src/monitor/global_book.rs`](src/monitor/global_book.rs) unit tests).
- **Integration:** extend [`tests/global_book.rs`](tests/global_book.rs) with a `parse_book_sources` case that includes the new label alongside existing venues without breaking them.
- Run `cargo test` (and `cargo test --features tui` if you touch the aggregated TUI).

### 5. Backlog

Update [`src/providers/.todo`](src/providers/.todo) so completed migrations are checked off and only real remaining work stays open.

## Project structure

```
src/
├── bin/depth_monitor.rs    # Binary entrypoint (tracing, CLI dispatch)
├── cli/mod.rs              # clap CLI definition
├── connectors/
│   ├── handler.rs          # EventHandler trait (parse WS frames → updates)
│   └── multiplexor.rs      # Per-symbol routing of parsed updates
├── monitor/
│   ├── mod.rs              # Provider enum, Monitor trait
│   ├── depth.rs            # DepthConfig, provider selection, monitor loop
│   └── order_book.rs       # left-right OrderBook, Absorb impl for LOB
├── net/
│   ├── streaming.rs        # MultiSymbolStream (WS subscribe + event loop)
│   └── ws_adapter.rs       # TLS WebSocket connection helper
├── providers/
│   ├── mod.rs              # Endpoints trait, REGISTERED_LABELS re-export
│   ├── .todo               # Provider expansion backlog
│   └── depth/
│       ├── mod.rs          # REGISTERED_LABELS, venue modules
│       ├── binance/        # spot + usd_m (RPI optional)
│       └── other.rs        # third-venue scaffold
├── servers/
│   ├── mod.rs              # Hyper/Axum server, route registration
│   ├── grpc/               # tonic gRPC service (GetLimitOrderBook)
│   └── scale/              # HTTP binary-encoded LOB endpoint
├── signals.rs              # Ctrl+C graceful shutdown
└── lib.rs                  # Library root, re-exports
```

## Testing

Default `cargo test` is offline: unit and fixture tests only. The global order book integration
test in [`tests/global_book.rs`](tests/global_book.rs) includes a live Binance REST merge check
that is `#[ignore]` so CI and local runs without credentials never hit the network.

**Fixture tests (always run):**

```bash
git submodule update --init patches/lob
cd patches/lob && git checkout main   # stay on branch, not detached HEAD
cd ../..
cargo test --test global_book
```

`patches/lob` is developed in parallel with trolly. Commit lob changes on **`main`**, then
record the new tip in trolly (`git add patches/lob && git commit`).

**Live REST merge (opt-in, requires Binance HTTPS):**

```bash
cp .env.example .env
# edit .env: RUN_GLOBAL_BOOK_INTEGRATION=1
cargo test --test global_book global_book_live_rest_merge -- --ignored
```

Optional: set `TROLLY_INTEGRATION_SYMBOL` in `.env` (default `BTCUSDT`). If the flag is missing or
`0`, the live test skips even when invoked with `--ignored`.

## Benchmarks

Criterion benchmarks for both serving paths:

```bash
cargo bench --bench depth_monitor         # gRPC round-trip
cargo bench --bench depth_monitor_scale   # HTTP codec round-trip
```

## Testing

### Default (no network)

Fixture and unit tests run on every `cargo test` with no `.env` and no live exchange calls:

```bash
cargo test
```

The global book integration test `global_book_live_rest_merge` is marked `#[ignore]`, so it is skipped unless you opt in explicitly.

### Global book live REST merge (opt-in)

To exercise the cross-source REST fetch → parse → merge path against Binance (or a local stub when Binance is geo-blocked):

1. Copy the env template: `cp .env.example .env`
2. Set `RUN_GLOBAL_BOOK_INTEGRATION=1` in `.env` (optionally change `TROLLY_INTEGRATION_SYMBOL`)
3. Run the ignored test:

```bash
cargo test --test global_book global_book_live_rest_merge -- --ignored --nocapture
```

If Binance REST returns HTTP 451 from your region, the test automatically falls back to a loopback REST stub so the merge pipeline is still verified. To force the stub:

```bash
TROLLY_INTEGRATION_USE_LOCAL_REST=1 cargo test --test global_book global_book_live_rest_merge -- --ignored --nocapture
```

See [`.env.example`](.env.example) for optional REST URL overrides.

## License

[GPL-3.0](LICENSE)
