use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicI32, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{Context, Result};
use serde::Serialize;
use static_flow_shared::article_request_store::{
    ArticleRequestRecord, ArticleRequestStore, NewArticleRequestAiRunChunkInput, NewArticleRequestAiRunInput,
    REQUEST_AI_RUN_STATUS_FAILED, REQUEST_AI_RUN_STATUS_SUCCESS, REQUEST_STATUS_APPROVED, REQUEST_STATUS_DONE,
    REQUEST_STATUS_FAILED, REQUEST_STATUS_REJECTED, REQUEST_STATUS_RUNNING,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::mpsc,
    time::timeout,
};

use crate::email::{build_article_detail_url, EmailNotifier};

#[derive(Clone, Debug)]
pub struct ArticleRequestWorkerConfig {
    pub runner_program: String,
    pub runner_args: Vec<String>,
    pub timeout_seconds: u64,
    pub workdir: PathBuf,
    pub content_db_path: String,
    pub skill_path: PathBuf,
    pub result_dir: PathBuf,
    pub cleanup_result_file_on_success: bool,
}

impl ArticleRequestWorkerConfig {
    pub fn from_env(content_db_path: String) -> Self {
        let runner_program =
            env::var("ARTICLE_REQUEST_RUNNER_PROGRAM").unwrap_or_else(|_| "bash".to_string());
        let runner_args = env::var("ARTICLE_REQUEST_RUNNER_ARGS")
            .ok()
            .map(|v| {
                v.split_whitespace()
                    .map(str::to_string)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| vec!["scripts/article_request_worker_runner.sh".to_string()]);
        let timeout_seconds = env::var("ARTICLE_REQUEST_TIMEOUT_SECONDS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(3600)
            .max(30);
        let workdir = env::var("ARTICLE_REQUEST_WORKDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let skill_path = env::var("ARTICLE_REQUEST_SKILL_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| workdir.join("skills/external-blog-repost-publisher/SKILL.md"));
        let result_dir = env::var("ARTICLE_REQUEST_RESULT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp/staticflow-article-request-results"));
        let cleanup_result_file_on_success = env::var("ARTICLE_REQUEST_RESULT_CLEANUP_ON_SUCCESS")
            .ok()
            .map(|v| parse_bool_env(&v))
            .unwrap_or(true);

        Self {
            runner_program,
            runner_args,
            timeout_seconds,
            workdir,
            content_db_path,
            skill_path,
            result_dir,
            cleanup_result_file_on_success,
        }
    }
}

#[derive(Debug, Serialize)]
struct ArticleRequestWorkerPayload<'a> {
    request_id: &'a str,
    article_url: &'a str,
    title_hint: Option<&'a str>,
    request_message: &'a str,
    content_db_path: &'a str,
    skill_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_request_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_context: Option<Vec<ParentContextEntry>>,
}

#[derive(Debug, Serialize)]
struct ParentContextEntry {
    request_id: String,
    article_url: String,
    request_message: String,
    ingested_article_id: Option<String>,
    ai_reply: Option<String>,
}

#[derive(Debug)]
struct RunnerProcessOutput {
    #[allow(dead_code)]
    success: bool,
    exit_code: Option<i32>,
    #[allow(dead_code)]
    stdout: String,
    #[allow(dead_code)]
    stderr: String,
    result_file_path: PathBuf,
}

const RUN_CHUNK_MAX_SEGMENTS: usize = 4096;

pub fn spawn_article_request_worker(
    store: Arc<ArticleRequestStore>,
    config: ArticleRequestWorkerConfig,
    email_notifier: Option<Arc<EmailNotifier>>,
) -> mpsc::Sender<String> {
    let (sender, mut receiver) = mpsc::channel::<String>(128);
    tokio::spawn(async move {
        while let Some(request_id) = receiver.recv().await {
            if let Err(err) =
                process_one_request(store.clone(), config.clone(), email_notifier.clone(), &request_id)
                    .await
            {
                tracing::error!("article request worker failed for {request_id}: {err}");
            }
        }
    });
    sender
}

async fn process_one_request(
    store: Arc<ArticleRequestStore>,
    config: ArticleRequestWorkerConfig,
    email_notifier: Option<Arc<EmailNotifier>>,
    request_id: &str,
) -> Result<()> {
    let request = match store.get_request(request_id).await? {
        Some(w) => w,
        None => {
            tracing::warn!("article request worker skipped missing request {request_id}");
            return Ok(());
        },
    };

    if request.status == REQUEST_STATUS_REJECTED || request.status == REQUEST_STATUS_DONE {
        tracing::info!("article request worker skipped finalized request {request_id}");
        return Ok(());
    }

    if request.status == REQUEST_STATUS_APPROVED {
        store
            .transition_request(request_id, REQUEST_STATUS_RUNNING, None, None, None, None)
            .await?;
    } else if request.status != REQUEST_STATUS_RUNNING {
        tracing::warn!("article request worker skipped request {request_id} with status {}", request.status);
        return Ok(());
    }

    let run_id = format!("arrun-{}-{}", request_id, chrono::Utc::now().timestamp_millis());
    if let Err(err) = store
        .create_ai_run(NewArticleRequestAiRunInput {
            run_id: run_id.clone(),
            request_id: request_id.to_string(),
            runner_program: config.runner_program.clone(),
        })
        .await
    {
        let reason = format!("failed to create article request ai run: {err}");
        mark_request_failed(&store, request_id, reason).await;
        return Ok(());
    }

    let run_output = match run_request_runner(store.clone(), &config, &request, &run_id).await {
        Ok(output) => output,
        Err(err) => {
            let reason = err.to_string();
            let is_timeout = reason.contains("timed out");

            // On timeout, check if the result file was already written before giving up.
            // The runner may have completed the actual work (e.g. song ingestion) but
            // exceeded the wall-clock limit during post-verification steps.
            if is_timeout {
                let result_path = build_result_file_path(&config.result_dir, request_id);
                if let Ok(result_json) = read_result_json(&result_path).await {
                    tracing::info!(
                        "article request runner timed out but result file exists for {request_id}, \
                         treating as success"
                    );
                    let ingested_article_id = result_json
                        .get("ingested_article_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let reply_markdown = result_json
                        .get("reply_markdown")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if let Err(e) = store
                        .transition_request(
                            request_id,
                            REQUEST_STATUS_DONE,
                            None,
                            None,
                            ingested_article_id.as_deref(),
                            Some(&reply_markdown),
                        )
                        .await
                    {
                        tracing::warn!("failed to mark timed-out request as done: {e}");
                    }
                    send_request_done_notification(
                        email_notifier.as_ref(),
                        &request,
                        ingested_article_id.as_deref(),
                        &reply_markdown,
                    )
                    .await;
                    let _ = store
                        .finalize_ai_run(
                            &run_id,
                            REQUEST_AI_RUN_STATUS_SUCCESS,
                            None,
                            None,
                            Some(&reply_markdown),
                        )
                        .await;
                    if config.cleanup_result_file_on_success {
                        let _ = tokio::fs::remove_file(&result_path).await;
                    }
                    return Ok(());
                }
            }

            let _ = store
                .finalize_ai_run(&run_id, REQUEST_AI_RUN_STATUS_FAILED, None, Some(&reason), None)
                .await;
            mark_request_failed(&store, request_id, reason).await;
            return Ok(());
        },
    };

    let result_json = match read_result_json(&run_output.result_file_path).await {
        Ok(j) => j,
        Err(err) => {
            let reason = format!(
                "article request result file invalid: {err} path={} exit_code={:?}",
                run_output.result_file_path.display(),
                run_output.exit_code,
            );
            let _ = store
                .finalize_ai_run(
                    &run_id,
                    REQUEST_AI_RUN_STATUS_FAILED,
                    run_output.exit_code,
                    Some(&reason),
                    None,
                )
                .await;
            mark_request_failed(&store, request_id, reason).await;
            return Ok(());
        },
    };

    let ingested_article_id = result_json
        .get("ingested_article_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let reply_markdown = result_json
        .get("reply_markdown")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if let Err(err) = store
        .transition_request(
            request_id,
            REQUEST_STATUS_DONE,
            None,
            None,
            ingested_article_id.as_deref(),
            Some(&reply_markdown),
        )
        .await
    {
        let reason = format!("failed to mark request done: {err}");
        let _ = store
            .finalize_ai_run(
                &run_id,
                REQUEST_AI_RUN_STATUS_FAILED,
                run_output.exit_code,
                Some(&reason),
                Some(&reply_markdown),
            )
            .await;
        mark_request_failed(&store, request_id, reason).await;
        return Ok(());
    }
    send_request_done_notification(
        email_notifier.as_ref(),
        &request,
        ingested_article_id.as_deref(),
        &reply_markdown,
    )
    .await;

    let _ = store
        .finalize_ai_run(
            &run_id,
            REQUEST_AI_RUN_STATUS_SUCCESS,
            run_output.exit_code,
            None,
            Some(&reply_markdown),
        )
        .await;

    if config.cleanup_result_file_on_success {
        let _ = tokio::fs::remove_file(&run_output.result_file_path).await;
    }

    Ok(())
}

async fn run_request_runner(
    store: Arc<ArticleRequestStore>,
    config: &ArticleRequestWorkerConfig,
    request: &ArticleRequestRecord,
    run_id: &str,
) -> Result<RunnerProcessOutput> {
    tokio::fs::create_dir_all(&config.result_dir)
        .await
        .with_context(|| {
            format!("failed to ensure article request result dir {}", config.result_dir.display())
        })?;
    let result_file_path = build_result_file_path(&config.result_dir, &request.request_id);
    let _ = tokio::fs::remove_file(&result_file_path).await;

    let (parent_id, parent_context) = if let Some(ref pid) = request.parent_request_id {
        let chain = store
            .build_parent_context_chain(pid, 5)
            .await
            .unwrap_or_default();
        let entries: Vec<ParentContextEntry> = chain
            .into_iter()
            .map(|r| ParentContextEntry {
                request_id: r.request_id,
                article_url: r.article_url,
                request_message: r.request_message,
                ingested_article_id: r.ingested_article_id,
                ai_reply: r.ai_reply,
            })
            .collect();
        (Some(pid.as_str()), if entries.is_empty() { None } else { Some(entries) })
    } else {
        (None, None)
    };

    let payload = ArticleRequestWorkerPayload {
        request_id: &request.request_id,
        article_url: &request.article_url,
        title_hint: request.title_hint.as_deref(),
        request_message: &request.request_message,
        content_db_path: &config.content_db_path,
        skill_path: config.skill_path.display().to_string(),
        parent_request_id: parent_id,
        parent_context,
    };

    let payload_json =
        serde_json::to_vec_pretty(&payload).context("failed to encode request payload")?;
    let payload_path =
        std::env::temp_dir().join(format!("staticflow-article-request-{}.json", request.request_id));
    tokio::fs::write(&payload_path, payload_json)
        .await
        .with_context(|| format!("failed to write payload {}", payload_path.display()))?;

    let mut command = Command::new(&config.runner_program);
    command.args(config.runner_args.clone());
    command.arg(payload_path.as_os_str());
    command.current_dir(&config.workdir);
    command.env("ARTICLE_REQUEST_SKILL_PATH", &config.skill_path);
    command.env("CONTENT_DB_PATH", &config.content_db_path);
    command.env("ARTICLE_REQUEST_RESULT_DIR", &config.result_dir);
    command.env("ARTICLE_REQUEST_RESULT_PATH", &result_file_path);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .context("failed to execute article request runner command")?;
    let stdout = child.stdout.take().context("missing runner stdout pipe")?;
    let stderr = child.stderr.take().context("missing runner stderr pipe")?;

    let sequence = Arc::new(AtomicI32::new(0));
    let stdout_handle = {
        let store = store.clone();
        let run_id = run_id.to_string();
        let request_id = request.request_id.clone();
        let sequence = sequence.clone();
        tokio::spawn(async move {
            pump_child_stream(store, &run_id, &request_id, "stdout", sequence, stdout).await
        })
    };
    let stderr_handle = {
        let store = store.clone();
        let run_id = run_id.to_string();
        let request_id = request.request_id.clone();
        let sequence = sequence.clone();
        tokio::spawn(async move {
            pump_child_stream(store, &run_id, &request_id, "stderr", sequence, stderr).await
        })
    };

    let status = match timeout(Duration::from_secs(config.timeout_seconds), child.wait()).await {
        Ok(result) => result.context("failed to wait article request runner")?,
        Err(_) => {
            let _ = child.kill().await;
            anyhow::bail!("article request runner timed out");
        },
    };

    let stdout_text = stdout_handle
        .await
        .context("stdout pump join failed")?
        .context("stdout pump failed")?;
    let stderr_text = stderr_handle
        .await
        .context("stderr pump join failed")?
        .context("stderr pump failed")?;

    let _ = tokio::fs::remove_file(&payload_path).await;

    Ok(RunnerProcessOutput {
        success: status.success(),
        exit_code: status.code(),
        stdout: stdout_text,
        stderr: stderr_text,
        result_file_path,
    })
}

async fn read_result_json(path: &Path) -> Result<serde_json::Value> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read result file {}", path.display()))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        anyhow::bail!("result file is empty: {}", path.display());
    }
    serde_json::from_str(trimmed)
        .with_context(|| format!("result file is not valid JSON: {}", path.display()))
}

fn build_result_file_path(result_dir: &Path, request_id: &str) -> PathBuf {
    let safe = sanitize_id_for_path(request_id);
    result_dir.join(format!("request-{safe}.json"))
}

fn sanitize_id_for_path(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for ch in id.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "unknown".to_string()
    } else {
        out
    }
}

async fn pump_child_stream(
    store: Arc<ArticleRequestStore>,
    run_id: &str,
    request_id: &str,
    stream: &str,
    sequence: Arc<AtomicI32>,
    reader: impl tokio::io::AsyncRead + Unpin,
) -> Result<String> {
    let mut lines = BufReader::new(reader).lines();
    let mut collected = String::new();
    let mut accepted = 0usize;

    while let Some(line) = lines.next_line().await? {
        if stream == "stderr"
            && line
                .trim()
                .contains("state db missing rollout path for thread")
        {
            continue;
        }
        if !collected.is_empty() {
            collected.push('\n');
        }
        collected.push_str(&line);

        if accepted >= RUN_CHUNK_MAX_SEGMENTS {
            continue;
        }
        let batch_index = sequence.fetch_add(1, Ordering::Relaxed);
        let chunk_id = format!("{run_id}-{batch_index}");
        if let Err(err) = store
            .append_ai_run_chunk(NewArticleRequestAiRunChunkInput {
                chunk_id,
                run_id: run_id.to_string(),
                request_id: request_id.to_string(),
                stream: stream.to_string(),
                batch_index,
                content: line,
            })
            .await
        {
            tracing::warn!("failed to append article request ai chunk run_id={run_id}: {err}");
        } else {
            accepted += 1;
        }
    }

    Ok(collected)
}

async fn mark_request_failed(store: &ArticleRequestStore, request_id: &str, message: String) {
    let _ = store
        .transition_request(request_id, REQUEST_STATUS_FAILED, None, Some(&message), None, None)
        .await;
}

fn parse_bool_env(raw: &str) -> bool {
    matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "y" | "on")
}

async fn send_request_done_notification(
    notifier: Option<&Arc<EmailNotifier>>,
    request: &ArticleRequestRecord,
    ingested_article_id: Option<&str>,
    reply_markdown: &str,
) {
    let Some(notifier) = notifier else {
        return;
    };
    let Some(requester_email) = request.requester_email.as_deref() else {
        return;
    };

    let article_url_link = match (request.frontend_page_url.as_deref(), ingested_article_id) {
        (Some(frontend_page_url), Some(article_id)) => {
            match build_article_detail_url(frontend_page_url, article_id) {
                Ok(url) => Some(url),
                Err(err) => {
                    tracing::warn!(
                        "failed to build article URL for done request {}: {}",
                        request.request_id,
                        err
                    );
                    None
                },
            }
        },
        _ => None,
    };

    let mut done_request = request.clone();
    done_request.status = REQUEST_STATUS_DONE.to_string();
    done_request.ingested_article_id = ingested_article_id.map(str::to_string);
    done_request.ai_reply = Some(reply_markdown.to_string());
    done_request.requester_email = Some(requester_email.to_string());

    if let Err(err) = notifier
        .send_user_article_request_done_notification(&done_request, article_url_link.as_deref())
        .await
    {
        tracing::warn!("failed to send done notification email for request {}: {}", request.request_id, err);
    }
}
