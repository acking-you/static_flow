use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicI32, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use static_flow_shared::comments_store::{
    CommentDataStore, CommentTaskRecord, NewCommentAiRunChunkInput, NewCommentAiRunInput,
    NewPublishedCommentInput, COMMENT_AI_RUN_STATUS_FAILED, COMMENT_AI_RUN_STATUS_SUCCESS,
    COMMENT_STATUS_APPROVED, COMMENT_STATUS_DONE, COMMENT_STATUS_FAILED, COMMENT_STATUS_REJECTED,
    COMMENT_STATUS_RUNNING,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::mpsc,
    time::timeout,
};

#[derive(Clone, Debug)]
pub struct CommentAiWorkerConfig {
    pub runner_program: String,
    pub runner_args: Vec<String>,
    pub timeout_seconds: u64,
    pub workdir: PathBuf,
    pub comment_author_salt: String,
    pub content_db_path: String,
    pub content_api_base: String,
    pub skill_path: PathBuf,
    pub result_dir: PathBuf,
    pub cleanup_result_file_on_success: bool,
}

impl CommentAiWorkerConfig {
    pub fn from_env(content_db_path: String) -> Self {
        let runner_program =
            env::var("COMMENT_AI_RUNNER_PROGRAM").unwrap_or_else(|_| "bash".to_string());
        let runner_args = env::var("COMMENT_AI_RUNNER_ARGS")
            .ok()
            .map(|value| {
                value
                    .split_whitespace()
                    .map(str::to_string)
                    .filter(|item| !item.is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|items| !items.is_empty())
            .unwrap_or_else(|| vec!["scripts/comment_ai_worker_runner.sh".to_string()]);
        let timeout_seconds = env::var("COMMENT_AI_TIMEOUT_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(180)
            .max(30);
        let workdir = env::var("COMMENT_AI_WORKDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let comment_author_salt =
            env::var("COMMENT_AUTHOR_SALT").unwrap_or_else(|_| "static-flow-comment".to_string());
        let content_api_base = env::var("COMMENT_AI_CONTENT_API_BASE")
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                let port = env::var("PORT").unwrap_or_else(|_| "3000".to_string());
                format!("http://127.0.0.1:{port}/api")
            });
        let skill_path = env::var("COMMENT_AI_SKILL_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| workdir.join("skills/comment-review-ai-responder/SKILL.md"));
        let result_dir = env::var("COMMENT_AI_RESULT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp/staticflow-comment-results"));
        let cleanup_result_file_on_success = env::var("COMMENT_AI_RESULT_CLEANUP_ON_SUCCESS")
            .ok()
            .map(|value| parse_bool_env(&value))
            .unwrap_or(true);

        Self {
            runner_program,
            runner_args,
            timeout_seconds,
            workdir,
            comment_author_salt,
            content_db_path,
            content_api_base,
            skill_path,
            result_dir,
            cleanup_result_file_on_success,
        }
    }
}

#[derive(Debug, Serialize)]
struct WorkerTaskPayload<'a> {
    task_id: &'a str,
    article_id: &'a str,
    entry_type: &'a str,
    comment_text: &'a str,
    selected_text: Option<&'a str>,
    anchor_block_id: Option<&'a str>,
    anchor_context_before: Option<&'a str>,
    anchor_context_after: Option<&'a str>,
    reply_to_comment_id: Option<&'a str>,
    reply_to_comment_text: Option<&'a str>,
    reply_to_ai_reply_markdown: Option<&'a str>,
    content_db_path: &'a str,
    content_api_base: &'a str,
    skill_path: String,
    instructions: &'a str,
}

#[derive(Debug, Deserialize)]
struct WorkerRunnerOutput {
    final_reply_markdown: Option<String>,
    #[allow(dead_code)]
    confidence: Option<f32>,
    #[allow(dead_code)]
    sources: Option<Vec<String>>,
    #[allow(dead_code)]
    decision_notes: Option<String>,
}

#[derive(Debug)]
struct RunnerProcessOutput {
    success: bool,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    result_file_path: PathBuf,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RunnerReplySource {
    Stdout,
    Stderr,
}

const COMMENT_SKILL_HINT: &str = "Use skill comment-review-ai-responder. Fetch article raw \
                                  markdown via local HTTP API first, and fallback to sf-cli \
                                  content-only query when HTTP fails.";
const RUN_CHUNK_MAX_SEGMENTS: usize = 4096;

pub fn spawn_comment_worker(
    store: Arc<CommentDataStore>,
    config: CommentAiWorkerConfig,
) -> mpsc::Sender<String> {
    let (sender, mut receiver) = mpsc::channel::<String>(128);
    tokio::spawn(async move {
        while let Some(task_id) = receiver.recv().await {
            if let Err(err) = process_one_task(store.clone(), config.clone(), &task_id).await {
                tracing::error!("comment worker failed for task {task_id}: {err}");
            }
        }
    });
    sender
}

async fn process_one_task(
    store: Arc<CommentDataStore>,
    config: CommentAiWorkerConfig,
    task_id: &str,
) -> Result<()> {
    let mut task = match store.get_comment_task(task_id).await? {
        Some(task) => task,
        None => {
            tracing::warn!("comment worker skipped missing task {task_id}");
            return Ok(());
        },
    };

    if task.status == COMMENT_STATUS_REJECTED || task.status == COMMENT_STATUS_DONE {
        tracing::info!("comment worker skipped finalized task {task_id}");
        return Ok(());
    }

    if task.status == COMMENT_STATUS_APPROVED {
        let transitioned = store
            .transition_comment_task(task_id, COMMENT_STATUS_RUNNING, None, None, true)
            .await?;
        if let Some(updated) = transitioned {
            task = updated;
        }
    } else if task.status != COMMENT_STATUS_RUNNING {
        tracing::warn!("comment worker skipped task {} with status {}", task.task_id, task.status);
        return Ok(());
    }

    let run_id = generate_ai_run_id(&task.task_id);
    let run_created = store
        .create_ai_run(NewCommentAiRunInput {
            run_id: run_id.clone(),
            task_id: task.task_id.clone(),
            runner_program: config.runner_program.clone(),
            runner_args_json: serde_json::to_string(&config.runner_args).unwrap_or_default(),
            skill_path: config.skill_path.display().to_string(),
        })
        .await;
    if let Err(err) = run_created {
        let reason = format!("failed to create comment ai run record: {err}");
        mark_task_failed(store.as_ref(), task_id, reason).await;
        return Ok(());
    }

    let run_output = match run_ai_runner(store.clone(), &config, &task, &run_id).await {
        Ok(output) => output,
        Err(err) => {
            let reason = err.to_string();
            let _ = store
                .finalize_ai_run(
                    &run_id,
                    COMMENT_AI_RUN_STATUS_FAILED,
                    None,
                    Some(reason.clone()),
                    None,
                )
                .await;
            mark_task_failed(store.as_ref(), task_id, reason).await;
            return Ok(());
        },
    };

    let reply_markdown = match read_comment_result_markdown(&run_output.result_file_path).await {
        Ok(reply) => reply,
        Err(err) => {
            let stdout_diagnostics = inspect_runner_output(&run_output.stdout).summary();
            let stderr_diagnostics = inspect_runner_output(&run_output.stderr).summary();
            let reason = format!(
                "comment ai result file invalid: {err}. result_file={} exit_code={:?} \
                 stdout_diagnostics={stdout_diagnostics} stderr_diagnostics={stderr_diagnostics} \
                 stdout={} stderr={}",
                run_output.result_file_path.display(),
                run_output.exit_code,
                compact_for_reason(&run_output.stdout),
                compact_for_reason(&run_output.stderr)
            );
            let _ = store
                .finalize_ai_run(
                    &run_id,
                    COMMENT_AI_RUN_STATUS_FAILED,
                    run_output.exit_code,
                    Some(reason.clone()),
                    None,
                )
                .await;
            mark_task_failed(store.as_ref(), task_id, reason).await;
            return Ok(());
        },
    };

    if !run_output.success {
        tracing::warn!(
            "comment ai runner exited non-zero for task {} (exit_code={:?}) but result file {} \
             was valid; continuing with file-first success policy",
            task.task_id,
            run_output.exit_code,
            run_output.result_file_path.display()
        );
    }

    let (author_hash, author_name, avatar_seed) =
        derive_author_identity(&task.fingerprint, &config.comment_author_salt);
    let comment_id = format!("cmt-{}-{}", task.task_id, now_ms().unsigned_abs());

    let publish_result = store
        .upsert_published_comment(NewPublishedCommentInput {
            comment_id,
            task_id: task.task_id.clone(),
            article_id: task.article_id.clone(),
            author_name,
            author_avatar_seed: avatar_seed,
            author_hash,
            comment_text: task.comment_text.clone(),
            selected_text: task.selected_text.clone(),
            anchor_block_id: task.anchor_block_id.clone(),
            anchor_context_before: task.anchor_context_before.clone(),
            anchor_context_after: task.anchor_context_after.clone(),
            reply_to_comment_id: task.reply_to_comment_id.clone(),
            reply_to_comment_text: task.reply_to_comment_text.clone(),
            reply_to_ai_reply_markdown: task.reply_to_ai_reply_markdown.clone(),
            ai_reply_markdown: reply_markdown.clone(),
            ip_region: task.ip_region.clone(),
        })
        .await;
    if let Err(err) = publish_result {
        let reason = format!("failed to write published comment: {err}");
        let _ = store
            .finalize_ai_run(
                &run_id,
                COMMENT_AI_RUN_STATUS_FAILED,
                run_output.exit_code,
                Some(reason.clone()),
                Some(reply_markdown),
            )
            .await;
        mark_task_failed(store.as_ref(), task_id, reason).await;
        return Ok(());
    }

    if let Err(err) = store
        .transition_comment_task(task_id, COMMENT_STATUS_DONE, None, None, false)
        .await
    {
        let reason = format!("failed to mark comment task done: {err}");
        let _ = store
            .finalize_ai_run(
                &run_id,
                COMMENT_AI_RUN_STATUS_FAILED,
                run_output.exit_code,
                Some(reason.clone()),
                Some(reply_markdown),
            )
            .await;
        mark_task_failed(store.as_ref(), task_id, reason).await;
        return Ok(());
    }

    let _ = store
        .finalize_ai_run(
            &run_id,
            COMMENT_AI_RUN_STATUS_SUCCESS,
            run_output.exit_code,
            None,
            Some(reply_markdown),
        )
        .await;

    if config.cleanup_result_file_on_success {
        if let Err(err) = tokio::fs::remove_file(&run_output.result_file_path).await {
            tracing::warn!(
                "failed to remove comment ai result file after success task_id={} path={} \
                 err={err}",
                task.task_id,
                run_output.result_file_path.display()
            );
        }
    }

    Ok(())
}

async fn run_ai_runner(
    store: Arc<CommentDataStore>,
    config: &CommentAiWorkerConfig,
    task: &CommentTaskRecord,
    run_id: &str,
) -> Result<RunnerProcessOutput> {
    tokio::fs::create_dir_all(&config.result_dir)
        .await
        .with_context(|| {
            format!("failed to ensure comment ai result dir {}", config.result_dir.display())
        })?;
    let result_file_path = build_comment_result_file_path(&config.result_dir, &task.task_id);
    let _ = tokio::fs::remove_file(&result_file_path).await;

    let payload = WorkerTaskPayload {
        task_id: &task.task_id,
        article_id: &task.article_id,
        entry_type: &task.entry_type,
        comment_text: &task.comment_text,
        selected_text: task.selected_text.as_deref(),
        anchor_block_id: task.anchor_block_id.as_deref(),
        anchor_context_before: task.anchor_context_before.as_deref(),
        anchor_context_after: task.anchor_context_after.as_deref(),
        reply_to_comment_id: task.reply_to_comment_id.as_deref(),
        reply_to_comment_text: task.reply_to_comment_text.as_deref(),
        reply_to_ai_reply_markdown: task.reply_to_ai_reply_markdown.as_deref(),
        content_db_path: &config.content_db_path,
        content_api_base: &config.content_api_base,
        skill_path: config.skill_path.display().to_string(),
        instructions: COMMENT_SKILL_HINT,
    };

    let payload_json =
        serde_json::to_vec_pretty(&payload).context("failed to encode task payload")?;
    let payload_path =
        std::env::temp_dir().join(format!("staticflow-comment-task-{}.json", task.task_id));
    tokio::fs::write(&payload_path, payload_json)
        .await
        .with_context(|| format!("failed to write payload {}", payload_path.display()))?;

    let mut command = Command::new(&config.runner_program);
    command.args(config.runner_args.clone());
    command.arg(payload_path.as_os_str());
    command.current_dir(&config.workdir);
    command.env("COMMENT_AI_SKILL_PATH", &config.skill_path);
    command.env("STATICFLOW_LANCEDB_URI", &config.content_db_path);
    command.env("COMMENT_AI_CONTENT_API_BASE", &config.content_api_base);
    command.env("COMMENT_AI_RESULT_DIR", &config.result_dir);
    command.env("COMMENT_AI_RESULT_PATH", &result_file_path);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .context("failed to execute comment ai runner command")?;
    let stdout = child.stdout.take().context("missing runner stdout pipe")?;
    let stderr = child.stderr.take().context("missing runner stderr pipe")?;

    let sequence = Arc::new(AtomicI32::new(0));
    let stdout_handle = {
        let store = store.clone();
        let run_id = run_id.to_string();
        let task_id = task.task_id.clone();
        let sequence = sequence.clone();
        tokio::spawn(async move {
            pump_child_stream(store, &run_id, &task_id, "stdout", sequence, stdout).await
        })
    };
    let stderr_handle = {
        let store = store.clone();
        let run_id = run_id.to_string();
        let task_id = task.task_id.clone();
        let sequence = sequence.clone();
        tokio::spawn(async move {
            pump_child_stream(store, &run_id, &task_id, "stderr", sequence, stderr).await
        })
    };

    let status = match timeout(Duration::from_secs(config.timeout_seconds), child.wait()).await {
        Ok(result) => result.context("failed to wait comment ai runner command")?,
        Err(_) => {
            let _ = child.kill().await;
            anyhow::bail!("comment ai runner timed out");
        },
    };

    let stdout = stdout_handle
        .await
        .context("stdout pump task join failed")?
        .context("stdout pump failed")?;
    let stderr = stderr_handle
        .await
        .context("stderr pump task join failed")?
        .context("stderr pump failed")?;

    let _ = tokio::fs::remove_file(&payload_path).await;

    Ok(RunnerProcessOutput {
        success: status.success(),
        exit_code: status.code(),
        stdout,
        stderr,
        result_file_path,
    })
}

async fn read_comment_result_markdown(path: &Path) -> Result<String> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read result file {}", path.display()))?;
    let normalized = raw.trim();
    if normalized.is_empty() {
        anyhow::bail!("result file is empty: {}", path.display());
    }
    Ok(normalized.to_string())
}

fn build_comment_result_file_path(result_dir: &Path, task_id: &str) -> PathBuf {
    let safe_task_id = sanitize_task_id_for_path(task_id);
    result_dir.join(format!("task-{safe_task_id}.md"))
}

fn sanitize_task_id_for_path(task_id: &str) -> String {
    let mut out = String::with_capacity(task_id.len());
    for ch in task_id.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "unknown-task".to_string()
    } else {
        out
    }
}

fn parse_bool_env(raw: &str) -> bool {
    matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "y" | "on")
}

async fn pump_child_stream(
    store: Arc<CommentDataStore>,
    run_id: &str,
    task_id: &str,
    stream: &str,
    sequence: Arc<AtomicI32>,
    reader: impl tokio::io::AsyncRead + Unpin,
) -> Result<String> {
    let mut lines = BufReader::new(reader).lines();
    let mut collected = String::new();
    let mut accepted = 0usize;

    while let Some(line) = lines.next_line().await? {
        if stream == "stderr" && should_suppress_runner_stderr_line(&line) {
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
            .append_ai_run_chunk(NewCommentAiRunChunkInput {
                chunk_id,
                run_id: run_id.to_string(),
                task_id: task_id.to_string(),
                stream: stream.to_string(),
                batch_index,
                content: line,
            })
            .await
        {
            tracing::warn!("failed to append ai run chunk run_id={run_id} stream={stream}: {err}");
        } else {
            accepted += 1;
        }
    }

    Ok(collected)
}

fn should_suppress_runner_stderr_line(line: &str) -> bool {
    let normalized = line.trim();
    normalized.contains("state db missing rollout path for thread")
}

async fn mark_task_failed(store: &CommentDataStore, task_id: &str, message: String) {
    let _ = store
        .transition_comment_task(task_id, COMMENT_STATUS_FAILED, None, Some(message), false)
        .await;
}

#[cfg(test)]
fn parse_runner_output(stdout: &str) -> Result<String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        anyhow::bail!("comment ai runner returned empty output");
    }

    if let Some(markdown) = extract_final_reply_markdown(trimmed) {
        return Ok(markdown);
    }

    let normalized_quotes = normalize_json_quotes(trimmed);
    if normalized_quotes != trimmed {
        if let Some(markdown) = extract_final_reply_markdown(&normalized_quotes) {
            return Ok(markdown);
        }
    }

    if let Some(markdown) = extract_final_reply_from_text(&normalized_quotes) {
        return Ok(markdown);
    }

    let normalized_unescaped = normalized_quotes
        .replace("\\\"", "\"")
        .replace("\\\\", "\\");
    if normalized_unescaped != normalized_quotes {
        if let Some(markdown) = extract_final_reply_markdown(&normalized_unescaped) {
            return Ok(markdown);
        }
        if let Some(markdown) = extract_final_reply_from_text(&normalized_unescaped) {
            return Ok(markdown);
        }
    }

    let diagnostics = inspect_runner_output(&normalized_quotes);
    if looks_like_codex_json_stream(&normalized_quotes) {
        anyhow::bail!(
            "codex stream completed but no `final_reply_markdown` payload was extracted ({})",
            diagnostics.summary()
        );
    }

    anyhow::bail!(
        "runner output missing `final_reply_markdown` payload ({})",
        diagnostics.summary()
    )
}

#[cfg(test)]
fn parse_runner_output_with_fallback(
    stdout: &str,
    stderr: &str,
) -> Result<(String, RunnerReplySource)> {
    match parse_runner_output(stdout) {
        Ok(reply) => Ok((reply, RunnerReplySource::Stdout)),
        Err(stdout_err) => match parse_runner_output(stderr) {
            Ok(reply) => Ok((reply, RunnerReplySource::Stderr)),
            Err(stderr_err) => {
                anyhow::bail!("stdout_parse_error={stdout_err}; stderr_parse_error={stderr_err}")
            },
        },
    }
}

#[cfg(test)]
fn extract_final_reply_markdown(raw: &str) -> Option<String> {
    let mut candidates = Vec::new();

    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        collect_markdown_candidates(&value, &mut candidates);
    }

    for value in serde_json::Deserializer::from_str(raw)
        .into_iter::<Value>()
        .flatten()
    {
        collect_markdown_candidates(&value, &mut candidates);
    }

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            collect_markdown_candidates(&value, &mut candidates);
        }
    }

    candidates
        .into_iter()
        .rev()
        .find(|item| !item.trim().is_empty())
        .map(|item| item.trim().to_string())
}

fn collect_markdown_candidates(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(raw) = map.get("final_reply_markdown").and_then(Value::as_str) {
                let parsed = raw.trim();
                if !parsed.is_empty() {
                    output.push(parsed.to_string());
                }
            } else if let Ok(typed) = serde_json::from_value::<WorkerRunnerOutput>(value.clone()) {
                if let Some(raw) = typed.final_reply_markdown {
                    let parsed = raw.trim();
                    if !parsed.is_empty() {
                        output.push(parsed.to_string());
                    }
                }
            }

            for nested in map.values() {
                collect_markdown_candidates(nested, output);
            }
        },
        Value::Array(items) => {
            for item in items {
                collect_markdown_candidates(item, output);
            }
        },
        Value::String(raw) => {
            // Codex JSON streaming events may place the final payload as a JSON
            // string inside fields like `item.text`.
            if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
                collect_markdown_candidates(&parsed, output);
                return;
            }

            if let Some(markdown) = extract_final_reply_from_text(raw) {
                output.push(markdown);
                return;
            }

            // Some streams embed escaped JSON text like:
            // {\"final_reply_markdown\":\"...\"}
            let unescaped = raw.replace("\\\"", "\"").replace("\\\\", "\\");
            if let Some(markdown) = extract_final_reply_from_text(&unescaped) {
                output.push(markdown);
            }
        },
        _ => {},
    }
}

fn extract_final_reply_from_text(raw: &str) -> Option<String> {
    let key = "\"final_reply_markdown\"";
    let mut cursor = 0usize;
    let mut latest = None;

    while let Some(offset) = raw[cursor..].find(key) {
        let start = cursor + offset + key.len();
        let tail = &raw[start..];
        let colon_index = tail.find(':')?;
        let after_colon_raw = &tail[colon_index + 1..];
        let after_colon = after_colon_raw.trim_start();
        let whitespace_offset = after_colon_raw.len().saturating_sub(after_colon.len());
        if let Some((value, consumed)) = parse_json_string_literal(after_colon) {
            let parsed = value.trim();
            if !parsed.is_empty() {
                latest = Some(parsed.to_string());
            }
            cursor = start + colon_index + 1 + whitespace_offset + consumed;
        } else {
            cursor = start;
        }
    }

    latest
}

fn parse_json_string_literal(raw: &str) -> Option<(String, usize)> {
    let bytes = raw.as_bytes();
    if bytes.first().copied() != Some(b'"') {
        return None;
    }

    let mut escaped = false;
    for idx in 1..bytes.len() {
        let byte = bytes[idx];
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            continue;
        }
        if byte == b'"' {
            let slice = &raw[..=idx];
            let value = serde_json::from_str::<String>(slice).ok()?;
            return Some((value, idx + 1));
        }
    }
    None
}

#[cfg(test)]
fn normalize_json_quotes(raw: &str) -> String {
    raw.replace(['“', '”'], "\"").replace(['‘', '’'], "'")
}

#[cfg(test)]
fn looks_like_codex_json_stream(raw: &str) -> bool {
    raw.contains("\"type\":\"turn.completed\"")
        || raw.contains("\"type\":\"item.completed\"")
        || raw.contains("\"type\": \"turn.completed\"")
        || raw.contains("\"type\": \"item.completed\"")
}

#[derive(Default)]
struct RunnerOutputDiagnostics {
    line_count: usize,
    json_line_count: usize,
    item_completed_count: usize,
    agent_message_item_count: usize,
    turn_completed_count: usize,
    final_reply_markdown_count: usize,
}

impl RunnerOutputDiagnostics {
    fn summary(&self) -> String {
        format!(
            "lines={}, json_lines={}, item_completed={}, agent_message_items={}, \
             turn_completed={}, final_reply_candidates={}",
            self.line_count,
            self.json_line_count,
            self.item_completed_count,
            self.agent_message_item_count,
            self.turn_completed_count,
            self.final_reply_markdown_count
        )
    }
}

fn inspect_runner_output(raw: &str) -> RunnerOutputDiagnostics {
    let mut diagnostics = RunnerOutputDiagnostics::default();

    for line in raw.lines() {
        diagnostics.line_count += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };

        diagnostics.json_line_count += 1;

        if let Some(event_type) = value.get("type").and_then(Value::as_str) {
            if event_type == "item.completed" {
                diagnostics.item_completed_count += 1;
                if value
                    .get("item")
                    .and_then(|item| item.get("type"))
                    .and_then(Value::as_str)
                    == Some("agent_message")
                {
                    diagnostics.agent_message_item_count += 1;
                }
            } else if event_type == "turn.completed" {
                diagnostics.turn_completed_count += 1;
            }
        }

        let mut candidates = Vec::new();
        collect_markdown_candidates(&value, &mut candidates);
        diagnostics.final_reply_markdown_count += candidates
            .iter()
            .filter(|candidate| !candidate.trim().is_empty())
            .count();
    }

    if diagnostics.line_count == 0 {
        diagnostics.line_count = 1;
    }

    diagnostics
}

fn compact_for_reason(raw: &str) -> String {
    let compact = raw.trim();
    if compact.chars().count() <= 800 {
        return compact.to_string();
    }
    let head = compact.chars().take(800).collect::<String>();
    format!("{head}...(truncated)")
}

fn derive_author_identity(fingerprint: &str, salt: &str) -> (String, String, String) {
    let raw = format!("{fingerprint}:{salt}");
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    let short = &digest[..10];
    let author_name = format!("Reader-{}", &short[..6]);
    let avatar_seed = short.to_string();
    (digest, author_name, avatar_seed)
}

fn generate_ai_run_id(task_id: &str) -> String {
    format!("airun-{}-{}", task_id, now_ms().unsigned_abs())
}

fn now_ms() -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(now).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        build_comment_result_file_path, parse_runner_output, parse_runner_output_with_fallback,
        sanitize_task_id_for_path, RunnerReplySource,
    };

    #[test]
    fn parse_runner_output_extracts_single_json_object() {
        let raw = r#"{"final_reply_markdown":"hello"}"#;
        let parsed = parse_runner_output(raw).unwrap();
        assert_eq!(parsed, "hello");
    }

    #[test]
    fn parse_runner_output_extracts_last_json_object() {
        let raw = r#"{"final_reply_markdown":"first"}{"final_reply_markdown":"second"}"#;
        let parsed = parse_runner_output(raw).unwrap();
        assert_eq!(parsed, "second");
    }

    #[test]
    fn parse_runner_output_extracts_from_jsonl() {
        let raw = r#"{"event":"thinking"}
{"final_reply_markdown":"line-jsonl-answer"}"#;
        let parsed = parse_runner_output(raw).unwrap();
        assert_eq!(parsed, "line-jsonl-answer");
    }

    #[test]
    fn parse_runner_output_handles_smart_quotes() {
        let raw = "{“final_reply_markdown”:“你好，测试”}";
        let parsed = parse_runner_output(raw).unwrap();
        assert_eq!(parsed, "你好，测试");
    }

    #[test]
    fn parse_runner_output_extracts_from_codex_stream_item_text_json_string() {
        let raw = r#"{"type":"item.completed","item":{"id":"item_69","type":"agent_message","text":"{\"final_reply_markdown\":\"stream-final\"}"}}
{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}"#;
        let parsed = parse_runner_output(raw).unwrap();
        assert_eq!(parsed, "stream-final");
    }

    #[test]
    fn parse_runner_output_extracts_from_escaped_final_reply_text_without_outer_json() {
        let raw = r#"stream-chunk text: {\"final_reply_markdown\":\"escaped-final\"}
{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}"#;
        let parsed = parse_runner_output(raw).unwrap();
        assert_eq!(parsed, "escaped-final");
    }

    #[test]
    fn parse_runner_output_rejects_turn_completed_without_final_payload() {
        let raw = r#"{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}"#;
        let parsed = parse_runner_output(raw);
        assert!(parsed.is_err());
    }

    #[test]
    fn parse_runner_output_rejects_plain_markdown_without_json_contract() {
        let raw = "just markdown without json contract";
        let parsed = parse_runner_output(raw);
        assert!(parsed.is_err());
    }

    #[test]
    fn parse_runner_output_with_fallback_prefers_stdout() {
        let stdout = r#"{"final_reply_markdown":"stdout-final"}"#;
        let stderr = r#"{"final_reply_markdown":"stderr-final"}"#;
        let parsed = parse_runner_output_with_fallback(stdout, stderr).unwrap();
        assert_eq!(parsed.0, "stdout-final");
        assert_eq!(parsed.1, RunnerReplySource::Stdout);
    }

    #[test]
    fn parse_runner_output_with_fallback_uses_stderr_when_stdout_empty() {
        let stdout = "";
        let stderr = r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"{\"final_reply_markdown\":\"stderr-stream-final\"}"}}
{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}"#;
        let parsed = parse_runner_output_with_fallback(stdout, stderr).unwrap();
        assert_eq!(parsed.0, "stderr-stream-final");
        assert_eq!(parsed.1, RunnerReplySource::Stderr);
    }

    #[test]
    fn parse_runner_output_with_fallback_rejects_when_both_channels_invalid() {
        let stdout = "invalid stdout";
        let stderr = "invalid stderr";
        let parsed = parse_runner_output_with_fallback(stdout, stderr);
        assert!(parsed.is_err());
    }

    #[test]
    fn sanitize_task_id_for_path_replaces_unsafe_chars() {
        let safe = sanitize_task_id_for_path("cmt:17713/abc?*中文");
        assert_eq!(safe, "cmt_17713_abc____");
    }

    #[test]
    fn build_comment_result_file_path_uses_task_prefix_and_md_suffix() {
        let path =
            build_comment_result_file_path(Path::new("/tmp/staticflow-comment-results"), "cmt/1");
        assert_eq!(path.to_string_lossy(), "/tmp/staticflow-comment-results/task-cmt_1.md");
    }
}
