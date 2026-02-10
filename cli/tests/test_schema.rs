#[cfg(test)]
mod tests {
    use arrow_array::{Array, FixedSizeListArray, Int32Array, ListArray, StringArray};
    use arrow_schema::{DataType, TimeUnit};
    use sf_cli::schema::{self, ArticleRecord};
    use static_flow_shared::embedding::{IMAGE_VECTOR_DIM, TEXT_VECTOR_DIM_EN, TEXT_VECTOR_DIM_ZH};

    #[test]
    fn article_schema_has_expected_fields() {
        let schema = schema::article_schema();
        assert_eq!(schema.fields().len(), 14);

        let id_field = schema.field_with_name("id").expect("id field");
        assert_eq!(id_field.data_type(), &DataType::Utf8);
        assert!(!id_field.is_nullable());

        let tags_field = schema.field_with_name("tags").expect("tags field");
        match tags_field.data_type() {
            DataType::List(field) => {
                assert_eq!(field.name(), "item");
                assert_eq!(field.data_type(), &DataType::Utf8);
                assert!(field.is_nullable());
            },
            other => panic!("unexpected tags type: {other:?}"),
        }

        let vector_en = schema
            .field_with_name("vector_en")
            .expect("vector_en field");
        match vector_en.data_type() {
            DataType::FixedSizeList(field, size) => {
                assert_eq!(*size as usize, TEXT_VECTOR_DIM_EN);
                assert_eq!(field.data_type(), &DataType::Float32);
            },
            other => panic!("unexpected vector_en type: {other:?}"),
        }

        let vector_zh = schema
            .field_with_name("vector_zh")
            .expect("vector_zh field");
        match vector_zh.data_type() {
            DataType::FixedSizeList(field, size) => {
                assert_eq!(*size as usize, TEXT_VECTOR_DIM_ZH);
                assert_eq!(field.data_type(), &DataType::Float32);
            },
            other => panic!("unexpected vector_zh type: {other:?}"),
        }

        let created_at = schema
            .field_with_name("created_at")
            .expect("created_at field");
        assert_eq!(created_at.data_type(), &DataType::Timestamp(TimeUnit::Millisecond, None));
    }

    #[test]
    fn image_schema_has_expected_fields() {
        let schema = schema::image_schema();
        assert_eq!(schema.fields().len(), 7);

        let data = schema.field_with_name("data").expect("data field");
        assert_eq!(data.data_type(), &DataType::Binary);
        assert!(!data.is_nullable());

        let thumbnail = schema
            .field_with_name("thumbnail")
            .expect("thumbnail field");
        assert_eq!(thumbnail.data_type(), &DataType::Binary);
        assert!(thumbnail.is_nullable());

        let vector = schema.field_with_name("vector").expect("vector field");
        match vector.data_type() {
            DataType::FixedSizeList(field, size) => {
                assert_eq!(*size as usize, IMAGE_VECTOR_DIM);
                assert_eq!(field.data_type(), &DataType::Float32);
            },
            other => panic!("unexpected vector type: {other:?}"),
        }
    }

    #[test]
    fn build_article_batch_builds_expected_rows() {
        let records = vec![
            ArticleRecord {
                id: "post-1".to_string(),
                title: "Title One".to_string(),
                content: "Content One".to_string(),
                summary: "Summary One".to_string(),
                tags: vec!["rust".to_string(), "cli".to_string()],
                category: "Tech".to_string(),
                author: "Ada".to_string(),
                date: "2024-01-01".to_string(),
                featured_image: Some("hero.jpg".to_string()),
                read_time: 3,
                vector_en: Some(vec![0.1; TEXT_VECTOR_DIM_EN]),
                vector_zh: None,
                created_at: 1,
                updated_at: 2,
            },
            ArticleRecord {
                id: "post-2".to_string(),
                title: "Title Two".to_string(),
                content: "Content Two".to_string(),
                summary: "Summary Two".to_string(),
                tags: vec!["wasm".to_string()],
                category: "Web".to_string(),
                author: "Grace".to_string(),
                date: "2024-02-01".to_string(),
                featured_image: None,
                read_time: 5,
                vector_en: None,
                vector_zh: Some(vec![0.2; TEXT_VECTOR_DIM_ZH]),
                created_at: 3,
                updated_at: 4,
            },
        ];

        let batch = schema::build_article_batch(&records).expect("build article batch");
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.schema().as_ref(), schema::article_schema().as_ref());

        let id_idx = batch.schema().index_of("id").expect("id column");
        let id_array = batch
            .column(id_idx)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("id array");
        assert_eq!(id_array.value(0), "post-1");
        assert_eq!(id_array.value(1), "post-2");

        let read_time_idx = batch
            .schema()
            .index_of("read_time")
            .expect("read_time column");
        let read_time_array = batch
            .column(read_time_idx)
            .as_any()
            .downcast_ref::<Int32Array>()
            .expect("read_time array");
        assert_eq!(read_time_array.value(0), 3);
        assert_eq!(read_time_array.value(1), 5);

        let featured_idx = batch
            .schema()
            .index_of("featured_image")
            .expect("featured_image column");
        let featured_array = batch
            .column(featured_idx)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("featured_image array");
        assert_eq!(featured_array.value(0), "hero.jpg");
        assert!(featured_array.is_null(1));

        let vector_en_idx = batch
            .schema()
            .index_of("vector_en")
            .expect("vector_en column");
        let vector_en_array = batch
            .column(vector_en_idx)
            .as_any()
            .downcast_ref::<FixedSizeListArray>()
            .expect("vector_en array");
        assert_eq!(vector_en_array.value_length() as usize, TEXT_VECTOR_DIM_EN);
        assert!(!vector_en_array.is_null(0));
        assert!(vector_en_array.is_null(1));

        let vector_zh_idx = batch
            .schema()
            .index_of("vector_zh")
            .expect("vector_zh column");
        let vector_zh_array = batch
            .column(vector_zh_idx)
            .as_any()
            .downcast_ref::<FixedSizeListArray>()
            .expect("vector_zh array");
        assert_eq!(vector_zh_array.value_length() as usize, TEXT_VECTOR_DIM_ZH);
        assert!(vector_zh_array.is_null(0));
        assert!(!vector_zh_array.is_null(1));

        let tags_idx = batch.schema().index_of("tags").expect("tags column");
        let tags_array = batch
            .column(tags_idx)
            .as_any()
            .downcast_ref::<ListArray>()
            .expect("tags array");
        let tags_value = tags_array.value(0);
        let tags_values = tags_value
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("tags values");
        let tags = (0..tags_values.len())
            .map(|idx| tags_values.value(idx).to_string())
            .collect::<Vec<_>>();
        assert_eq!(tags, vec!["rust".to_string(), "cli".to_string()]);
    }

    #[test]
    fn taxonomy_schema_has_expected_fields() {
        let schema = schema::taxonomy_schema();
        assert_eq!(schema.fields().len(), 7);

        let id = schema.field_with_name("id").expect("id field");
        assert_eq!(id.data_type(), &DataType::Utf8);

        let kind = schema.field_with_name("kind").expect("kind field");
        assert_eq!(kind.data_type(), &DataType::Utf8);

        let key = schema.field_with_name("key").expect("key field");
        assert_eq!(key.data_type(), &DataType::Utf8);

        let description = schema
            .field_with_name("description")
            .expect("description field");
        assert_eq!(description.data_type(), &DataType::Utf8);
        assert!(description.is_nullable());
    }
}
