use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_ENCODING, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use maud::{Markup, html};
use serde::Deserialize;
use std::sync::Arc;

use crate::explorer::components::layout::layout_with_meta;
use crate::explorer::components::rune_icon::rune_icon;
use crate::explorer::pages::state::ExplorerState;
use crate::explorer::paths::explorer_path;
use crate::modules::runes::storage::{RuneEntry, RunesProvider};

#[derive(Deserialize)]
pub struct PageQuery {
    pub tab: Option<String>,
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
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let tab = RuneTab::from_query(q.tab.as_deref());
    let holders = provider.get_holders_count(entry.id).unwrap_or(0);

    layout_with_meta(
        &entry.spaced_name,
        &format!("/rune/{}", entry.id),
        Some("Rune details"),
        html! {
            section class="page-section" {
                div class="token-hero" {
                    (rune_icon(&entry, "token-icon"))
                    div {
                        div class="pill" { "RUNE" }
                        h1 class="mono" { (entry.spaced_name.clone()) }
                        div class="muted mono" { (entry.id.to_string()) }
                    }
                }
                div class="stat-grid" {
                    div { span class="muted" { "Supply" } strong class="mono" { (entry.supply()) } }
                    div { span class="muted" { "Holders" } strong class="mono" { (holders) } }
                    div { span class="muted" { "Mints" } strong class="mono" { (entry.mints) } }
                    div { span class="muted" { "Burned" } strong class="mono" { (entry.burned) } }
                }
                div class="alkane-tab-list" {
                    (tab_link(&entry, RuneTab::Holders, tab))
                    (tab_link(&entry, RuneTab::Volume, tab))
                    (tab_link(&entry, RuneTab::Activity, tab))
                }
                (tab_body(&provider, &entry, tab, page, limit))
            }
        },
    )
    .into_response()
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

fn tab_link(entry: &RuneEntry, tab: RuneTab, active: RuneTab) -> Markup {
    let id = entry.id.to_string();
    let label = match tab {
        RuneTab::Holders => "Holders",
        RuneTab::Volume => "Volume",
        RuneTab::Activity => "Activity",
    };
    let class = if tab == active { "tab-link active" } else { "tab-link" };
    html! {
        a class=(class) href=(explorer_path(&format!("/rune/{id}?tab={}", tab.as_query()))) { (label) }
    }
}

fn tab_body(
    provider: &RunesProvider,
    entry: &RuneEntry,
    tab: RuneTab,
    page: usize,
    limit: usize,
) -> Markup {
    match tab {
        RuneTab::Holders => {
            let rows = provider.get_holders(entry.id, page, limit).unwrap_or_default();
            html! {
                table class="holders_table table" {
                    thead { tr { th { "Address" } th { "Amount" } } }
                    tbody {
                        @for (address, amount) in rows {
                            tr {
                                td class="mono" { a class="link" href=(explorer_path(&format!("/address/{address}"))) { (address) } }
                                td class="mono" { (amount) }
                            }
                        }
                    }
                }
            }
        }
        RuneTab::Volume => html! {
            p class="muted" { "Rune volume indexing is not populated yet." }
        },
        RuneTab::Activity => {
            let rows = provider.get_mint_activity(entry.id, page, limit).unwrap_or_default();
            html! {
                table class="holders_table table" {
                    thead { tr { th { "Mint" } th { "Transaction" } th { "Height" } } }
                    tbody {
                        @for row in rows {
                            @let txid = hex::encode(row.txid);
                            tr {
                                td class="mono" { (row.amount) }
                                td class="mono" { a class="link" href=(explorer_path(&format!("/tx/{txid}"))) { (txid) } }
                                td class="mono" { (row.height) }
                            }
                        }
                    }
                }
            }
        }
    }
}
