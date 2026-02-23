use std::future::Future;

use crate::config::get_explorer_base_path;
use crate::explorer::i18n::ExplorerLanguage;

tokio::task_local! {
    static REQUEST_LANGUAGE: ExplorerLanguage;
}

pub fn explorer_base_path() -> &'static str {
    get_explorer_base_path()
}

pub fn current_language() -> ExplorerLanguage {
    REQUEST_LANGUAGE.try_with(|lang| *lang).unwrap_or(ExplorerLanguage::English)
}

pub async fn with_language<T>(language: ExplorerLanguage, future: impl Future<Output = T>) -> T {
    REQUEST_LANGUAGE.scope(language, future).await
}

pub fn explorer_path(path: &str) -> String {
    let base = explorer_base_path();
    let resolved = if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let lang_prefix = match current_language() {
        ExplorerLanguage::English => "",
        ExplorerLanguage::Chinese => "/zh",
    };
    let resolved_with_lang = if lang_prefix.is_empty() {
        resolved
    } else if resolved == "/" {
        lang_prefix.to_string()
    } else {
        format!("{lang_prefix}{resolved}")
    };

    if base == "/" {
        return resolved_with_lang;
    }
    if resolved_with_lang == "/" {
        return base.to_string();
    }
    format!("{base}{resolved_with_lang}")
}
