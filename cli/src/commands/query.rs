use std::path::Path;

use anyhow::Result;
use arrow::util::pretty::pretty_format_batches;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};

use crate::db::connect_db;

pub async fn run(db_path: &Path, table: &str, limit: usize) -> Result<()> {
    let db = connect_db(db_path).await?;
    let table = db.open_table(table).execute().await?;

    let batches = table
        .query()
        .limit(limit)
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;

    let formatted = pretty_format_batches(&batches)?;
    tracing::info!("\n{formatted}");
    Ok(())
}
