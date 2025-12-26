use std::sync::Arc;

use anyhow::Result;
use arrow_array::builder::{
    BinaryBuilder, FixedSizeListBuilder, Float32Builder, Int32Builder, ListBuilder,
    StringBuilder, TimestampMillisecondBuilder,
};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use static_flow_shared::embedding::{
    IMAGE_VECTOR_DIM, TEXT_VECTOR_DIM_EN, TEXT_VECTOR_DIM_ZH,
};

pub struct ArticleRecord {
    pub id: String,
    pub title: String,
    pub content: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub category: String,
    pub author: String,
    pub date: String,
    pub featured_image: Option<String>,
    pub read_time: i32,
    pub vector_en: Option<Vec<f32>>,
    pub vector_zh: Option<Vec<f32>>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct ImageRecord {
    pub id: String,
    pub filename: String,
    pub data: Vec<u8>,
    pub thumbnail: Option<Vec<u8>>,
    pub vector: Vec<f32>,
    pub metadata: String,
    pub created_at: i64,
}

pub fn article_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("summary", DataType::Utf8, false),
        Field::new(
            "tags",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            false,
        ),
        Field::new("category", DataType::Utf8, false),
        Field::new("author", DataType::Utf8, false),
        Field::new("date", DataType::Utf8, false),
        Field::new("featured_image", DataType::Utf8, true),
        Field::new("read_time", DataType::Int32, false),
        Field::new(
            "vector_en",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                TEXT_VECTOR_DIM_EN as i32,
            ),
            true,
        ),
        Field::new(
            "vector_zh",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                TEXT_VECTOR_DIM_ZH as i32,
            ),
            true,
        ),
        Field::new(
            "created_at",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ),
        Field::new(
            "updated_at",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ),
    ]))
}

pub fn image_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("filename", DataType::Utf8, false),
        Field::new("data", DataType::Binary, false),
        Field::new("thumbnail", DataType::Binary, true),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                IMAGE_VECTOR_DIM as i32,
            ),
            false,
        ),
        Field::new("metadata", DataType::Utf8, false),
        Field::new(
            "created_at",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ),
    ]))
}

pub fn build_article_batch(records: &[ArticleRecord]) -> Result<RecordBatch> {
    let mut id_builder = StringBuilder::new();
    let mut title_builder = StringBuilder::new();
    let mut content_builder = StringBuilder::new();
    let mut summary_builder = StringBuilder::new();
    let mut tags_builder = ListBuilder::new(StringBuilder::new());
    let mut category_builder = StringBuilder::new();
    let mut author_builder = StringBuilder::new();
    let mut date_builder = StringBuilder::new();
    let mut featured_builder = StringBuilder::new();
    let mut read_time_builder = Int32Builder::new();
    let mut vector_en_builder =
        FixedSizeListBuilder::new(Float32Builder::new(), TEXT_VECTOR_DIM_EN as i32);
    let mut vector_zh_builder =
        FixedSizeListBuilder::new(Float32Builder::new(), TEXT_VECTOR_DIM_ZH as i32);
    let mut created_at_builder = TimestampMillisecondBuilder::new();
    let mut updated_at_builder = TimestampMillisecondBuilder::new();

    for record in records {
        id_builder.append_value(&record.id);
        title_builder.append_value(&record.title);
        content_builder.append_value(&record.content);
        summary_builder.append_value(&record.summary);

        for tag in &record.tags {
            tags_builder.values().append_value(tag);
        }
        tags_builder.append(true);

        category_builder.append_value(&record.category);
        author_builder.append_value(&record.author);
        date_builder.append_value(&record.date);

        if let Some(featured) = &record.featured_image {
            featured_builder.append_value(featured);
        } else {
            featured_builder.append_null();
        }

        read_time_builder.append_value(record.read_time);

        match &record.vector_en {
            Some(vector) => {
                if vector.len() != TEXT_VECTOR_DIM_EN {
                    anyhow::bail!(
                        "article vector_en length {} does not match {}",
                        vector.len(),
                        TEXT_VECTOR_DIM_EN
                    );
                }
                for value in vector {
                    vector_en_builder.values().append_value(*value);
                }
                vector_en_builder.append(true);
            },
            None => {
                for _ in 0..TEXT_VECTOR_DIM_EN {
                    vector_en_builder.values().append_null();
                }
                vector_en_builder.append(false);
            },
        }

        match &record.vector_zh {
            Some(vector) => {
                if vector.len() != TEXT_VECTOR_DIM_ZH {
                    anyhow::bail!(
                        "article vector_zh length {} does not match {}",
                        vector.len(),
                        TEXT_VECTOR_DIM_ZH
                    );
                }
                for value in vector {
                    vector_zh_builder.values().append_value(*value);
                }
                vector_zh_builder.append(true);
            },
            None => {
                for _ in 0..TEXT_VECTOR_DIM_ZH {
                    vector_zh_builder.values().append_null();
                }
                vector_zh_builder.append(false);
            },
        }

        created_at_builder.append_value(record.created_at);
        updated_at_builder.append_value(record.updated_at);
    }

    let schema = article_schema();
    let arrays: Vec<ArrayRef> = vec![
        Arc::new(id_builder.finish()),
        Arc::new(title_builder.finish()),
        Arc::new(content_builder.finish()),
        Arc::new(summary_builder.finish()),
        Arc::new(tags_builder.finish()),
        Arc::new(category_builder.finish()),
        Arc::new(author_builder.finish()),
        Arc::new(date_builder.finish()),
        Arc::new(featured_builder.finish()),
        Arc::new(read_time_builder.finish()),
        Arc::new(vector_en_builder.finish()),
        Arc::new(vector_zh_builder.finish()),
        Arc::new(created_at_builder.finish()),
        Arc::new(updated_at_builder.finish()),
    ];

    Ok(RecordBatch::try_new(schema, arrays)?)
}

pub fn build_image_batch(records: &[ImageRecord]) -> Result<RecordBatch> {
    let mut id_builder = StringBuilder::new();
    let mut filename_builder = StringBuilder::new();
    let mut data_builder = BinaryBuilder::new();
    let mut thumb_builder = BinaryBuilder::new();
    let mut vector_builder =
        FixedSizeListBuilder::new(Float32Builder::new(), IMAGE_VECTOR_DIM as i32);
    let mut metadata_builder = StringBuilder::new();
    let mut created_at_builder = TimestampMillisecondBuilder::new();

    for record in records {
        id_builder.append_value(&record.id);
        filename_builder.append_value(&record.filename);
        data_builder.append_value(&record.data);

        if let Some(thumb) = &record.thumbnail {
            thumb_builder.append_value(thumb);
        } else {
            thumb_builder.append_null();
        }

        if record.vector.len() != IMAGE_VECTOR_DIM {
            anyhow::bail!(
                "image vector length {} does not match {}",
                record.vector.len(),
                IMAGE_VECTOR_DIM
            );
        }
        for value in &record.vector {
            vector_builder.values().append_value(*value);
        }
        vector_builder.append(true);

        metadata_builder.append_value(&record.metadata);
        created_at_builder.append_value(record.created_at);
    }

    let schema = image_schema();
    let arrays: Vec<ArrayRef> = vec![
        Arc::new(id_builder.finish()),
        Arc::new(filename_builder.finish()),
        Arc::new(data_builder.finish()),
        Arc::new(thumb_builder.finish()),
        Arc::new(vector_builder.finish()),
        Arc::new(metadata_builder.finish()),
        Arc::new(created_at_builder.finish()),
    ];

    Ok(RecordBatch::try_new(schema, arrays)?)
}
