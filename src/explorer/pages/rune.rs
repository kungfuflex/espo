use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_ENCODING, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use bitcoin::Txid;
use bitcoin::hashes::Hash;
use maud::{Markup, PreEscaped, html};
use serde::Deserialize;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::explorer::components::dropdown::{DropdownItem, DropdownProps, dropdown};
use crate::explorer::components::layout::layout_with_meta;
use crate::explorer::components::rune_icon::rune_icon;
use crate::explorer::components::svg_assets::{
    icon_activity, icon_activity_mint, icon_pager_first, icon_pager_last, icon_pager_left,
    icon_pager_right,
};
use crate::explorer::components::table::holders_table;
use crate::explorer::pages::common::{fmt_scaled_amount, format_integer};
use crate::explorer::pages::state::ExplorerState;
use crate::explorer::paths::explorer_path;
use crate::modules::ammdata::consts::PRICE_SCALE;
use crate::modules::runes::storage::{
    GetRuneActivityPageParams, RuneActivity, RuneActivityKind, RuneActivityScope,
    RuneActivitySortField, RuneEntry, RuneVolumeKind as StorageRuneVolumeKind, RunesProvider,
    SortDir as RuneSortDir,
};
use alloy_primitives::U256;

const ADDR_SUFFIX_LEN: usize = 8;

#[derive(Deserialize)]
pub struct PageQuery {
    pub tab: Option<String>,
    pub volume: Option<String>,
    pub order: Option<String>,
    pub dir: Option<String>,
    pub filter: Option<String>,
    pub page: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuneTab {
    Holders,
    Volume,
    Activity,
}

impl RuneTab {
    fn from_query(raw: Option<&str>) -> Self {
        match raw {
            Some("volume") => Self::Volume,
            Some("activity") => Self::Activity,
            _ => Self::Holders,
        }
    }

    fn as_query(self) -> &'static str {
        match self {
            Self::Holders => "holders",
            Self::Volume => "volume",
            Self::Activity => "activity",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VolumeKind {
    TransferVolume,
    TotalReceived,
}

impl VolumeKind {
    fn from_query(tab: Option<&str>, volume: Option<&str>) -> Self {
        match (tab, volume) {
            (Some("total_received"), _) | (_, Some("total_received")) => Self::TotalReceived,
            _ => Self::TransferVolume,
        }
    }

    fn query_value(self) -> &'static str {
        match self {
            Self::TransferVolume => "transfer_volume",
            Self::TotalReceived => "total_received",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::TransferVolume => "Transfer Volume",
            Self::TotalReceived => "Total Received",
        }
    }

    fn storage_kind(self) -> StorageRuneVolumeKind {
        match self {
            Self::TransferVolume => StorageRuneVolumeKind::TransferVolume,
            Self::TotalReceived => StorageRuneVolumeKind::TotalReceived,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActivityOrder {
    Timestamp,
    Volume,
}

impl ActivityOrder {
    fn from_query(raw: Option<&str>) -> Self {
        match raw {
            Some("volume") | Some("amount") => Self::Volume,
            _ => Self::Timestamp,
        }
    }

    fn as_query(self) -> &'static str {
        match self {
            Self::Timestamp => "timestamp",
            Self::Volume => "volume",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Timestamp => "Timestamp",
            Self::Volume => "Volume",
        }
    }

    fn storage_sort(self) -> RuneActivitySortField {
        match self {
            Self::Timestamp => RuneActivitySortField::Timestamp,
            Self::Volume => RuneActivitySortField::Amount,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActivityDir {
    Desc,
    Asc,
}

impl ActivityDir {
    fn from_query(raw: Option<&str>) -> Self {
        match raw {
            Some("asc") => Self::Asc,
            _ => Self::Desc,
        }
    }

    fn as_query(self) -> &'static str {
        match self {
            Self::Desc => "desc",
            Self::Asc => "asc",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Desc => "Descending",
            Self::Asc => "Ascending",
        }
    }

    fn storage_dir(self) -> RuneSortDir {
        match self {
            Self::Desc => RuneSortDir::Desc,
            Self::Asc => RuneSortDir::Asc,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActivityFilter {
    All,
    Mint,
    Etch,
}

impl ActivityFilter {
    fn from_query(raw: Option<&str>) -> Self {
        match raw {
            Some("mint") | Some("mints") => Self::Mint,
            Some("etch") | Some("etching") | Some("etchings") => Self::Etch,
            _ => Self::All,
        }
    }

    fn as_query(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Mint => "mint",
            Self::Etch => "etch",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "All activity",
            Self::Mint => "Only mints",
            Self::Etch => "Only etchings",
        }
    }

    fn storage_scope(self) -> RuneActivityScope {
        match self {
            Self::All => RuneActivityScope::All,
            Self::Mint => RuneActivityScope::Mint,
            Self::Etch => RuneActivityScope::Etch,
        }
    }
}

pub async fn rune_page(
    State(state): State<ExplorerState>,
    Path(rune): Path<String>,
    Query(q): Query<PageQuery>,
) -> Response {
    let provider = RunesProvider::new(Arc::new(state.runes_mdb.clone()));
    let Some(entry) = provider.get_rune_by_query(&rune).ok().flatten() else {
        return (StatusCode::NOT_FOUND, "rune not found").into_response();
    };
    let page = q.page.unwrap_or(1).max(1);
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let tab = RuneTab::from_query(q.tab.as_deref());
    let volume_kind = VolumeKind::from_query(q.tab.as_deref(), q.volume.as_deref());
    let activity_order = ActivityOrder::from_query(q.order.as_deref());
    let activity_dir = ActivityDir::from_query(q.dir.as_deref());
    let activity_filter = ActivityFilter::from_query(q.filter.as_deref());
    let holders = provider.get_holders_count(entry.id).unwrap_or(0);
    let is_uncommon_goods = entry.id.block == 1 && entry.id.tx == 0;
    let all_range_label = "All";

    layout_with_meta(
        &entry.spaced_name,
        &format!("/rune/{}", entry.id),
        Some("Rune details"),
        html! {
            div class="alkane-page" {
                div class="alkane-hero-card" {
                    (rune_icon(&entry, "alk-icon-wrap alk-icon-lg"))
                    div class="alkane-hero-text" {
                        span class="alkane-tag" { "RUNE" }
                        h1 class="alkane-hero-title" { (entry.spaced_name.clone()) }
                        span class="alkane-hero-id mono" { (entry.id.to_string()) }
                    }
                }

                section class="alkane-section" data-alkane-overview="" {
                    div class="alkane-overview-grid" data-chart-hidden="1" {
                        div class="alkane-overview-pane" {
                            h2 class="section-title" { "Overview" }
                            div class="alkane-overview-card" {
                                div class="alkane-stat" {
                                    span class="alkane-stat-label" { "Symbol" }
                                    div class="alkane-stat-line" {
                                        span class="alkane-stat-value" { (entry.symbol.clone().unwrap_or_else(|| "¤".to_string())) }
                                    }
                                }
                                div class="alkane-stat" {
                                    span class="alkane-stat-label" { "Circulating supply" }
                                    div class="alkane-stat-line" {
                                        span class="alkane-stat-value" { (fmt_rune_amount(&entry, entry.supply())) }
                                        span class="alkane-stat-sub" { (format!("(with {} decimals)", entry.divisibility)) }
                                    }
                                }
                                div class="alkane-stat" {
                                    span class="alkane-stat-label" { "Holders" }
                                    div class="alkane-stat-line" {
                                        span class="alkane-stat-value" { (format_integer(holders as u128)) }
                                    }
                                }
                                div class="alkane-stat" {
                                    span class="alkane-stat-label" { "Mints" }
                                    div class="alkane-stat-line" {
                                        span class="alkane-stat-value" { (format_integer(entry.mints)) }
                                    }
                                }
                                div class="alkane-stat" {
                                    span class="alkane-stat-label" { "Burned" }
                                    div class="alkane-stat-line" {
                                        span class="alkane-stat-value" { (fmt_rune_amount(&entry, entry.burned)) }
                                    }
                                }
                                div class="alkane-stat" {
                                    span class="alkane-stat-label" { "Etching transaction" }
                                    div class="alkane-stat-line" {
                                        @let txid = Txid::from_byte_array(entry.etching_txid).to_string();
                                        a class="alkane-stat-value link mono" href=(explorer_path(&format!("/tx/{txid}"))) { (short_hex(&txid)) }
                                    }
                                }
                            }
                        }
                    }
                }

                @if is_uncommon_goods {
                    section class="alkane-section" {
                        div
                            class="card address-balance-chart-card"
                            data-minting-price-chart=""
                            data-minting-price-kind="rune"
                            data-default-range="all"
                        {
                            div class="address-balance-chart-head" {
                                h2 class="h2 minting-price-title" { "Minting Price" }
                            }
                            div class="address-balance-chart-plot" data-minting-price-chart-root {
                                div class="address-balance-chart-loading" data-minting-price-chart-loading="" data-spinning="1" {
                                    span class="spinner address-balance-chart-spinner" data-minting-price-chart-loading-spinner="" aria-hidden="true" {}
                                    span data-minting-price-chart-loading-text="" { "Loading chart..." }
                                }
                            }
                            div class="address-balance-chart-tabs" {
                                button type="button" class="address-balance-chart-tab" data-range="1d" { "1D" }
                                button type="button" class="address-balance-chart-tab" data-range="1w" { "1W" }
                                button type="button" class="address-balance-chart-tab" data-range="1m" { "1M" }
                                button type="button" class="address-balance-chart-tab active" data-range="all" { (all_range_label) }
                            }
                        }
                    }
                }

                section class="alkane-section" {
                    div class="alkane-tabs" {
                        div class="alkane-tab-list" {
                            (tab_link(&entry, RuneTab::Holders, tab, limit))
                            (tab_link(&entry, RuneTab::Volume, tab, limit))
                            (tab_link(&entry, RuneTab::Activity, tab, limit))
                        }
                        div class="alkane-tab-panel" {
                            (tab_body(
                                &provider,
                                &entry,
                                tab,
                                page,
                                limit,
                                holders as usize,
                                volume_kind,
                                activity_order,
                                activity_dir,
                                activity_filter,
                            ))
                        }
                    }
                }
            }
            @if page > 1 {
                (rune_tab_autoscroll_script())
            }
            @if is_uncommon_goods {
                (minting_price_chart_scripts())
            }
        },
    )
    .into_response()
}

fn rune_tab_autoscroll_script() -> Markup {
    PreEscaped(
        r#"<script>
(() => {
  const scrollToTabs = () => {
    const target = document.querySelector('.alkane-tab-list');
    if (!target) return;
    const top = Math.max(0, target.getBoundingClientRect().top + window.scrollY);
    window.scrollTo({ top, left: 0, behavior: 'auto' });
  };
  scrollToTabs();
  requestAnimationFrame(scrollToTabs);
})();
</script>"#
            .to_string(),
    )
}

fn minting_price_chart_scripts() -> Markup {
    PreEscaped(
        r#"<script>
(() => {
  const card = document.querySelector('[data-minting-price-chart]');
  if (!card) return;
  const apiPath = '../api/minting-price-chart';
  const kind = (card.dataset.mintingPriceKind || 'rune').trim();
  const root = card.querySelector('[data-minting-price-chart-root]');
  const loading = card.querySelector('[data-minting-price-chart-loading]');
  const loadingText = card.querySelector('[data-minting-price-chart-loading-text]');
  const spinner = card.querySelector('[data-minting-price-chart-loading-spinner]');
  const tabs = Array.from(card.querySelectorAll('[data-range]'));
  let chart = null;
  let canvas = null;
  let tooltipEl = null;
  let activeRange = (card.dataset.defaultRange || 'all').toLowerCase();
  let inFlight = false;

  const color = (name, fallback) =>
    getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
  const setLoading = (message, spin) => {
    if (!loading) return;
    if (loadingText) loadingText.textContent = message;
    else loading.textContent = message;
    loading.dataset.spinning = spin ? '1' : '0';
    if (spinner) spinner.style.display = spin ? '' : 'none';
    loading.style.display = '';
  };
  const hideLoading = () => {
    if (loading) loading.style.display = 'none';
  };
  const clearChart = () => {
    if (chart) chart.destroy();
    chart = null;
    hideTooltip();
    if (canvas) canvas.remove();
    canvas = null;
  };
  const ensureScript = (src) => new Promise((resolve, reject) => {
    const existing = document.querySelector(`script[src="${src}"]`);
    if (existing) {
      if (existing.dataset.loaded === '1') resolve();
      else {
        existing.addEventListener('load', resolve, { once: true });
        existing.addEventListener('error', reject, { once: true });
      }
      return;
    }
    const script = document.createElement('script');
    script.src = src;
    script.async = true;
    script.addEventListener('load', () => {
      script.dataset.loaded = '1';
      resolve();
    }, { once: true });
    script.addEventListener('error', reject, { once: true });
    document.head.appendChild(script);
  });
  const loadChartJs = async () => {
    if (!window.Chart) {
      await ensureScript('https://cdnjs.cloudflare.com/ajax/libs/Chart.js/4.4.1/chart.umd.min.js');
    }
  };
  const ensureCanvas = () => {
    if (!canvas) {
      canvas = document.createElement('canvas');
      canvas.setAttribute('aria-label', 'Minting price history');
      canvas.setAttribute('role', 'img');
      if (loading && loading.parentNode === root) root.insertBefore(canvas, loading);
      else root.appendChild(canvas);
    }
    return canvas.getContext('2d');
  };
  const usd = (v) =>
    new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD', maximumFractionDigits: 6 }).format(v || 0);
  const unitLabel = kind === 'rune' || kind === 'ug' || kind === 'uncommon_goods' ? 'UG' : 'DIESEL';
  const formatBlock = (height) => {
    if (!Number.isFinite(height)) return 'Block';
    return `Block ${new Intl.NumberFormat('en-US', { maximumFractionDigits: 0 }).format(height)}`;
  };
  const ensureTooltip = () => {
    if (!root) return null;
    if (tooltipEl && tooltipEl.isConnected) return tooltipEl;
    tooltipEl = document.createElement('div');
    tooltipEl.className = 'address-balance-chart-tooltip';
    tooltipEl.innerHTML = `
      <div class="address-balance-chart-tooltip-title" data-minting-price-tooltip-title=""></div>
      <div class="address-balance-chart-tooltip-row">
        <span class="address-balance-chart-tooltip-value" data-minting-price-tooltip-value=""></span>
      </div>
    `;
    root.appendChild(tooltipEl);
    return tooltipEl;
  };
  const hideTooltip = () => {
    if (!tooltipEl) return;
    tooltipEl.dataset.visible = '0';
    tooltipEl.style.opacity = '0';
  };
  const renderTooltip = (context) => {
    const tooltip = context && context.tooltip ? context.tooltip : null;
    const el = ensureTooltip();
    if (!tooltip || !el) return;
    if (tooltip.opacity === 0 || !tooltip.dataPoints || tooltip.dataPoints.length === 0) {
      hideTooltip();
      return;
    }
    const dataPoint = tooltip.dataPoints[0];
    const rawHeight = Number(dataPoint ? dataPoint.label : NaN);
    const rawValue = dataPoint && dataPoint.parsed && typeof dataPoint.parsed.y === 'number'
      ? dataPoint.parsed.y
      : NaN;
    const titleEl = el.querySelector('[data-minting-price-tooltip-title]');
    if (titleEl) titleEl.textContent = formatBlock(rawHeight);
    const valueEl = el.querySelector('[data-minting-price-tooltip-value]');
    if (valueEl) valueEl.textContent = `${usd(rawValue)} / ${unitLabel}`;
    const padding = 8;
    const width = el.offsetWidth;
    const height = el.offsetHeight;
    const maxLeft = Math.max(padding, root.clientWidth - width - padding);
    const maxTop = Math.max(padding, root.clientHeight - height - padding);
    const left = Math.min(Math.max(tooltip.caretX + 12, padding), maxLeft);
    const top = Math.min(Math.max(tooltip.caretY + 12, padding), maxTop);
    el.style.left = `${left}px`;
    el.style.top = `${top}px`;
    el.dataset.visible = '1';
    el.style.opacity = '1';
  };
  const render = (points) => {
    const ctx = ensureCanvas();
    const labels = points.map((p) => p.height);
    const values = points.map((p) => p.value);
    const min = Math.min(...values);
    const max = Math.max(...values);
    const span = Math.max(max - min, Math.abs(max) || 1);
    const yMin = Math.max(0, min - span * 0.12);
    const yMax = max + span * 0.12;
    const line = color('--link', '#7db7ff');
    const fill = 'rgba(125, 183, 255, 0.14)';
    if (chart) {
      chart.data.labels = labels;
      chart.data.datasets[0].data = values;
      chart.options.scales.y.min = yMin;
      chart.options.scales.y.max = yMax;
      chart.update('none');
      return;
    }
    chart = new window.Chart(ctx, {
      type: 'line',
      data: { labels, datasets: [{ data: values, borderColor: line, backgroundColor: fill, borderWidth: 3, pointRadius: 0, pointHoverRadius: 5, pointHoverBackgroundColor: line, pointHoverBorderColor: color('--text', '#ffffff'), pointHoverBorderWidth: 2, tension: 0.35, cubicInterpolationMode: 'monotone', fill: 'start' }] },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        animation: false,
        plugins: {
          legend: { display: false },
          tooltip: { enabled: false, external: renderTooltip }
        },
        interaction: { mode: 'index', intersect: false },
        hover: { mode: 'index', intersect: false },
        scales: { x: { display: false }, y: { display: false, min: yMin, max: yMax } }
      }
    });
  };
  const load = async (range) => {
    if (inFlight) return;
    inFlight = true;
    setLoading('Loading chart...', true);
    try {
      const res = await fetch(`${apiPath}?${new URLSearchParams({ kind, range })}`, { headers: { Accept: 'application/json' } });
      const data = await res.json();
      const points = Array.isArray(data && data.points) ? data.points.slice().sort((a, b) => a.height - b.height) : [];
      if (!data || !data.ok || points.length === 0) {
        clearChart();
        card.removeAttribute('data-tone');
        setLoading('No chart data for this selection', false);
        return;
      }
      await loadChartJs();
      const first = Number(points[0].value);
      const last = Number(points[points.length - 1].value);
      card.dataset.tone = last >= first ? 'up' : 'down';
      hideLoading();
      render(points);
    } catch (_) {
      clearChart();
      setLoading('Chart unavailable', false);
    } finally {
      inFlight = false;
    }
  };
  const setActive = (range) => tabs.forEach((tab) => tab.classList.toggle('active', tab.dataset.range === range));
  tabs.forEach((tab) => tab.addEventListener('click', () => {
    const range = (tab.dataset.range || 'all').toLowerCase();
    if (range === activeRange) return;
    activeRange = range;
    setActive(range);
    load(range);
  }));
  setActive(activeRange);
  load(activeRange);
})();
</script>"#
            .to_string(),
    )
}

pub async fn rune_icon_asset(
    State(state): State<ExplorerState>,
    Path(rune): Path<String>,
) -> Response {
    let provider = RunesProvider::new(Arc::new(state.runes_mdb.clone()));
    let Some(entry) = provider.get_rune_by_query(&rune).ok().flatten() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(icon) = provider.get_rune_icon(entry.id).ok().flatten() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(content_type) = HeaderValue::from_str(&icon.content_type) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let mut response =
        Response::builder().status(StatusCode::OK).header(CONTENT_TYPE, content_type);
    if let Some(content_encoding) = icon.content_encoding {
        if let Ok(value) = HeaderValue::from_str(&content_encoding) {
            response = response.header(CONTENT_ENCODING, value);
        }
    }

    response
        .body(Body::from(icon.body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn tab_link(entry: &RuneEntry, tab: RuneTab, active: RuneTab, limit: usize) -> Markup {
    let id = entry.id.to_string();
    let label = match tab {
        RuneTab::Holders => "Holders",
        RuneTab::Volume => "Volume",
        RuneTab::Activity => "Activity",
    };
    let class = if tab == active { "alkane-tab active" } else { "alkane-tab" };
    html! {
        a class=(class) href=(explorer_path(&format!("/rune/{id}?tab={}&page=1&limit={limit}", tab.as_query()))) { (label) }
    }
}

fn tab_body(
    provider: &RunesProvider,
    entry: &RuneEntry,
    tab: RuneTab,
    page: usize,
    limit: usize,
    holders_total: usize,
    volume_kind: VolumeKind,
    activity_order: ActivityOrder,
    activity_dir: ActivityDir,
    activity_filter: ActivityFilter,
) -> Markup {
    let id = entry.id.to_string();
    match tab {
        RuneTab::Holders => {
            let rows = provider.get_holders(entry.id, page, limit).unwrap_or_default();
            let supply = entry.supply();
            let rows_len = rows.len();
            let rows = rows
                .into_iter()
                .map(|(address, amount)| {
                    let pct_label = if supply == 0 {
                        "0%".to_string()
                    } else {
                        format!("{:.4}%", (amount as f64 / supply as f64) * 100.0)
                    };
                    vec![
                        html! { a class="link mono" href=(explorer_path(&format!("/address/{address}"))) { (address) } },
                        rune_amount_line(entry, amount),
                        html! { span class="alk-holding-pct mono" { (pct_label) } },
                    ]
                })
                .collect();
            html! {
                div class="alkane-panel alkane-holders-card" {
                    (holders_table(&["Holder", "Balance", "Holding %"], rows))
                }
                (pager(holders_total, rows_len, page, limit, |target| {
                    explorer_path(&format!("/rune/{id}?tab=holders&page={target}&limit={limit}"))
                }))
            }
        }
        RuneTab::Volume => {
            let (total, rows) = provider
                .get_volume(entry.id, volume_kind.storage_kind(), page, limit)
                .unwrap_or_default();
            let rows_len = rows.len();
            let volume_query = volume_kind.query_value();
            let volume_dropdown = dropdown(DropdownProps {
                label: Some(volume_kind.label().to_string()),
                selected_icon: None,
                aria_label: Some("Select volume metric".to_string()),
                items: vec![
                    DropdownItem {
                        label: "Transfer Volume".to_string(),
                        href: explorer_path(&format!(
                            "/rune/{id}?tab=volume&volume=transfer_volume&page=1&limit={limit}"
                        )),
                        icon: None,
                        selected: volume_kind == VolumeKind::TransferVolume,
                    },
                    DropdownItem {
                        label: "Total Received".to_string(),
                        href: explorer_path(&format!(
                            "/rune/{id}?tab=volume&volume=total_received&page=1&limit={limit}"
                        )),
                        icon: None,
                        selected: volume_kind == VolumeKind::TotalReceived,
                    },
                ],
            });
            let rows = rows
                .into_iter()
                .enumerate()
                .map(|(idx, row)| {
                    let rank = limit.saturating_mul(page.saturating_sub(1)) + idx + 1;
                    let (addr_prefix, addr_suffix) = addr_prefix_suffix(&row.address);
                    vec![
                        html! {
                            a class="link mono addr-inline" href=(explorer_path(&format!("/address/{}", row.address))) {
                                span class="addr-rank" { (format!("{rank}.")) }
                                span class="addr-prefix" { (addr_prefix) }
                                span class="addr-suffix" { (addr_suffix) }
                            }
                        },
                        rune_amount_line(entry, row.amount),
                    ]
                })
                .collect();
            let table = if total == 0 {
                html! { div class="alkane-panel" { p class="muted" { "No activity yet." } } }
            } else {
                html! {
                    div class="alkane-panel alkane-holders-card alkane-activity-card" {
                        (holders_table(&["Address", volume_kind.label()], rows))
                    }
                }
            };
            html! {
                div class="alkane-volume-toolbar" {
                    (volume_dropdown)
                }
                (table)
                (pager(total, rows_len, page, limit, |target| {
                    explorer_path(&format!(
                        "/rune/{id}?tab=volume&volume={volume_query}&page={target}&limit={limit}"
                    ))
                }))
            }
        }
        RuneTab::Activity => {
            let offset = limit.saturating_mul(page.saturating_sub(1));
            let page_result = provider
                .get_rune_activity_page(GetRuneActivityPageParams {
                    id: entry.id,
                    address: None,
                    offset,
                    limit,
                    kind: None,
                    scope: activity_filter.storage_scope(),
                    sort_by: activity_order.storage_sort(),
                    dir: activity_dir.storage_dir(),
                    start_time: None,
                    end_time: None,
                })
                .ok();
            let total = page_result.as_ref().map(|res| res.total).unwrap_or(0);
            let entries = page_result.map(|res| res.entries).unwrap_or_default();
            let rows_len = entries.len();
            let activity_sort_dropdown = dropdown(DropdownProps {
                label: Some(activity_order.label().to_string()),
                selected_icon: None,
                aria_label: Some("Sort rune activity".to_string()),
                items: [ActivityOrder::Timestamp, ActivityOrder::Volume]
                    .iter()
                    .map(|opt| DropdownItem {
                        label: opt.label().to_string(),
                        href: activity_tab_url(&id, 1, limit, *opt, activity_dir, activity_filter),
                        icon: None,
                        selected: *opt == activity_order,
                    })
                    .collect(),
            });
            let activity_dir_dropdown = dropdown(DropdownProps {
                label: Some(activity_dir.label().to_string()),
                selected_icon: None,
                aria_label: Some("Rune activity sort direction".to_string()),
                items: [ActivityDir::Asc, ActivityDir::Desc]
                    .iter()
                    .map(|opt| DropdownItem {
                        label: opt.label().to_string(),
                        href: activity_tab_url(
                            &id,
                            1,
                            limit,
                            activity_order,
                            *opt,
                            activity_filter,
                        ),
                        icon: None,
                        selected: *opt == activity_dir,
                    })
                    .collect(),
            });
            let activity_filter_dropdown = dropdown(DropdownProps {
                label: Some(activity_filter.label().to_string()),
                selected_icon: None,
                aria_label: Some("Rune activity filter".to_string()),
                items: [ActivityFilter::All, ActivityFilter::Mint, ActivityFilter::Etch]
                    .iter()
                    .map(|opt| DropdownItem {
                        label: opt.label().to_string(),
                        href: activity_tab_url(&id, 1, limit, activity_order, activity_dir, *opt),
                        icon: None,
                        selected: *opt == activity_filter,
                    })
                    .collect(),
            });
            let rows = entries
                .into_iter()
                .map(|activity| {
                    let mint_price_paid_usd = if matches!(activity.kind, RuneActivityKind::Mint) {
                        format_rune_mint_price_paid_usd(activity.mint_price_paid_usd)
                    } else {
                        None
                    };
                    rune_activity_row(entry, activity, mint_price_paid_usd)
                })
                .collect();
            let table = if total == 0 {
                html! { div class="alkane-panel" { p class="muted" { "No token activity yet." } } }
            } else {
                html! {
                    div class="alkane-panel alkane-token-activity-card" {
                        (holders_table(&["Activity", "Pool", "Tx", "Flow"], rows))
                    }
                }
            };
            html! {
                div class="order-control" {
                    span class="muted" { "Sort by:" }
                    (activity_sort_dropdown)
                    (activity_dir_dropdown)
                    (activity_filter_dropdown)
                }
                (table)
                (pager(total, rows_len, page, limit, |target| {
                    activity_tab_url(
                        &id,
                        target,
                        limit,
                        activity_order,
                        activity_dir,
                        activity_filter,
                    )
                }))
            }
        }
    }
}

fn activity_tab_url(
    id: &str,
    page: usize,
    limit: usize,
    order: ActivityOrder,
    dir: ActivityDir,
    filter: ActivityFilter,
) -> String {
    explorer_path(&format!(
        "/rune/{id}?tab=activity&order={}&dir={}&filter={}&page={page}&limit={limit}",
        order.as_query(),
        dir.as_query(),
        filter.as_query(),
    ))
}

fn pager<F>(total: usize, rows_len: usize, page: usize, limit: usize, url: F) -> Markup
where
    F: Fn(usize) -> String,
{
    let off = limit.saturating_mul(page.saturating_sub(1));
    let has_prev = page > 1;
    let has_next = off + rows_len < total;
    let display_start = if total > 0 && off < total { off + 1 } else { 0 };
    let display_end = (off + rows_len).min(total);
    let last_page = if total > 0 { (total + limit - 1) / limit } else { 1 };
    html! {
        div class="pager" {
            @if has_prev {
                a class="pill iconbtn" href=(url(1)) aria-label="First page" { (icon_pager_first()) }
            } @else {
                span class="pill disabled iconbtn" aria-hidden="true" { (icon_pager_first()) }
            }
            @if has_prev {
                a class="pill iconbtn" href=(url(page - 1)) aria-label="Previous page" { (icon_pager_left()) }
            } @else {
                span class="pill disabled iconbtn" aria-hidden="true" { (icon_pager_left()) }
            }
            span class="pager-meta muted" { "Showing "
                (format_integer(display_start as u128))
                @if total > 0 {
                    "-"
                    (format_integer(display_end as u128))
                }
                " / "
                (format_integer(total as u128))
            }
            @if has_next {
                a class="pill iconbtn" href=(url(page + 1)) aria-label="Next page" { (icon_pager_right()) }
            } @else {
                span class="pill disabled iconbtn" aria-hidden="true" { (icon_pager_right()) }
            }
            @if has_next {
                a class="pill iconbtn" href=(url(last_page)) aria-label="Last page" { (icon_pager_last()) }
            } @else {
                span class="pill disabled iconbtn" aria-hidden="true" { (icon_pager_last()) }
            }
        }
    }
}

fn rune_activity_row(
    entry: &RuneEntry,
    activity: RuneActivity,
    mint_price_paid_usd: Option<String>,
) -> Vec<Markup> {
    let txid = Txid::from_byte_array(activity.txid).to_string();
    let activity_label = match activity.kind {
        RuneActivityKind::Etch => "Etch",
        RuneActivityKind::Mint => "Mint",
    };
    let activity_key = match activity.kind {
        RuneActivityKind::Etch => "etch",
        RuneActivityKind::Mint => "mint",
    };
    let activity_icon = match activity.kind {
        RuneActivityKind::Etch => icon_activity(),
        RuneActivityKind::Mint => icon_activity_mint(),
    };
    let account_markup = if let Some(address) = activity.destination.as_ref() {
        html! {
            a class="link mono alkane-token-activity-account" href=(explorer_path(&format!("/address/{address}"))) {
                (short_hex(address))
            }
        }
    } else {
        html! { span class="muted mono alkane-token-activity-account" { "Unknown" } }
    };
    let (tx_prefix, tx_suffix) = addr_prefix_suffix(&txid);
    vec![
        html! {
            div {
                div class="alkane-meta alkane-token-activity-mobile-head" {
                    a class="link mono alkane-id alkane-token-activity-mobile-tx" href=(explorer_path(&format!("/tx/{txid}"))) {
                        (&txid)
                    }
                }
                div class="alkane-token-activity-summary" {
                    span class=(format!("alkane-token-activity-icon {activity_key}")) aria-hidden="true" {
                        (activity_icon)
                    }
                    div class="alkane-token-activity-summary-copy" {
                        div class="alkane-token-activity-kind-row" {
                            span class="alkane-token-activity-kind-label" { (activity_label) }
                        }
                            div class="alkane-token-activity-meta" {
                                div class="alkane-token-activity-time" data-ts-group="" {
                                    span hidden data-header-ts=(activity.timestamp) { (activity.timestamp) }
                                    span class="muted" data-header-ts-rel data-rel-only title="" { (relative_time_label(activity.timestamp)) }
                                }
                                (account_markup)
                            }
                    }
                }
            }
        },
        html! {
            div class="alkane-token-activity-pool" {
                span class="muted" { "—" }
            }
        },
        html! {
            a class="link mono addr-inline alkane-token-activity-tx" href=(explorer_path(&format!("/tx/{txid}"))) {
                span class="addr-prefix" { (tx_prefix) }
                span class="addr-suffix" { (tx_suffix) }
            }
        },
        html! {
            div class="alkane-token-activity-flow" {
                div class="alkane-token-activity-flow-line in alk-line" {
                    (rune_icon(entry, "alk-icon-wrap"))
                    span class="alk-amt mono" { (format!("+{}", fmt_rune_amount(entry, activity.amount))) }
                    a class="alk-sym link mono" href=(explorer_path(&format!("/rune/{}", entry.id))) {
                        (entry.symbol.clone().unwrap_or_else(|| "¤".to_string()))
                    }
                }
                @if let Some(price_paid_usd) = mint_price_paid_usd.as_ref() {
                    div class="alkane-token-activity-flow-line neutral" {
                        span class="alkane-token-activity-price-paid" {
                            "Price paid: $" (price_paid_usd) " / " (entry.symbol.clone().unwrap_or_else(|| "token".to_string()))
                        }
                    }
                }
            }
        },
    ]
}

fn format_rune_mint_price_paid_usd(mint_price_paid_usd: [u8; 32]) -> Option<String> {
    let scale = U256::from(PRICE_SCALE);
    let usd_scaled = U256::from_be_bytes(mint_price_paid_usd);
    if usd_scaled.is_zero() {
        return None;
    }
    let micros = (usd_scaled
        .saturating_mul(U256::from(1_000_000u32))
        .saturating_add(scale / U256::from(2u8)))
        / scale;
    let whole = micros / U256::from(1_000_000u32);
    let frac = (micros % U256::from(1_000_000u32)).to::<u32>();
    Some(format!("{whole}.{:06}", frac))
}

fn relative_time_label(ts: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(ts);
    let diff = now.saturating_sub(ts);
    let mins = diff / 60;
    let hrs = mins / 60;
    let days = hrs / 24;
    if days > 365 {
        format!("{}y ago", days / 365)
    } else if days > 30 {
        format!("{}mo ago", days / 30)
    } else if days > 0 {
        format!("{days}d ago")
    } else if hrs > 0 {
        format!("{hrs}h ago")
    } else if mins > 0 {
        format!("{mins}m ago")
    } else {
        "just now".to_string()
    }
}

fn addr_prefix_suffix(addr: &str) -> (String, String) {
    let suffix_len = addr.len().min(ADDR_SUFFIX_LEN);
    let split_at = addr.len().saturating_sub(suffix_len);
    let prefix = addr[..split_at].to_string();
    let suffix = addr[split_at..].to_string();
    (prefix, suffix)
}

fn fmt_rune_amount(entry: &RuneEntry, raw: u128) -> String {
    fmt_scaled_amount(raw, entry.divisibility)
}

fn rune_amount_line(entry: &RuneEntry, raw: u128) -> Markup {
    let id = entry.id.to_string();
    let symbol = entry.symbol.clone().unwrap_or_else(|| "¤".to_string());
    html! {
        div class="alk-line" {
            (rune_icon(entry, "alk-icon-wrap"))
            span class="alk-amt mono" { (fmt_rune_amount(entry, raw)) }
            a class="alk-sym link mono" href=(explorer_path(&format!("/rune/{id}"))) { (symbol) }
        }
    }
}

fn short_hex(txid: &str) -> String {
    if txid.len() <= 16 {
        txid.to_string()
    } else {
        format!("{}...{}", &txid[..8], &txid[txid.len() - 8..])
    }
}
