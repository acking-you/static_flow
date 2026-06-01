//! Pure JSON scalar helpers: optional-field readers/writers and the
//! deserialization helper shared by the row decoders.

use anyhow::Context;

pub fn decode_optional_json<T: serde::de::DeserializeOwned>(value: Option<&str>) -> Option<T> {
    value.and_then(|raw| serde_json::from_str(raw).ok())
}

pub fn optional_json_string(value: &serde_json::Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub fn optional_json_string_any(value: &serde_json::Value, fields: &[&str]) -> Option<String> {
    fields
        .iter()
        .find_map(|field| optional_json_string(value, field))
}

pub fn optional_json_bool_any(value: &serde_json::Value, fields: &[&str]) -> Option<bool> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(serde_json::Value::as_bool))
}

pub fn optional_json_u64_any(value: &serde_json::Value, fields: &[&str]) -> Option<u64> {
    fields.iter().find_map(|field| {
        value
            .get(*field)
            .and_then(serde_json::Value::as_u64)
            .or_else(|| {
                value
                    .get(*field)
                    .and_then(serde_json::Value::as_i64)
                    .and_then(non_negative_i64_to_u64)
            })
    })
}

pub fn optional_json_i64_any(value: &serde_json::Value, fields: &[&str]) -> Option<i64> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(serde_json::Value::as_i64))
}

pub fn optional_json_f64_any(value: &serde_json::Value, fields: &[&str]) -> Option<f64> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(serde_json::Value::as_f64))
}

pub fn set_json_optional_string(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<String>,
) {
    match value {
        Some(value) => {
            object.insert(key.to_string(), serde_json::Value::String(value));
        },
        None => {
            object.remove(key);
        },
    }
}

pub fn set_json_optional_bool(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<bool>,
) {
    match value {
        Some(value) => {
            object.insert(key.to_string(), serde_json::Value::Bool(value));
        },
        None => {
            object.remove(key);
        },
    }
}

pub fn set_json_optional_u64(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<u64>,
) {
    match value {
        Some(value) => {
            object.insert(key.to_string(), serde_json::Value::Number(value.into()));
        },
        None => {
            object.remove(key);
        },
    }
}

pub fn set_json_optional_f64(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<f64>,
) -> anyhow::Result<()> {
    match value {
        Some(value) => {
            let number =
                serde_json::Number::from_f64(value).context("serialize finite JSON number")?;
            object.insert(key.to_string(), serde_json::Value::Number(number));
        },
        None => {
            object.remove(key);
        },
    }
    Ok(())
}

pub fn non_negative_i64_to_u64(value: i64) -> Option<u64> {
    u64::try_from(value.max(0)).ok()
}
