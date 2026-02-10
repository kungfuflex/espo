use std::collections::HashSet;

use crate::modules::essentials::utils::names::normalize_alkane_name;

pub fn normalize_search_text(raw: &str) -> Option<String> {
    normalize_alkane_name(raw)
}

pub fn collect_search_prefixes(
    names: &[String],
    symbols: &[String],
    min_len: usize,
    max_len: usize,
) -> Vec<String> {
    if min_len == 0 || max_len < min_len {
        return Vec::new();
    }

    let mut out: HashSet<String> = HashSet::new();
    for val in names.iter().chain(symbols.iter()) {
        let Some(norm) = normalize_alkane_name(val) else { continue };
        let chars: Vec<char> = norm.chars().collect();
        let max = std::cmp::min(max_len, chars.len());
        if max < min_len {
            continue;
        }
        for len in min_len..=max {
            let prefix: String = chars[..len].iter().collect();
            out.insert(prefix);
        }
    }
    out.into_iter().collect()
}
