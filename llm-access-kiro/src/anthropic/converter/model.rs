//! Model-id mapping and per-model context-window sizing.


/// Maps an Anthropic model name (e.g. `"claude-sonnet-4-6"`) to the
/// canonical Kiro model identifier. Returns `None` for unrecognized models.
pub fn map_model(model: &str) -> Option<String> {
    let model = model.to_lowercase();
    if model.contains("sonnet") {
        if model.contains("4-6") || model.contains("4.6") {
            Some("claude-sonnet-4.6".to_string())
        } else {
            Some("claude-sonnet-4.5".to_string())
        }
    } else if model.contains("opus") {
        if model.contains("4-8") || model.contains("4.8") {
            Some("claude-opus-4.8".to_string())
        } else if model.contains("4-7") || model.contains("4.7") {
            Some("claude-opus-4.7".to_string())
        } else if model.contains("4-5") || model.contains("4.5") {
            Some("claude-opus-4.5".to_string())
        } else {
            Some("claude-opus-4.6".to_string())
        }
    } else if model.contains("haiku") {
        Some("claude-haiku-4.5".to_string())
    } else {
        None
    }
}

/// Returns the context window size (in tokens) for the given model.
/// 4.6-generation models get 1M; everything else defaults to 200K.
pub fn get_context_window_size(model: &str) -> i32 {
    match map_model(model) {
        Some(mapped)
            if mapped == "claude-sonnet-4.6"
                || mapped == "claude-opus-4.6"
                || mapped == "claude-opus-4.7"
                || mapped == "claude-opus-4.8" =>
        {
            1_000_000
        },
        _ => 200_000,
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_context_window_size_matches_latest_kiro_model_rules() {
        assert_eq!(get_context_window_size("claude-sonnet-4-6"), 1_000_000);
        assert_eq!(get_context_window_size("claude-opus-4-20250514"), 1_000_000);
        assert_eq!(map_model("claude-opus-4-8"), Some("claude-opus-4.8".to_string()));
        assert_eq!(map_model("claude-opus-4.8"), Some("claude-opus-4.8".to_string()));
        assert_eq!(get_context_window_size("claude-opus-4-8"), 1_000_000);
        assert_eq!(map_model("claude-opus-4-7"), Some("claude-opus-4.7".to_string()));
        assert_eq!(get_context_window_size("claude-opus-4-7"), 1_000_000);
        assert_eq!(get_context_window_size("claude-sonnet-4-5-20250929"), 200_000);
    }
}
