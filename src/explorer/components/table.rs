use maud::{Markup, html};

use bitcoin::Txid;
use bitcoin::hashes::Hash;

use crate::explorer::components::rune_icon::rune_icon;
use crate::explorer::components::svg_assets::icon_user;
use crate::explorer::components::tx_view::icon_bg_style;
use crate::explorer::paths::explorer_path;
use crate::explorer::phishing::is_phishing_alkane;
use crate::modules::runes::storage::RuneEntry;
use crate::schemas::SchemaAlkaneId;

#[derive(Clone, Debug)]
pub struct AlkaneTableRow {
    pub id: String,
    pub name: String,
    pub holders: u64,
    pub icon_url: String,
    pub fallback: char,
    pub creation_height: u32,
    pub creation_txid: String,
}

fn compact_count(value: u64) -> String {
    const UNITS: &[(u64, &str)] =
        &[(1_000_000_000_000, "t"), (1_000_000_000, "b"), (1_000_000, "m"), (1_000, "k")];
    let Some((divisor, suffix)) = UNITS.iter().find(|(divisor, _)| value >= *divisor) else {
        return value.to_string();
    };
    let scaled = value as f64 / *divisor as f64;
    let formatted = if scaled >= 100.0 {
        format!("{scaled:.0}")
    } else if scaled >= 10.0 {
        format!("{scaled:.1}")
    } else {
        format!("{scaled:.2}")
    };
    let formatted = if formatted.contains('.') {
        formatted.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        formatted
    };
    format!("{formatted}{suffix}")
}

fn comma_count(value: u64) -> String {
    let raw = value.to_string();
    let mut out = String::with_capacity(raw.len() + raw.len() / 3);
    for (idx, ch) in raw.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn holder_count(value: u64, compact: bool) -> String {
    if compact { compact_count(value) } else { comma_count(value) }
}

fn parse_alkane_id(raw: &str) -> Option<SchemaAlkaneId> {
    let (block, tx) = raw.split_once(':')?;
    Some(SchemaAlkaneId { block: block.parse().ok()?, tx: tx.parse().ok()? })
}

fn scam_tag_for_alkane_id(raw: &str) -> Markup {
    if parse_alkane_id(raw).map(|id| is_phishing_alkane(&id)).unwrap_or(false) {
        html! { span class="tag scam-tag" { "SCAM" } }
    } else {
        html! {}
    }
}

/// Table renderer with fixed width last column used for holders lists.
pub fn holders_table(headers: &[&str], rows: Vec<Vec<Markup>>) -> Markup {
    // assumes every row has the same number of cells as headers
    let n = headers.len();

    html! {
        table class="holders_table table" {
            colgroup {
                @for i in 0..n {
                    @if i == n - 1 {
                        col style="width: 300px;";
                    } @else {
                        col;
                    }
                }
            }

            thead {
                tr {
                    @for h in headers {
                        th { (h) }
                    }
                }
            }

            tbody {
                @for row in rows {
                    tr {
                        @for cell in row {
                            td { (cell) }
                        }
                    }
                }
            }
        }
    }
}

/// Simple table renderer without column sizing.
pub fn table(headers: &[&str], rows: Vec<Vec<Markup>>) -> Markup {
    html! {
        table class="table" {
            thead {
                tr {
                    @for h in headers {
                        th { (h) }
                    }
                }
            }
            tbody {
                @for row in rows {
                    tr {
                        @for cell in row {
                            td { (cell) }
                        }
                    }
                }
            }
        }
    }
}

pub fn alkanes_table(
    rows: &[AlkaneTableRow],
    show_header: bool,
    show_creation_block: bool,
    show_holder_icon: bool,
) -> Markup {
    alkanes_table_inner(rows, show_header, show_creation_block, show_holder_icon, false)
}

pub fn alkanes_table_compact_holders(
    rows: &[AlkaneTableRow],
    show_header: bool,
    show_creation_block: bool,
    show_holder_icon: bool,
) -> Markup {
    alkanes_table_inner(rows, show_header, show_creation_block, show_holder_icon, true)
}

fn alkanes_table_inner(
    rows: &[AlkaneTableRow],
    show_header: bool,
    show_creation_block: bool,
    show_holder_icon: bool,
    compact_holders: bool,
) -> Markup {
    let table_class = if show_creation_block {
        "table holders_table alkanes-table has-block"
    } else {
        "table holders_table alkanes-table"
    };
    html! {
        table class=(table_class) {
            colgroup {
                @if show_creation_block {
                    col style="width: 44%;";
                    col style="width: 12%;";
                    col style="width: 30%;";
                    col style="width: 14%;";
                } @else {
                    col style="width: 56%;";
                    col style="width: 28%;";
                    col style="width: 16%;";
                }
            }
            @if show_header {
                thead {
                    tr {
                        th { "Alkane" }
                        @if show_creation_block {
                            th { "Creation block" }
                        }
                        th { "Creation tx" }
                        th class="right" { "Holders" }
                    }
                }
            }
            tbody {
                @for row in rows {
                    tr {
                        td class="alkane-main-cell" {
                            div class="alk-line" {
                                div class="alk-icon-wrap" aria-hidden="true" {
                                    span class="alk-icon-img" style=(icon_bg_style(&row.icon_url)) {}
                                    span class="alk-icon-letter" { (row.fallback) }
                                }
                                div class="alkane-meta" {
                                    a class="alk-sym link mono alkane-name-link" href=(explorer_path(&format!("/alkane/{}", row.id))) { (row.name.clone()) }
                                    (scam_tag_for_alkane_id(&row.id))
                                    div class="muted mono alkane-id" { (row.id.clone()) }
                                }
                            }
                        }
                        @if show_creation_block {
                            td class="alkane-block-cell" {
                                a class="link mono" href=(explorer_path(&format!("/block/{}", row.creation_height))) { (row.creation_height) }
                            }
                        }
                        td class="mono alkane-tx-cell" {
                            a class="link ellipsis alkane-txid" href=(explorer_path(&format!("/tx/{}", row.creation_txid))) { (&row.creation_txid) }
                        }
                        td class="alkane-holders-cell" {
                            span class="mono holders-count" title=(row.holders) {
                                @if show_holder_icon {
                                    (icon_user())
                                }
                                (holder_count(row.holders, compact_holders))
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn runes_table(
    rows: &[(RuneEntry, u64)],
    show_header: bool,
    show_creation_block: bool,
    show_holder_icon: bool,
) -> Markup {
    runes_table_inner(rows, show_header, show_creation_block, show_holder_icon, false)
}

pub fn runes_table_compact_holders(
    rows: &[(RuneEntry, u64)],
    show_header: bool,
    show_creation_block: bool,
    show_holder_icon: bool,
) -> Markup {
    runes_table_inner(rows, show_header, show_creation_block, show_holder_icon, true)
}

fn runes_table_inner(
    rows: &[(RuneEntry, u64)],
    show_header: bool,
    show_creation_block: bool,
    show_holder_icon: bool,
    compact_holders: bool,
) -> Markup {
    let table_class = if show_creation_block {
        "table holders_table alkanes-table has-block"
    } else {
        "table holders_table alkanes-table"
    };
    html! {
        table class=(table_class) {
            colgroup {
                @if show_creation_block {
                    col style="width: 44%;";
                    col style="width: 12%;";
                    col style="width: 30%;";
                    col style="width: 14%;";
                } @else {
                    col style="width: 56%;";
                    col style="width: 28%;";
                    col style="width: 16%;";
                }
            }
            @if show_header {
                thead {
                    tr {
                        th { "Rune" }
                        @if show_creation_block {
                            th { "Creation block" }
                        }
                        th { "Creation tx" }
                        th class="right" { "Holders" }
                    }
                }
            }
            tbody {
                @for (entry, holders) in rows {
                    @let id = entry.id.to_string();
                    @let creation_height = if entry.id.block == 1 && entry.id.tx == 0 {
                        840_000
                    } else {
                        entry.id.block
                    };
                    @let etching_txid = Txid::from_byte_array(entry.etching_txid).to_string();
                    tr {
                        td class="alkane-main-cell" {
                            div class="alk-line" {
                                (rune_icon(entry, "alk-icon-wrap"))
                                div class="alkane-meta" {
                                    a class="alk-sym link mono alkane-name-link" href=(explorer_path(&format!("/rune/{id}"))) { (entry.spaced_name.clone()) }
                                    div class="muted mono alkane-id" { (id.clone()) }
                                }
                            }
                        }
                        @if show_creation_block {
                            td class="alkane-block-cell" {
                                a class="link mono" href=(explorer_path(&format!("/block/{creation_height}"))) { (creation_height) }
                            }
                        }
                        td class="mono alkane-tx-cell" {
                            a class="link ellipsis alkane-txid" href=(explorer_path(&format!("/tx/{etching_txid}"))) { (&etching_txid) }
                        }
                        td class="alkane-holders-cell" {
                            span class="mono holders-count" title=(holders) {
                                @if show_holder_icon {
                                    (icon_user())
                                }
                                (holder_count(*holders, compact_holders))
                            }
                        }
                    }
                }
            }
        }
    }
}
