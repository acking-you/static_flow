use std::path::Path;

use anyhow::Result;

use crate::commands::db_manage::{self, QueryRowsOptions};

pub async fn run(db_path: &Path, options: QueryRowsOptions) -> Result<()> {
    db_manage::query_rows(db_path, options).await
}
