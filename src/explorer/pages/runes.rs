use axum::extract::{Query, State};
use axum::response::Html;
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
    pub page: Option<usize>,
    pub limit: Option<usize>,
}

pub async fn runes_page(
    State(state): State<ExplorerState>,
    Query(q): Query<PageQuery>,
) -> Html<String> {
    let page = q.page.unwrap_or(1).max(1);
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let provider = RunesProvider::new(Arc::new(state.runes_mdb.clone()));
    let rows = provider.get_top_runes(page, limit).unwrap_or_default();

    layout_with_meta(
        "Runes",
        "/runes",
        Some("Top Runes by holder count"),
        html! {
            section class="page-section" {
                div class="section-head" {
                    h1 { "Top Runes" }
                }
                div class="table-wrap" {
                    table class="holders_table table alkanes-table" {
                        thead {
                            tr {
                                th { "Rune" }
                                th { "ID" }
                                th { "Supply" }
                                th { "Holders" }
                            }
                        }
                        tbody {
                            @for (entry, holders) in rows {
                                (rune_row(&entry, holders))
                            }
                        }
                    }
                }
            }
        },
    )
}

fn rune_row(entry: &RuneEntry, holders: u64) -> Markup {
    let id = entry.id.to_string();
    html! {
        tr {
            td class="alkane-main-cell" {
                div class="alkane-main" {
                    (rune_icon(entry, "alkane-icon"))
                    div class="alkane-meta" {
                        a class="alk-sym link mono alkane-name-link" href=(explorer_path(&format!("/rune/{id}"))) { (entry.spaced_name.clone()) }
                        div class="muted mono alkane-id" { (entry.name.clone()) }
                    }
                }
            }
            td class="mono" { (id) }
            td class="mono" { (entry.supply()) }
            td class="mono" { (holders) }
        }
    }
}
