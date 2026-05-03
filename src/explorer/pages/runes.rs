use axum::extract::{Query, State};
use axum::response::Html;
use maud::{Markup, html};
use serde::Deserialize;
use std::sync::Arc;

use crate::explorer::components::dropdown::{DropdownItem, DropdownProps, dropdown};
use crate::explorer::components::layout::layout_with_meta;
use crate::explorer::components::svg_assets::{
    icon_pager_first, icon_pager_last, icon_pager_left, icon_pager_right,
};
use crate::explorer::components::table::runes_table;
use crate::explorer::pages::common::format_integer;
use crate::explorer::pages::state::ExplorerState;
use crate::explorer::paths::explorer_path;
use crate::modules::runes::storage::RunesProvider;

#[derive(Deserialize)]
pub struct PageQuery {
    pub page: Option<usize>,
    pub limit: Option<usize>,
    pub order: Option<String>,
    pub dir: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortField {
    Age,
    Holders,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortDir {
    Desc,
    Asc,
}

impl SortField {
    fn from_query(order: Option<&str>) -> Self {
        match order {
            Some("age") | Some("age_desc") | Some("age_asc") => Self::Age,
            _ => Self::Holders,
        }
    }

    fn as_query(self) -> &'static str {
        match self {
            Self::Age => "age",
            Self::Holders => "holders",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Age => "Age",
            Self::Holders => "Holder Count",
        }
    }
}

impl SortDir {
    fn from_query(order: Option<&str>, dir: Option<&str>) -> Self {
        match order {
            Some("age_asc") | Some("holders_asc") => Self::Asc,
            Some("age_desc") | Some("holders_desc") => Self::Desc,
            _ => match dir {
                Some("asc") => Self::Asc,
                _ => Self::Desc,
            },
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
}

fn runes_url_with_order(page: usize, limit: usize, field: SortField, dir: SortDir) -> String {
    explorer_path(&format!(
        "/runes?page={page}&limit={limit}&order={}&dir={}",
        field.as_query(),
        dir.as_query()
    ))
}

pub async fn runes_page(
    State(state): State<ExplorerState>,
    Query(q): Query<PageQuery>,
) -> Html<String> {
    let page = q.page.unwrap_or(1).max(1);
    let limit = q.limit.unwrap_or(50).clamp(1, 50);
    let provider = RunesProvider::new(Arc::new(state.runes_mdb.clone()));
    let field = SortField::from_query(q.order.as_deref());
    let dir = SortDir::from_query(q.order.as_deref(), q.dir.as_deref());
    let desc = dir == SortDir::Desc;
    let rows = match field {
        SortField::Age => provider.get_runes_by_age(page, limit, desc).unwrap_or_default(),
        SortField::Holders => provider.get_runes_by_holders(page, limit, desc).unwrap_or_default(),
    };
    let total = provider.get_rune_count().unwrap_or(0) as usize;
    let offset = limit.saturating_mul(page.saturating_sub(1));
    let display_start = if total > 0 && offset < total { offset + 1 } else { 0 };
    let display_end = (offset + rows.len()).min(total);
    let has_prev = page > 1;
    let has_next = display_end < total;
    let last_page = if total > 0 { (total + limit - 1) / limit } else { 1 };
    let show_creation_block = has_prev || has_next;

    let field_options = [SortField::Age, SortField::Holders];
    let field_dropdown = dropdown(DropdownProps {
        label: Some(field.label().to_string()),
        selected_icon: None,
        items: field_options
            .iter()
            .map(|opt| DropdownItem {
                label: opt.label().to_string(),
                href: runes_url_with_order(1, limit, *opt, dir),
                icon: None,
                selected: *opt == field,
            })
            .collect(),
        aria_label: Some("Order runes".to_string()),
    });
    let dir_options = [SortDir::Asc, SortDir::Desc];
    let dir_dropdown = dropdown(DropdownProps {
        label: Some(dir.label().to_string()),
        selected_icon: None,
        items: dir_options
            .iter()
            .map(|opt| DropdownItem {
                label: opt.label().to_string(),
                href: runes_url_with_order(1, limit, field, *opt),
                icon: None,
                selected: *opt == dir,
            })
            .collect(),
        aria_label: Some("Order direction".to_string()),
    });

    let table: Markup = if rows.is_empty() {
        html! { p class="muted" { "No runes found." } }
    } else {
        html! { div class="alkanes-card" { (runes_table(&rows, true, show_creation_block, true)) } }
    };

    layout_with_meta(
        "Runes",
        "/runes",
        None,
        html! {
            div class="row" {
                h1 class="h1" { "All Runes" }
                div class="order-control" {
                    span class="muted" { "Order by:" }
                    (field_dropdown)
                    (dir_dropdown)
                }
            }
            (table)
            div class="pager" {
                @if has_prev {
                    a class="pill iconbtn" href=(runes_url_with_order(1, limit, field, dir)) aria-label="First page" { (icon_pager_first()) }
                } @else {
                    span class="pill disabled iconbtn" aria-hidden="true" { (icon_pager_first()) }
                }
                @if has_prev {
                    a class="pill iconbtn" href=(runes_url_with_order(page - 1, limit, field, dir)) aria-label="Previous page" { (icon_pager_left()) }
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
                    a class="pill iconbtn" href=(runes_url_with_order(page + 1, limit, field, dir)) aria-label="Next page" { (icon_pager_right()) }
                } @else {
                    span class="pill disabled iconbtn" aria-hidden="true" { (icon_pager_right()) }
                }
                @if has_next {
                    a class="pill iconbtn" href=(runes_url_with_order(last_page, limit, field, dir)) aria-label="Last page" { (icon_pager_last()) }
                } @else {
                    span class="pill disabled iconbtn" aria-hidden="true" { (icon_pager_last()) }
                }
            }
        },
    )
}
