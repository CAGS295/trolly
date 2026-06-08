//! Terminal UI for [`trolly::monitor::GlobalBookHub`]: **cross-source** `MERGED·INSTRUMENT` tabs,
//! per [`BookSource::stream_id`] legs, and optional Binance-USDM **`Δ·INSTRUMENT`** (`@depth − @rpiDepth`).
//!
//! ```text
//! cargo run --features tui --bin aggregated_depth_tui -- \
//!   --sources binance:BTCUSDT,binance-usd-m:BTCUSDT
//! ```

use std::io::stdout;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style, Stylize};
use ratatui::symbols::Marker;
use ratatui::widgets::{
    Axis, Block, Borders, Chart, Dataset, GraphType, LegendPosition, Paragraph, Tabs,
};
use ratatui::{Frame, Terminal};
use std::collections::{HashMap, HashSet};

use left_right::ReadHandleFactory;
use lob::LimitOrderBook;
use regex::Regex;
use trolly::monitor::{
    parse_book_sources, run_global_book_stream, BookSource, GlobalBookHub,
};
use trolly::providers::RPI_PREFIX;

#[derive(Parser, Debug)]
#[command(about = "Global book TUI: MERGED·INSTRUMENT, per-source streams, optional Binance-USDM Δ tab")]
struct Args {
    #[arg(
        long,
        help = "Comma-separated book sources: provider:SYMBOL (e.g. binance:BTCUSDT,binance-usd-m:BTCUSDT)"
    )]
    sources: String,
    /// Max bid / ask price levels included in each cumulative curve (nearest the touch).
    #[arg(long, default_value_t = 48)]
    depth_rows: usize,
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let args = Args::parse();

    let book_sources = parse_book_sources(&args.sources)
        .map_err(|e| color_eyre::eyre::eyre!(e))?;
    let stream_order: Vec<String> = book_sources
        .iter()
        .map(BookSource::stream_id)
        .collect();

    let hub = GlobalBookHub::new();
    let hub_feed = hub.clone();
    let sources_feed = book_sources.clone();

    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(async move {
            run_global_book_stream(hub_feed, &sources_feed).await;
        });
    });

    run_tui(hub, args.depth_rows, stream_order)?;
    Ok(())
}

/// Instrument key for cross-source merge (strip known provider prefix from stream id).
fn canonical_from_stream_id(stream_id: &str) -> String {
    for prefix in ["binance-usd-m:", "binance:"] {
        if let Some(sym) = stream_id.strip_prefix(prefix) {
            return sym.to_uppercase();
        }
    }
    stream_id.to_uppercase()
}

/// Parse [`lob::LimitOrderBook`]'s `Display` output (`price:qty` tuples) into ladder rows.
fn parse_book_display(s: &str) -> Option<(u64, Vec<(String, String)>, Vec<(String, String)>)> {
    let s = s.trim();
    let (id_s, after_bids_kw) = s.split_once(", bids: ")?;
    let id_s = id_s.strip_prefix("update_id: ")?;
    let id: u64 = id_s.parse().ok()?;
    let (bids_inner, asks_inner) = split_bids_asks_sections(after_bids_kw.trim_start())?;
    Some((id, tuples_from_side(bids_inner), tuples_from_side(asks_inner)))
}

static LEVEL_RE: OnceLock<Regex> = OnceLock::new();

/// Pull `price:qty` pairs from a bids/asks bracket body (robust vs `Display` float formatting).
fn tuples_from_side(inner: &str) -> Vec<(String, String)> {
    let re = LEVEL_RE.get_or_init(|| {
        Regex::new(r"(?P<p>[0-9.eE+-]+):(?P<q>[0-9.eE+-]+)").expect("level regex")
    });
    re.captures_iter(inner)
        .filter_map(|c| {
            Some((
                c.name("p")?.as_str().to_string(),
                c.name("q")?.as_str().to_string(),
            ))
        })
        .collect()
}

/// `s` starts at the `[` that opens the bids list (`[p:q, ...], asks:[...]` or `[], asks:[]`).
fn split_bids_asks_sections(s: &str) -> Option<(&str, &str)> {
    if !s.starts_with('[') {
        return None;
    }
    let mut depth = 0u32;
    for (i, c) in s.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    let bids_inner = &s[1..i];
                    let tail = s.get(i + 1..)?.trim_start();
                    let tail = tail.strip_prefix(", asks:")?.trim_start();
                    let asks_inner = tail.strip_prefix('[')?.strip_suffix(']')?;
                    return Some((bids_inner, asks_inner));
                }
            }
            _ => {}
        }
    }
    None
}

/// Cheap fingerprint of the rendered book string (first `max` bytes). Same fp ⇒ same snapshot text.
fn fnv1a64_prefix(s: &str, max: usize) -> u64 {
    const OFFSET: u64 = 14695981039346656037;
    const PRIME: u64 = 1099511628211;
    let mut h = OFFSET;
    for b in s.bytes().take(max) {
        h ^= u64::from(b);
        h = h.wrapping_mul(PRIME);
    }
    h
}

fn parse_level_pairs(levels: &[(String, String)]) -> Vec<(f64, f64)> {
    levels
        .iter()
        .filter_map(|(ps, qs)| {
            let p = ps.parse::<f64>().ok()?;
            let q = qs.parse::<f64>().ok()?;
            (p.is_finite() && q.is_finite() && q > 0.0).then_some((p, q))
        })
        .collect()
}

/// Cumulative bid size from each price **inward** to the touch (top-`max_levels` bid prices).
fn bid_cumulative_top_n(levels: &[(String, String)], max_levels: usize) -> Vec<(f64, f64)> {
    let mut v = parse_level_pairs(levels);
    if v.is_empty() {
        return vec![];
    }
    v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    v.truncate(max_levels);
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut total: f64 = v.iter().map(|(_, q)| q).sum();
    let mut out = Vec::with_capacity(v.len());
    for (p, q) in &v {
        out.push((*p, total));
        total -= q;
    }
    out
}

/// Cumulative ask size from the touch **outward** (top-`max_levels` ask prices).
fn ask_cumulative_top_n(levels: &[(String, String)], max_levels: usize) -> Vec<(f64, f64)> {
    let mut v = parse_level_pairs(levels);
    if v.is_empty() {
        return vec![];
    }
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    v.truncate(max_levels);
    let mut cum = 0.0;
    let mut out = Vec::with_capacity(v.len());
    for (p, q) in v {
        cum += q;
        out.push((p, cum));
    }
    out
}

fn levels_qty_map(levels: &[(String, String)]) -> HashMap<String, f64> {
    let mut m = HashMap::new();
    for (p, qs) in levels {
        if let Ok(q) = qs.parse::<f64>() {
            if q.is_finite() {
                *m.entry(p.clone()).or_insert(0.0) += q;
            }
        }
    }
    m
}

/// Per-price `qty(right) − qty(left)` for union of price keys (RPI diff: left=rpi stream, right=public).
///
/// When `omit_zero_delta` is false, exact matches (`Δ=0`) are kept so the chart does not go blank
/// when both streams agree at every level. Output is capped for frame cost.
fn diff_qty_levels(
    left_side: &[(String, String)],
    right_side: &[(String, String)],
    omit_zero_delta: bool,
) -> Vec<(String, String)> {
    const MAX_LEVELS: usize = 2048;
    let bm = levels_qty_map(left_side);
    let sm = levels_qty_map(right_side);
    let keys: HashSet<String> = bm.keys().chain(sm.keys()).cloned().collect();
    let mut pairs: Vec<(f64, String, f64)> = Vec::new();
    for p in keys {
        let d = sm.get(&p).copied().unwrap_or(0.0) - bm.get(&p).copied().unwrap_or(0.0);
        if !d.is_finite() {
            continue;
        }
        if omit_zero_delta && d.abs() < 1e-12 {
            continue;
        }
        let Some(pf) = p.parse::<f64>().ok() else {
            continue;
        };
        if !pf.is_finite() {
            continue;
        }
        pairs.push((pf, p, d));
    }
    pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pairs.truncate(MAX_LEVELS);
    pairs
        .into_iter()
        .map(|(_, p, d)| (p, format!("{d}")))
        .collect()
}

fn parse_signed_level_pairs(levels: &[(String, String)]) -> Vec<(f64, f64)> {
    levels
        .iter()
        .filter_map(|(ps, qs)| {
            let p = ps.parse::<f64>().ok()?;
            let q = qs.parse::<f64>().ok()?;
            (p.is_finite() && q.is_finite()).then_some((p, q))
        })
        .collect()
}

/// Bid-side Δqty at each price, top-`max_levels` prices nearest the touch (highest bid prices).
fn bid_delta_line_top_n(levels: &[(String, String)], max_levels: usize) -> Vec<(f64, f64)> {
    let mut v = parse_signed_level_pairs(levels);
    if v.is_empty() {
        return vec![];
    }
    v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    v.truncate(max_levels);
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// Ask-side Δqty at each price, top-`max_levels` prices nearest the touch (lowest ask prices).
fn ask_delta_line_top_n(levels: &[(String, String)], max_levels: usize) -> Vec<(f64, f64)> {
    let mut v = parse_signed_level_pairs(levels);
    if v.is_empty() {
        return vec![];
    }
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    v.truncate(max_levels);
    v
}

fn widen_bounds(lo: f64, hi: f64) -> [f64; 2] {
    if !lo.is_finite() || !hi.is_finite() {
        return [0.0, 1.0];
    }
    if (hi - lo).abs() < 1e-12 {
        return [lo - 1.0, hi + 1.0];
    }
    let pad = (hi - lo) * 0.02;
    [lo - pad, hi + pad]
}

fn fmt_axis_tick(v: f64) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    if v != 0.0 && (v.abs() >= 1e7 || v.abs() < 1e-4) {
        format!("{v:.2e}")
    } else if v.abs() >= 1000.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.6}")
    }
}

fn snapshot_ladder(
    view: &View,
    factories_by_symbol: &HashMap<String, ReadHandleFactory<LimitOrderBook>>,
    hub: &GlobalBookHub,
) -> (u64, u64, Vec<(String, String)>, Vec<(String, String)>) {
    let factory = match view {
        View::MergedCanonical(canon) => {
            let Some(f) = hub.merged_factory_for(canon) else {
                return (0, 0, vec![], vec![]);
            };
            f
        }
        View::Symbol(sym) => {
            let sym = sym.to_uppercase();
            let Some(f) = factories_by_symbol
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&sym))
                .map(|(_, f)| f.clone())
            else {
                return (0, 0, vec![], vec![]);
            };
            f
        }
        View::Diff(_) => return (0, 0, vec![], vec![]),
    };
    let snap = factory.handle().enter().map(|g| {
        let book_id = g.update_id;
        let text = format!("{}", *g);
        let fp = fnv1a64_prefix(&text, 4096);
        (book_id, fp, text)
    });
    let Some((book_id, fp, text)) = snap else {
        return (0, 0, vec![], vec![]);
    };
    let (_parsed_id, bids, asks) = parse_book_display(&text).unwrap_or((0, vec![], vec![]));
    (book_id, fp, bids, asks)
}

fn usdm_std_stream_id(base: &str) -> String {
    format!("binance-usd-m:{}", base.to_uppercase())
}

fn usdm_rpi_stream_id(base: &str) -> String {
    format!("binance-usd-m:{RPI_PREFIX}{}", base.to_uppercase())
}

fn hub_has_symbol(
    factories_by_symbol: &HashMap<String, ReadHandleFactory<LimitOrderBook>>,
    key: &str,
) -> bool {
    factories_by_symbol
        .keys()
        .any(|k| k.eq_ignore_ascii_case(key))
}

fn hub_has_rpi_depth_pair(
    factories_by_symbol: &HashMap<String, ReadHandleFactory<LimitOrderBook>>,
    instrument: &str,
) -> bool {
    let base = instrument.to_uppercase();
    hub_has_symbol(factories_by_symbol, &usdm_std_stream_id(&base))
        && hub_has_symbol(factories_by_symbol, &usdm_rpi_stream_id(&base))
}

const BOOK_ENTER_RETRIES: u32 = 48;

fn clone_factory_for_symbol(
    factories: &HashMap<String, ReadHandleFactory<LimitOrderBook>>,
    sym: &str,
) -> Option<ReadHandleFactory<LimitOrderBook>> {
    let u = sym.to_uppercase();
    factories
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&u))
        .map(|(_, f)| f.clone())
}

/// Read one symbol’s book, retrying when [`ReadHandle::enter`] fails (common during concurrent depth writes).
fn read_symbol_book_with_retry(
    sym: &str,
    factories: &HashMap<String, ReadHandleFactory<LimitOrderBook>>,
) -> (u64, u64, Vec<(String, String)>, Vec<(String, String)>) {
    let Some(fac) = clone_factory_for_symbol(factories, sym) else {
        return (0, 0, vec![], vec![]);
    };
    for _ in 0..BOOK_ENTER_RETRIES {
        if let Some(out) = fac.handle().enter().map(|g| {
            let book_id = g.update_id;
            let text = format!("{}", *g);
            let fp = fnv1a64_prefix(&text, 4096);
            let (_, bids, asks) = parse_book_display(&text).unwrap_or((0, vec![], vec![]));
            (book_id, fp, bids, asks)
        }) {
            return out;
        }
        thread::yield_now();
    }
    (0, 0, vec![], vec![])
}

/// [`qty(BASE @depth) − qty(RPI:BASE @rpiDepth)`](https://developers.binance.com/docs/derivatives/usds-margined-futures/websocket-market-streams/Diff-Book-Depth-Streams-RPI)
/// per price when both legs exist in the hub.
fn try_snapshot_std_book_minus_rpi_stream(
    instrument: &str,
    factories_by_symbol: &HashMap<String, ReadHandleFactory<LimitOrderBook>>,
) -> Option<(u64, u64, Vec<(String, String)>, Vec<(String, String)>)> {
    let base = instrument.to_uppercase();
    let std_key = usdm_std_stream_id(&base);
    let rpi_key = usdm_rpi_stream_id(&base);
    if !hub_has_symbol(factories_by_symbol, &std_key)
        || !hub_has_symbol(factories_by_symbol, &rpi_key)
    {
        return None;
    }
    let std_snap = read_symbol_book_with_retry(&std_key, factories_by_symbol);
    let rpi_snap = read_symbol_book_with_retry(&rpi_key, factories_by_symbol);
    // diff_qty_levels(left, right) => right − left; want std − rpi ⇒ left=rpi, right=std.
    // Keep Δ=0 rows so identical books still draw (flat line), not an empty chart.
    let db = diff_qty_levels(&rpi_snap.2, &std_snap.2, false);
    let da = diff_qty_levels(&rpi_snap.3, &std_snap.3, false);
    let fp = fnv1a64_prefix(
        &format!("std−rpi|{:016x}|{:016x}", std_snap.1, rpi_snap.1),
        256,
    );
    // Do not XOR update_ids: `@depth` and `@rpiDepth` are independent sequences and can match
    // (then `a ^ a == 0`), which falsely shows book_id 0 between updates.
    let book_id = std_snap.0.wrapping_add(rpi_snap.0);
    Some((book_id, fp, db, da))
}

/// Per-price diff for the `Δ` tab: **only** `@depth − @rpiDepth` when both legs exist; otherwise
/// empty ladders and zeros for ids/fingerprint.
fn snapshot_diff(
    sym: &str,
    factories_by_symbol: &HashMap<String, ReadHandleFactory<LimitOrderBook>>,
) -> (u64, u64, Vec<(String, String)>, Vec<(String, String)>) {
    try_snapshot_std_book_minus_rpi_stream(sym, factories_by_symbol)
        .unwrap_or((0, 0, vec![], vec![]))
}

#[derive(Clone, Debug)]
enum TabKind {
    MergedCanon(String),
    Sym(String),
    Diff(String),
}

/// First-seen order of [`canonical_from_stream_id`] over subscribed stream ids.
fn canonical_first_seen(stream_order: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for s in stream_order {
        let c = canonical_from_stream_id(s);
        if seen.insert(c.clone()) {
            out.push(c);
        }
    }
    out
}

fn tab_strip(stream_order: &[String]) -> (Vec<String>, Vec<TabKind>) {
    let mut labels = Vec::new();
    let mut kinds = Vec::new();
    for c in canonical_first_seen(stream_order) {
        labels.push(format!("MERGED·{c}"));
        kinds.push(TabKind::MergedCanon(c.clone()));
        for s in stream_order {
            if canonical_from_stream_id(s) == c {
                labels.push(s.clone());
                kinds.push(TabKind::Sym(s.clone()));
            }
        }
        labels.push(format!("Δ·{c}"));
        kinds.push(TabKind::Diff(c.clone()));
    }
    (labels, kinds)
}

fn view_for_tab(tab: usize, kinds: &[TabKind]) -> View {
    match kinds.get(tab) {
        Some(TabKind::MergedCanon(c)) => View::MergedCanonical(c.clone()),
        Some(TabKind::Sym(s)) => View::Symbol(s.clone()),
        Some(TabKind::Diff(s)) => View::Diff(s.clone()),
        None => View::MergedCanonical(String::new()),
    }
}

enum View {
    MergedCanonical(String),
    Symbol(String),
    /// Canonical instrument (`BASE`); diff is always `BASE @depth − RPI:BASE @rpiDepth`.
    Diff(String),
}

fn diff_tab_title_to_canonical(view_title: &str) -> String {
    canonical_from_stream_id(
        view_title
            .strip_prefix("Δ·")
            .or_else(|| view_title.strip_prefix("Δ "))
            .unwrap_or(view_title),
    )
}

fn run_tui(
    hub: GlobalBookHub,
    max_rows: usize,
    symbol_order: Vec<String>,
) -> color_eyre::Result<()> {
    let mut stdout = stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen)?;
    let mut term = Terminal::new(ratatui::backend::CrosstermBackend::new(stdout))?;

    let mut tab: usize = 0;

    loop {
        // Do not call `Terminal::clear()` each frame: it erases the whole screen before paint and
        // causes visible flicker. `draw()` already refreshes the buffer for this frame.

        let (tab_labels, tab_kinds) = tab_strip(&symbol_order);
        if tab >= tab_labels.len() {
            tab = tab_labels.len().saturating_sub(1);
        }

        let by_sym: HashMap<String, ReadHandleFactory<LimitOrderBook>> =
            hub.per_source_factories().into_iter().collect();

        let view = view_for_tab(tab, &tab_kinds);
        let view_title = tab_labels
            .get(tab)
            .cloned()
            .unwrap_or_else(|| "?".to_string());

        let missing_symbol = match &view {
            View::MergedCanonical(c) => {
                c.is_empty() || hub.merged_factory_for(c).is_none()
            }
            View::Symbol(s) => !s.is_empty() && !by_sym.keys().any(|k| k.eq_ignore_ascii_case(s)),
            View::Diff(s) => s.is_empty() || !hub_has_rpi_depth_pair(&by_sym, s),
        };
        let (book_update_id, snap_fp, bids, asks) = match &view {
            View::Diff(sym) => snapshot_diff(sym, &by_sym),
            _ => snapshot_ladder(&view, &by_sym, &hub),
        };

        let is_diff = matches!(view, View::Diff(_));

        // LOB ladders: bids ascending (best bid = last), asks descending (best ask = last).
        // Diff ladders are sorted ascending by price on both sides (best ask = lowest = first).
        let best_bid = bids.last().map(|(p, q)| format!("{p} × {q}"));
        let best_ask = if is_diff {
            asks.first().map(|(p, q)| format!("{p} × {q}"))
        } else {
            asks.last().map(|(p, q)| format!("{p} × {q}"))
        };

        term.draw(|f| {
            ui(
                f,
                &tab_labels,
                tab,
                &view_title,
                is_diff,
                missing_symbol,
                book_update_id,
                snap_fp,
                best_bid.as_deref(),
                best_ask.as_deref(),
                bids.len(),
                asks.len(),
                &bids,
                &asks,
                max_rows,
            )
        })?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Right | KeyCode::Tab => {
                            tab = (tab + 1) % tab_labels.len().max(1);
                        }
                        KeyCode::Left => {
                            tab = tab.checked_sub(1).unwrap_or(tab_labels.len().saturating_sub(1));
                        }
                        _ => {}
                    }
                }
            }
        }

    }

    disable_raw_mode()?;
    execute!(
        term.backend_mut(),
        LeaveAlternateScreen,
        crossterm::cursor::Show
    )?;
    Ok(())
}

fn ui(
    f: &mut Frame,
    tabs: &[String],
    tab: usize,
    view_title: &str,
    is_diff: bool,
    missing_symbol: bool,
    book_update_id: u64,
    snap_fp: u64,
    best_bid: Option<&str>,
    best_ask: Option<&str>,
    n_bids: usize,
    n_asks: usize,
    bids: &[(String, String)],
    asks: &[(String, String)],
    max_levels: usize,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(9),
            Constraint::Min(10),
        ])
        .split(f.area());

    let tab_w = Tabs::new(tabs.iter().map(|t| t.clone().yellow()))
        .select(tab)
        .block(Block::default().borders(Borders::ALL).title("Book (Tab / ← →)"));
    f.render_widget(tab_w, chunks[0]);

    let meta = if missing_symbol && is_diff {
        let base = diff_tab_title_to_canonical(view_title);
        let std = usdm_std_stream_id(&base);
        let rpi = usdm_rpi_stream_id(&base);
        format!(
            "view: {view_title}  |  Δ undefined: hub needs `{std}` (@depth) and `{rpi}` (@rpiDepth). Add both to --sources. No fallback.  |  q quit"
        )
    } else if missing_symbol {
        format!(
            "view: {view_title}  |  NO BOOK YET (waiting for hub)  |  book_id: {book_update_id}"
        )
    } else if is_diff {
        let bb = best_bid.unwrap_or("—");
        let ba = best_ask.unwrap_or("—");
        let base = diff_tab_title_to_canonical(view_title);
        let std = usdm_std_stream_id(&base);
        let rpi = usdm_rpi_stream_id(&base);
        let explain = format!(
            "Δ = {std} @depth − {rpi} @rpiDepth  |  RPI = Retail Price Improvement (Binance: post-only TIF, matched only with APP/Web). @rpiDepth includes RPI layers in the pushed book. +Δ ⇒ more size on public depth than RPI snapshot at that price (MM/API proxy, not official MM tag).\nPrices are string keys; formatting can split ticks."
        );
        format!(
            "view: {view_title}  |  book_id {book_update_id}  |  snap {snap_fp:016x}  |  non-zero Δ rows {n_bids}/{n_asks}  |  bid Δ @ high p {bb}  ask Δ @ low p {ba}  |  q quit\n{explain}"
        )
    } else {
        let bb = best_bid.unwrap_or("—");
        let ba = best_ask.unwrap_or("—");
        format!(
            "view: {view_title}  |  book_id {book_update_id}  |  snap {snap_fp:016x}  |  rows {n_bids}/{n_asks}  |  top bid {bb}  top ask {ba}  |  q quit\n(same snap on different tabs = identical in-memory book; RPI vs public can still look very similar)"
        )
    };
    let hdr = Paragraph::new(meta).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Meta")
            .border_style(if missing_symbol {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            }),
    );
    f.render_widget(hdr, chunks[1]);

    let (bid_pts, ask_pts) = if is_diff {
        (
            bid_delta_line_top_n(bids, max_levels),
            ask_delta_line_top_n(asks, max_levels),
        )
    } else {
        (
            bid_cumulative_top_n(bids, max_levels),
            ask_cumulative_top_n(asks, max_levels),
        )
    };

    let mut xs: Vec<f64> = bid_pts.iter().map(|(p, _)| *p).collect();
    xs.extend(ask_pts.iter().map(|(p, _)| *p));
    let mut ys: Vec<f64> = bid_pts.iter().map(|(_, y)| *y).collect();
    ys.extend(ask_pts.iter().map(|(_, y)| *y));

    let (xb, yb) = if xs.is_empty() {
        ([0.0, 1.0], [0.0, 1.0])
    } else {
        let x0 = xs.iter().copied().fold(f64::INFINITY, f64::min);
        let x1 = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if is_diff {
            let y_lo = ys.iter().copied().fold(f64::INFINITY, f64::min);
            let y_hi = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let (yl, yh) = if !y_lo.is_finite() || !y_hi.is_finite() {
                (0.0, 1.0)
            } else if (y_hi - y_lo).abs() < 1e-18 {
                (y_lo - 1.0, y_hi + 1.0)
            } else {
                let pad = (y_hi - y_lo) * 0.06;
                (y_lo - pad, y_hi + pad)
            };
            (widen_bounds(x0, x1), [yl, yh])
        } else {
            let y0 = 0.0_f64;
            let y1 = ys.iter().copied().fold(0.0_f64, f64::max).max(1e-9);
            (widen_bounds(x0, x1), [y0, y1 * 1.05])
        }
    };

    let x_mid = (xb[0] + xb[1]) / 2.0;
    let y_mid = (yb[0] + yb[1]) / 2.0;

    let placeholder: [(f64, f64); 2] = [(xb[0], yb[0]), (xb[1], yb[1])];
    let mut datasets: Vec<Dataset<'_>> = Vec::new();
    if bid_pts.is_empty() && ask_pts.is_empty() {
        datasets.push(
            Dataset::default()
                .name("no data")
                .marker(Marker::Dot)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(Color::DarkGray))
                .data(&placeholder[..]),
        );
    } else {
        if !bid_pts.is_empty() {
            let bid_name = if is_diff {
                "bid @depth−rpi"
            } else {
                "bids (cum)"
            };
            datasets.push(
                Dataset::default()
                    .name(bid_name)
                    .marker(Marker::Braille)
                    .graph_type(GraphType::Line)
                    .style(Style::default().fg(Color::Green))
                    .data(bid_pts.as_slice()),
            );
        }
        if !ask_pts.is_empty() {
            let ask_name = if is_diff {
                "ask @depth−rpi"
            } else {
                "asks (cum)"
            };
            datasets.push(
                Dataset::default()
                    .name(ask_name)
                    .marker(Marker::Braille)
                    .graph_type(GraphType::Line)
                    .style(Style::default().fg(Color::Red))
                    .data(ask_pts.as_slice()),
            );
        }
    }

    let chart_title = if missing_symbol && is_diff {
        "Δ undefined — need PAIR + RPI:PAIR in hub".into()
    } else if missing_symbol {
        "Depth (no book)".into()
    } else if bid_pts.is_empty() && ask_pts.is_empty() {
        if is_diff {
            "Δ @depth−@rpiDepth: no differing levels at shared prices".into()
        } else {
            "Depth (no parseable levels)".into()
        }
    } else if is_diff {
        format!("Δ = public @depth − RPI @rpiDepth (per price; top {max_levels} / side)")
    } else {
        format!("Depth (cumulative qty vs price; top {max_levels} levels / side)")
    };

    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(chart_title)
                .border_style(
                    if missing_symbol {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default()
                    },
                ),
        )
        .x_axis(
            Axis::default()
                .title("price")
                .style(Style::default().fg(Color::Gray))
                .bounds(xb)
                .labels([
                    fmt_axis_tick(xb[0]).dim(),
                    fmt_axis_tick(x_mid).dim(),
                    fmt_axis_tick(xb[1]).dim(),
                ]),
        )
        .y_axis(
            Axis::default()
                .title(if is_diff { "Δ qty" } else { "cum qty" })
                .style(Style::default().fg(Color::Gray))
                .bounds(yb)
                .labels([
                    fmt_axis_tick(yb[0]).dim(),
                    fmt_axis_tick(y_mid).dim(),
                    fmt_axis_tick(yb[1]).dim(),
                ]),
        )
        .legend_position(Some(LegendPosition::TopRight))
        .style(Style::default().bg(Color::Reset));

    f.render_widget(chart, chunks[2]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_from_stream_id_standard() {
        assert_eq!(canonical_from_stream_id("binance:BTCUSDT"), "BTCUSDT");
        assert_eq!(canonical_from_stream_id("binance-usd-m:BTCUSDT"), "BTCUSDT");
    }

    #[test]
    fn canonical_from_stream_id_rpi_preserves_prefix() {
        assert_eq!(
            canonical_from_stream_id("binance-usd-m:RPI:BTCUSDT"),
            "RPI:BTCUSDT"
        );
    }

    #[test]
    fn canonical_from_stream_id_unknown_provider() {
        assert_eq!(canonical_from_stream_id("kraken:BTCUSD"), "KRAKEN:BTCUSD");
    }

    #[test]
    fn usdm_std_stream_id_format() {
        assert_eq!(usdm_std_stream_id("BTCUSDT"), "binance-usd-m:BTCUSDT");
    }

    #[test]
    fn usdm_rpi_stream_id_format() {
        assert_eq!(usdm_rpi_stream_id("BTCUSDT"), "binance-usd-m:RPI:BTCUSDT");
    }

    #[test]
    fn tab_strip_includes_delta_tab_per_canonical() {
        let streams = vec![
            "binance-usd-m:BTCUSDT".to_string(),
            "binance-usd-m:RPI:BTCUSDT".to_string(),
        ];
        let (labels, kinds) = tab_strip(&streams);
        assert!(labels.iter().any(|l| l.starts_with("Δ·")), "missing Δ tab in {labels:?}");
        let delta_idx = labels.iter().position(|l| l.starts_with("Δ·")).unwrap();
        assert!(matches!(kinds[delta_idx], TabKind::Diff(_)));
    }

    #[test]
    fn tab_strip_rpi_gets_own_canonical_group() {
        let streams = vec![
            "binance-usd-m:BTCUSDT".to_string(),
            "binance-usd-m:RPI:BTCUSDT".to_string(),
        ];
        let (labels, _) = tab_strip(&streams);
        let merged_labels: Vec<_> = labels.iter().filter(|l| l.starts_with("MERGED·")).collect();
        assert_eq!(merged_labels.len(), 2, "RPI and standard get separate MERGED tabs: {labels:?}");
        assert!(merged_labels.iter().any(|l| l.contains("BTCUSDT") && !l.contains("RPI")));
        assert!(merged_labels.iter().any(|l| l.contains("RPI:BTCUSDT")));
    }

    #[test]
    fn diff_qty_levels_computes_per_price_delta() {
        let left = vec![("100.0".into(), "1.0".into()), ("101.0".into(), "2.0".into())];
        let right = vec![("100.0".into(), "3.0".into()), ("101.0".into(), "2.0".into())];
        let result = diff_qty_levels(&left, &right, true);
        assert_eq!(result.len(), 1, "only non-zero delta should remain");
        assert_eq!(result[0].0, "100.0");
        let delta: f64 = result[0].1.parse().unwrap();
        assert!((delta - 2.0).abs() < 1e-9, "delta should be 3.0 - 1.0 = 2.0, got {delta}");
    }

    #[test]
    fn diff_qty_levels_includes_zero_when_not_omitted() {
        let left = vec![("100.0".into(), "5.0".into())];
        let right = vec![("100.0".into(), "5.0".into())];
        let with_zero = diff_qty_levels(&left, &right, false);
        assert_eq!(with_zero.len(), 1);
        let delta: f64 = with_zero[0].1.parse().unwrap();
        assert!(delta.abs() < 1e-12);
    }

    #[test]
    fn diff_qty_levels_union_of_prices() {
        let left = vec![("100.0".into(), "1.0".into())];
        let right = vec![("101.0".into(), "2.0".into())];
        let result = diff_qty_levels(&left, &right, false);
        assert_eq!(result.len(), 2);
        let prices: Vec<&str> = result.iter().map(|(p, _)| p.as_str()).collect();
        assert!(prices.contains(&"100.0"));
        assert!(prices.contains(&"101.0"));
    }

    #[test]
    fn hub_has_rpi_depth_pair_requires_both_streams() {
        let factories: HashMap<String, ReadHandleFactory<LimitOrderBook>> = HashMap::new();
        assert!(!hub_has_rpi_depth_pair(&factories, "BTCUSDT"));
    }

    #[test]
    fn view_for_tab_diff_returns_diff_variant() {
        let kinds = vec![
            TabKind::MergedCanon("BTCUSDT".into()),
            TabKind::Sym("binance-usd-m:BTCUSDT".into()),
            TabKind::Diff("BTCUSDT".into()),
        ];
        let view = view_for_tab(2, &kinds);
        assert!(matches!(view, View::Diff(ref s) if s == "BTCUSDT"));
    }

    #[test]
    fn rpi_canonical_instrument_differs_from_standard() {
        let std = BookSource::parse("binance-usd-m:BTCUSDT").unwrap();
        let rpi = BookSource::parse("binance-usd-m:RPI:BTCUSDT").unwrap();
        assert_ne!(
            std.canonical_instrument(),
            rpi.canonical_instrument(),
            "RPI must not share canonical instrument with standard"
        );
        assert_eq!(std.canonical_instrument(), "BTCUSDT");
        assert_eq!(rpi.canonical_instrument(), "RPI:BTCUSDT");
    }

    #[test]
    fn rpi_does_not_appear_in_standard_canonical_tab_strip() {
        let streams = vec![
            "binance-usd-m:BTCUSDT".to_string(),
            "binance-usd-m:RPI:BTCUSDT".to_string(),
        ];
        let (labels, kinds) = tab_strip(&streams);

        let btc_merged_idx = labels
            .iter()
            .position(|l| l == "MERGED·BTCUSDT")
            .expect("should have MERGED·BTCUSDT");
        let rpi_merged_idx = labels
            .iter()
            .position(|l| l == "MERGED·RPI:BTCUSDT")
            .expect("should have MERGED·RPI:BTCUSDT");

        assert_ne!(btc_merged_idx, rpi_merged_idx);
        assert!(matches!(&kinds[btc_merged_idx], TabKind::MergedCanon(s) if s == "BTCUSDT"));
        assert!(matches!(&kinds[rpi_merged_idx], TabKind::MergedCanon(s) if s == "RPI:BTCUSDT"));
    }
}
