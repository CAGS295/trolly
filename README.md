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

| Exchange | Status |
|----------|--------|
| Binance  | Implemented (`wss://stream.binance.com:9443`, REST depth API v3) |

The provider abstraction (`Endpoints` trait) makes it straightforward to add more exchanges.

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
│   ├── mod.rs              # Endpoints trait (WS URL, REST URL, subscriptions)
│   └── binance.rs          # Binance implementation
├── servers/
│   ├── mod.rs              # Hyper/Axum server, route registration
│   ├── grpc/               # tonic gRPC service (GetLimitOrderBook)
│   └── scale/              # HTTP binary-encoded LOB endpoint
├── signals.rs              # Ctrl+C graceful shutdown
└── lib.rs                  # Library root, re-exports
```

## Integration tests

Global book tests live in [`tests/global_book.rs`](tests/global_book.rs). Fixture tests run on every `cargo test` and use in-memory JSON only — no network access.

The live REST merge test (`global_book_live_rest_merge`) is marked `#[ignore]` so default test runs never call Binance. To opt in:

```bash
cp .env.example .env
# Edit .env: set RUN_GLOBAL_BOOK_INTEGRATION=1
cargo test --test global_book global_book_live_rest_merge -- --ignored --nocapture
```

| Variable | Default | Purpose |
|----------|---------|---------|
| `RUN_GLOBAL_BOOK_INTEGRATION` | `0` | Set to `1` (or `true`) to allow live REST calls in the ignored test |
| `TROLLY_INTEGRATION_SYMBOL` | `BTCUSDT` | Trading pair for spot + USDM snapshot fetch |

If you run the ignored test without enabling the flag, it prints a skip message and passes without network I/O.

## Benchmarks

Criterion benchmarks for both serving paths:

```bash
cargo bench --bench depth_monitor         # gRPC round-trip
cargo bench --bench depth_monitor_scale   # HTTP codec round-trip
```

## License

[GPL-3.0](LICENSE)
