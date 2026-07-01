//! Kiro model -> account-group preference helpers.
//!
//! ```text
//! raw request model
//!        |
//!        +-- exact preference hit --> preferred account set
//!        |
//!        +-- no hit ---------------> existing affinity/order rules
//! ```

use std::collections::BTreeMap;

/// Per-key exact model-name to Kiro account-group preference map.
pub type KiroModelGroupPreferences = BTreeMap<String, String>;

/// Normalize admin-supplied model preference rules.
pub fn normalize_kiro_model_group_preferences(
    input: KiroModelGroupPreferences,
) -> KiroModelGroupPreferences {
    input
        .into_iter()
        .filter_map(|(model, group_id)| {
            let model = model.trim();
            let group_id = group_id.trim();
            (!model.is_empty() && !group_id.is_empty())
                .then(|| (model.to_string(), group_id.to_string()))
        })
        .collect()
}

/// Resolve one exact model-name preference.
pub fn kiro_model_group_preference<'a>(
    preferences: &'a KiroModelGroupPreferences,
    model: &str,
) -> Option<&'a str> {
    preferences.get(model.trim()).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_model_group_preferences_by_trimming_and_dropping_empty_entries() {
        let normalized = normalize_kiro_model_group_preferences(BTreeMap::from([
            (" claude-sonnet-4 ".to_string(), " group-sonnet ".to_string()),
            ("".to_string(), "group-empty-model".to_string()),
            ("claude-opus-4".to_string(), " ".to_string()),
        ]));

        assert_eq!(
            normalized,
            BTreeMap::from([("claude-sonnet-4".to_string(), "group-sonnet".to_string())])
        );
    }

    #[test]
    fn resolves_model_group_preferences_by_exact_model_name() {
        let preferences =
            BTreeMap::from([("claude-sonnet-4".to_string(), "group-sonnet".to_string())]);

        assert_eq!(
            kiro_model_group_preference(&preferences, "claude-sonnet-4"),
            Some("group-sonnet")
        );
        assert_eq!(kiro_model_group_preference(&preferences, "claude-sonnet"), None);
    }
}
