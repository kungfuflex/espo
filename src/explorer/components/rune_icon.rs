use maud::{Markup, html};

use crate::explorer::paths::explorer_path;
use crate::modules::runes::storage::RuneEntry;

pub fn rune_icon(entry: &RuneEntry, class: &str) -> Markup {
    let id = entry.id.to_string();
    let symbol = entry.symbol.clone().unwrap_or_else(|| "¤".to_string());
    let src = explorer_path(&format!("/static/rune-icons/{id}"));
    html! {
        span class=(format!("{class} rune-icon-frame")) {
            img class="rune-icon-image" src=(src) alt="" loading="lazy" onerror="this.remove();this.nextElementSibling.hidden=false;";
            span class="rune-icon-symbol" hidden { (symbol) }
        }
    }
}
