use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use static_flow_shared::interactive_store::{
    now_ms, InteractiveAssetRecord, InteractivePageLocaleRecord, InteractivePageRecord,
    InteractivePageStore, INTERACTIVE_PAGE_STATUS_READY, MIRROR_POLICY_WHITELISTED,
    TRANSLATION_SCOPE_ARTICLE_AND_INTERACTIVE, TRANSLATION_SCOPE_ARTICLE_ONLY,
};
use tokio::process::Command;
use url::Url;

use super::write_article::{self, WriteArticleOptions};

pub struct IngestInteractivePageOptions {
    pub url: String,
    pub article_id: String,
    pub file: PathBuf,
    pub summary: String,
    pub tags: String,
    pub category: String,
    pub category_description: String,
    pub content_en_file: PathBuf,
    pub summary_zh_file: Option<PathBuf>,
    pub summary_en_file: Option<PathBuf>,
    pub title: Option<String>,
    pub author: String,
    pub date: Option<String>,
    pub source_lang: String,
    pub capture_script: PathBuf,
    pub capture_manifest: Option<PathBuf>,
    pub capture_dir: Option<PathBuf>,
    pub allow_host: Option<String>,
    pub mirror_policy: String,
    pub auto_optimize: bool,
}

pub struct AddInteractivePageLocaleOptions {
    pub page_id: String,
    pub locale: String,
    pub title: String,
    pub manifest: PathBuf,
}

#[derive(Debug, Deserialize)]
struct CaptureManifest {
    url: String,
    final_url: String,
    title: String,
    entry_path: String,
    source_host: String,
    assets: Vec<CaptureAssetManifest>,
}

#[derive(Debug, Deserialize)]
struct CaptureAssetManifest {
    logical_path: String,
    resolved_url: String,
    kind: String,
    mime_type: String,
    http_status: i32,
    etag: Option<String>,
    last_modified: Option<String>,
    is_entry: bool,
    file: String,
    sha256: String,
}

pub async fn ingest_page(db_path: &Path, opts: IngestInteractivePageOptions) -> Result<()> {
    let capture_manifest_path = match opts.capture_manifest.as_ref() {
        Some(path) => path.clone(),
        None => {
            let capture_dir = prepare_capture_dir(opts.capture_dir.as_ref(), &opts.article_id)?;
            let page_id = format!("ipg-{}", opts.article_id);
            run_capture(
                &opts.capture_script,
                &opts.url,
                &capture_dir,
                &page_id,
                opts.allow_host.as_deref(),
            )
            .await?
        },
    };

    let manifest = read_capture_manifest(&capture_manifest_path)?;
    validate_capture_manifest(&manifest, opts.allow_host.as_deref())?;

    let page_id = format!("ipg-{}", opts.article_id);
    let title_override = opts
        .title
        .clone()
        .or_else(|| non_empty_opt(&manifest.title))
        .or_else(|| filename_title(&opts.file));
    let created_at = now_ms();
    let assets = build_asset_records(&page_id, &manifest, created_at)?;
    let entry_asset = assets
        .iter()
        .find(|asset| asset.is_entry)
        .context("capture manifest missing entry asset")?;
    let page = InteractivePageRecord {
        id: page_id.clone(),
        article_id: opts.article_id.clone(),
        source_url: manifest.final_url.clone(),
        source_host: manifest.source_host.clone(),
        source_lang: opts.source_lang.clone(),
        title: title_override
            .clone()
            .unwrap_or_else(|| manifest.title.clone()),
        status: INTERACTIVE_PAGE_STATUS_READY.to_string(),
        mirror_policy: if opts.mirror_policy.trim().is_empty() {
            MIRROR_POLICY_WHITELISTED.to_string()
        } else {
            opts.mirror_policy.clone()
        },
        translation_scope: TRANSLATION_SCOPE_ARTICLE_ONLY.to_string(),
        entry_asset_id: entry_asset.id.clone(),
        entry_asset_path: entry_asset.logical_path.clone(),
        asset_count: assets.len() as u64,
        content_sha256: entry_asset.content_sha256.clone(),
        created_at,
        updated_at: created_at,
    };

    let interactive_store = InteractivePageStore::connect(&db_path.to_string_lossy()).await?;
    interactive_store.upsert_page(&page).await?;
    interactive_store.replace_assets(&page_id, &assets).await?;

    write_article::run(db_path, &opts.file, WriteArticleOptions {
        id: Some(opts.article_id.clone()),
        summary: Some(opts.summary),
        title_override,
        author_override: Some(opts.author),
        tags: Some(opts.tags),
        category: Some(opts.category),
        category_description: Some(opts.category_description),
        date: opts.date,
        content_en_file: Some(opts.content_en_file),
        summary_zh_file: opts.summary_zh_file,
        summary_en_file: opts.summary_en_file,
        import_local_images: false,
        media_roots: vec![],
        generate_thumbnail: false,
        thumbnail_size: 256,
        vector: None,
        vector_en: None,
        vector_zh: None,
        language: None,
        auto_optimize: opts.auto_optimize,
        article_kind: Some("interactive_repost".to_string()),
        source_url: Some(manifest.final_url.clone()),
        interactive_page_id: Some(page_id),
    })
    .await?;

    tracing::info!(
        article_id = %opts.article_id,
        source_url = %manifest.url,
        final_url = %manifest.final_url,
        assets = assets.len(),
        "interactive page ingested successfully"
    );
    Ok(())
}

pub async fn add_page_locale(db_path: &Path, opts: AddInteractivePageLocaleOptions) -> Result<()> {
    let manifest = read_capture_manifest(&opts.manifest)?;
    validate_capture_manifest(&manifest, None)?;

    let page_id = opts.page_id.trim();
    if page_id.is_empty() {
        bail!("page_id cannot be empty");
    }

    let locale = normalize_locale(&opts.locale);
    if locale.is_empty() {
        bail!("locale cannot be empty");
    }

    let title = opts.title.trim();
    if title.is_empty() {
        bail!("title cannot be empty");
    }

    let interactive_store = InteractivePageStore::connect(&db_path.to_string_lossy()).await?;
    let Some(mut page) = interactive_store.get_page(page_id).await? else {
        bail!("interactive page `{page_id}` not found");
    };

    let created_at = now_ms();
    let assets = build_asset_records(page_id, &manifest, created_at)?;
    let entry_asset = assets
        .iter()
        .find(|asset| asset.is_entry)
        .context("capture manifest missing entry asset")?;

    let locale_record = InteractivePageLocaleRecord {
        id: page_locale_id(page_id, &locale),
        page_id: page_id.to_string(),
        locale: locale.clone(),
        title: title.to_string(),
        entry_asset_id: entry_asset.id.clone(),
        entry_asset_path: entry_asset.logical_path.clone(),
        content_sha256: entry_asset.content_sha256.clone(),
        created_at,
        updated_at: created_at,
    };

    interactive_store
        .delete_assets_with_prefix(page_id, format!("localized/{locale}/").as_str())
        .await?;
    interactive_store.upsert_assets(&assets).await?;
    interactive_store.upsert_page_locale(&locale_record).await?;

    let total_assets = interactive_store.list_assets_for_page(page_id).await?;
    page.asset_count = total_assets.len() as u64;
    page.updated_at = now_ms();

    if locale != normalize_locale(&page.source_lang)
        && page.translation_scope != TRANSLATION_SCOPE_ARTICLE_AND_INTERACTIVE
    {
        page.translation_scope = TRANSLATION_SCOPE_ARTICLE_AND_INTERACTIVE.to_string();
    }
    interactive_store.upsert_page(&page).await?;

    tracing::info!(
        page_id,
        locale,
        assets = assets.len(),
        manifest = %opts.manifest.display(),
        "interactive page locale added successfully"
    );
    Ok(())
}

fn prepare_capture_dir(capture_dir: Option<&PathBuf>, article_id: &str) -> Result<PathBuf> {
    match capture_dir {
        Some(dir) => {
            fs::create_dir_all(dir)
                .with_context(|| format!("failed to create capture dir {}", dir.display()))?;
            Ok(dir.clone())
        },
        None => {
            let dir = std::env::temp_dir().join(format!(
                "sf-interactive-capture-{}-{}",
                article_id,
                now_ms()
            ));
            fs::create_dir_all(&dir)
                .with_context(|| format!("failed to create capture dir {}", dir.display()))?;
            Ok(dir)
        },
    }
}

async fn run_capture(
    script_path: &Path,
    url: &str,
    capture_dir: &Path,
    page_id: &str,
    allow_host: Option<&str>,
) -> Result<PathBuf> {
    let manifest_path = capture_dir.join("capture-manifest.json");
    let mut command = Command::new("node");
    command
        .arg(script_path)
        .arg("--url")
        .arg(url)
        .arg("--out-dir")
        .arg(capture_dir)
        .arg("--page-id")
        .arg(page_id)
        .arg("--manifest")
        .arg(&manifest_path);
    if let Some(host) = allow_host {
        command.arg("--allow-host").arg(host);
    }

    let output = command
        .output()
        .await
        .with_context(|| format!("failed to launch capture script {}", script_path.display()))?;
    if !output.status.success() {
        bail!(
            "capture script failed: status={} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(manifest_path)
}

fn read_capture_manifest(path: &Path) -> Result<CaptureManifest> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read capture manifest {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse capture manifest {}", path.display()))
}

fn validate_capture_manifest(manifest: &CaptureManifest, allow_host: Option<&str>) -> Result<()> {
    if manifest.assets.is_empty() {
        bail!("capture manifest has no assets");
    }
    if !manifest.assets.iter().any(|asset| asset.is_entry) {
        bail!("capture manifest has no entry asset");
    }
    if manifest.entry_path.trim().is_empty() {
        bail!("capture manifest entry_path is empty");
    }
    if let Some(host) = allow_host {
        let final_host = parse_host(&manifest.final_url)?;
        if final_host != host {
            bail!("capture host mismatch: expected `{host}`, got `{final_host}`");
        }
    }
    Ok(())
}

fn build_asset_records(
    page_id: &str,
    manifest: &CaptureManifest,
    timestamp_ms: i64,
) -> Result<Vec<InteractiveAssetRecord>> {
    manifest
        .assets
        .iter()
        .map(|asset| {
            let file_path = PathBuf::from(&asset.file);
            let bytes = fs::read(&file_path).with_context(|| {
                format!("failed to read captured asset {}", file_path.display())
            })?;
            let size_bytes = bytes.len() as u64;
            let sha256 = if asset.sha256.trim().is_empty() {
                sha256_hex(&bytes)
            } else {
                asset.sha256.clone()
            };
            Ok(InteractiveAssetRecord {
                id: asset_id(page_id, &asset.logical_path),
                page_id: page_id.to_string(),
                logical_path: asset.logical_path.clone(),
                resolved_url: asset.resolved_url.clone(),
                kind: asset.kind.clone(),
                mime_type: asset.mime_type.clone(),
                content_sha256: sha256,
                size_bytes,
                is_entry: asset.is_entry,
                http_status: asset.http_status,
                etag: asset.etag.clone(),
                last_modified: asset.last_modified.clone(),
                bytes,
                created_at: timestamp_ms,
                updated_at: timestamp_ms,
            })
        })
        .collect()
}

fn asset_id(page_id: &str, logical_path: &str) -> String {
    format!("ipa-{}", &sha256_hex(format!("{page_id}:{logical_path}").as_bytes())[..20])
}

fn page_locale_id(page_id: &str, locale: &str) -> String {
    format!(
        "ipl-{}",
        &sha256_hex(format!("{page_id}:{}", normalize_locale(locale)).as_bytes())[..20]
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn parse_host(raw_url: &str) -> Result<String> {
    let url = Url::parse(raw_url).with_context(|| format!("invalid URL `{raw_url}`"))?;
    url.host_str()
        .map(|value| value.to_string())
        .context("URL missing host")
}

fn non_empty_opt(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_locale(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn filename_title(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(|value| value.replace('-', " "))
}
