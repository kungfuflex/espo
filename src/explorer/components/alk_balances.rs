use std::collections::HashMap;

use maud::{Markup, html};

use crate::explorer::components::rune_icon::rune_icon;
use crate::explorer::components::tx_view::{AlkaneMetaCache, alkane_meta, icon_bg_style};
use crate::explorer::pages::common::{fmt_alkane_amount, fmt_scaled_amount};
use crate::explorer::paths::explorer_path;
use crate::explorer::phishing::is_phishing_alkane;
use crate::modules::essentials::storage::BalanceEntry;
use crate::modules::runes::storage::{RunesProvider, SchemaRuneId};
use crate::runtime::mdb::Mdb;

pub fn render_alkane_balance_cards(entries: &[BalanceEntry], essentials_mdb: &Mdb) -> Markup {
    if entries.is_empty() {
        return html! {};
    }

    let mut cache: AlkaneMetaCache = HashMap::new();

    html! {
        div class="io-alkanes io-alkanes-grid" {
            @for be in entries {
                @let meta = alkane_meta(&be.alkane, &mut cache, essentials_mdb);
                @let alk = format!("{}:{}", be.alkane.block, be.alkane.tx);
                @let fallback_letter = meta.name.fallback_letter();
                div class="alk-card" {
                    div class="alk-line" {
                        div class="alk-icon-wrap" aria-hidden="true" {
                            span class="alk-icon-img" style=(icon_bg_style(&meta.icon_url)) {}
                            span class="alk-icon-letter" { (fallback_letter) }
                        }
                        span class="alk-amt mono" { (fmt_alkane_amount(be.amount)) }
                        a class="alk-sym link mono" href=(explorer_path(&format!("/alkane/{alk}"))) { (meta.name.value.clone()) }
                        @if is_phishing_alkane(&be.alkane) {
                            span class="tag scam-tag" { "SCAM" }
                        }
                    }
                }
            }
        }
    }
}

pub fn render_rune_balance_cards(
    entries: &[(SchemaRuneId, u128)],
    runes_provider: &RunesProvider,
) -> Markup {
    if entries.is_empty() {
        return html! {};
    }

    html! {
        div class="io-alkanes io-alkanes-grid" {
            @for (id, amount_raw) in entries {
                @let entry = runes_provider.get_rune(*id).ok().flatten();
                @let id_s = id.to_string();
                @let amount = entry
                    .as_ref()
                    .map(|entry| fmt_scaled_amount(*amount_raw, entry.divisibility))
                    .unwrap_or_else(|| amount_raw.to_string());
                @let label = entry
                    .as_ref()
                    .map(|entry| entry.spaced_name.clone())
                    .unwrap_or_else(|| id_s.clone());
                @let symbol = entry
                    .as_ref()
                    .and_then(|entry| entry.symbol.clone())
                    .unwrap_or_else(|| "¤".to_string());
                div class="alk-card" {
                    div class="alk-line" {
                        @if let Some(entry) = entry.as_ref() {
                            (rune_icon(entry, "alk-icon-wrap"))
                        } @else {
                            div class="alk-icon-wrap" aria-hidden="true" {
                                span class="alk-icon-letter" { (symbol) }
                            }
                        }
                        span class="alk-amt mono" { (amount) }
                        a class="alk-sym link mono" href=(explorer_path(&format!("/rune/{id_s}"))) {
                            (label)
                        }
                    }
                }
            }
        }
    }
}
