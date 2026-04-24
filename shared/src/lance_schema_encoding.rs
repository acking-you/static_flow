use std::collections::HashMap;

use arrow_schema::{DataType, Field};

pub(crate) const COMPRESSION_META_KEY: &str = "lance-encoding:compression";
pub(crate) const COMPRESSION_LEVEL_META_KEY: &str = "lance-encoding:compression-level";
pub(crate) const DICT_DIVISOR_META_KEY: &str = "lance-encoding:dict-divisor";
pub(crate) const DICT_SIZE_RATIO_META_KEY: &str = "lance-encoding:dict-size-ratio";
pub(crate) const DICT_VALUES_COMPRESSION_META_KEY: &str = "lance-encoding:dict-values-compression";
pub(crate) const DICT_VALUES_COMPRESSION_LEVEL_META_KEY: &str =
    "lance-encoding:dict-values-compression-level";
pub(crate) const HEAVY_TEXT_COMPRESSION_SCHEME: &str = "zstd";
pub(crate) const HEAVY_TEXT_COMPRESSION_LEVEL: i32 = 6;
pub(crate) const LOW_CARDINALITY_DICT_DIVISOR: u64 = 8;
pub(crate) const LOW_CARDINALITY_DICT_SIZE_RATIO: f64 = 0.98;

fn low_cardinality_metadata() -> HashMap<String, String> {
    let mut metadata = HashMap::new();
    metadata.insert(DICT_DIVISOR_META_KEY.to_string(), LOW_CARDINALITY_DICT_DIVISOR.to_string());
    metadata
        .insert(DICT_SIZE_RATIO_META_KEY.to_string(), LOW_CARDINALITY_DICT_SIZE_RATIO.to_string());
    metadata
}

pub(crate) fn compressed_utf8_field(name: &str, nullable: bool) -> Field {
    let mut metadata = HashMap::new();
    metadata.insert(COMPRESSION_META_KEY.to_string(), HEAVY_TEXT_COMPRESSION_SCHEME.to_string());
    metadata
        .insert(COMPRESSION_LEVEL_META_KEY.to_string(), HEAVY_TEXT_COMPRESSION_LEVEL.to_string());
    Field::new(name, DataType::Utf8, nullable).with_metadata(metadata)
}

pub(crate) fn low_cardinality_utf8_field(name: &str, nullable: bool) -> Field {
    let mut metadata = low_cardinality_metadata();
    metadata.insert(
        DICT_VALUES_COMPRESSION_META_KEY.to_string(),
        HEAVY_TEXT_COMPRESSION_SCHEME.to_string(),
    );
    metadata.insert(
        DICT_VALUES_COMPRESSION_LEVEL_META_KEY.to_string(),
        HEAVY_TEXT_COMPRESSION_LEVEL.to_string(),
    );
    Field::new(name, DataType::Utf8, nullable).with_metadata(metadata)
}
