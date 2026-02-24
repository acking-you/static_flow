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
use static_flow_shared::music_wish_store::{
    MusicWishRecord, MusicWishStore, NewMusicWishAiRunChunkInput, NewMusicWishAiRunInput,
    WISH_AI_RUN_STATUS_FAILED, WISH_AI_RUN_STATUS_SUCCESS, WISH_STATUS_APPROVED, WISH_STATUS_DONE,
    WISH_STATUS_FAILED, WISH_STATUS_REJECTED, WISH_STATUS_RUNNING,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::mpsc,
    time::timeout,
};

#[derive(Clone, Debug)]
pub struct MusicWishWorkerConfig {
    pub runner_program: String,
    pub runner_args: Vec<String>,
    pub timeout_seconds: u64,
    pub workdir: PathBuf,
    pub music_db_path: String,
    pub skill_path: PathBuf,
    pub result_dir: PathBuf,
    pub cleanup_result_file_on_success: bool,
}

impl MusicWishWorkerConfig {
    pub fn from_env(music_db_path: String) -> Self {
        let runner_program =
            env::var("MUSIC_WISH_RUNNER_PROGRAM").unwrap_or_else(|_| "bash".to_string());
        let runner_args = env::var("MUSIC_WISH_RUNNER_ARGS")
            .ok()
            .map(|v| {
                v.split_whitespace()
                    .map(str::to_string)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| vec!["scripts/music_wish_worker_runner.sh".to_string()]);
        let timeout_seconds = env::var("MUSIC_WISH_TIMEOUT_SECONDS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(300)
            .max(30);
        let workdir = env::var("MUSIC_WISH_WORKDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let skill_path = env::var("MUSIC_WISH_SKILL_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| workdir.join("skills/music-ingestion-publisher/SKILL.md"));
        let result_dir = env::var("MUSIC_WISH_RESULT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp/staticflow-music-wish-results"));
        let cleanup_result_file_on_success = env::var("MUSIC_WISH_RESULT_CLEANUP_ON_SUCCESS")
            .ok()
            .map(|v| parse_bool_env(&v))
            .unwrap_or(true);

        Self {
            runner_program,
            runner_args,
            timeout_seconds,
            workdir,
            music_db_path,
            skill_path,
            result_dir,
            cleanup_result_file_on_success,
        }
    }
}

#[derive(Debug, Serialize)]
struct WishWorkerPayload<'a> {
    wish_id: &'a str,
    song_name: &'a str,
    artist_hint: Option<&'a str>,
    wish_message: &'a str,
    music_db_path: &'a str,
    skill_path: String,
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

pub fn spawn_music_wish_worker(
    store: Arc<MusicWishStore>,
    config: MusicWishWorkerConfig,
) -> mpsc::Sender<String> {
    let (sender, mut receiver) = mpsc::channel::<String>(128);
    tokio::spawn(async move {
        while let Some(wish_id) = receiver.recv().await {
            if let Err(err) = process_one_wish(store.clone(), config.clone(), &wish_id).await {
                tracing::error!("music wish worker failed for {wish_id}: {err}");
            }
        }
    });
    sender
}

async fn process_one_wish(
    store: Arc<MusicWishStore>,
    config: MusicWishWorkerConfig,
    wish_id: &str,
) -> Result<()> {
    let wish = match store.get_wish(wish_id).await? {
        Some(w) => w,
        None => {
            tracing::warn!("music wish worker skipped missing wish {wish_id}");
            return Ok(());
        },
    };

    if wish.status == WISH_STATUS_REJECTED || wish.status == WISH_STATUS_DONE {
        tracing::info!("music wish worker skipped finalized wish {wish_id}");
        return Ok(());
    }

    if wish.status == WISH_STATUS_APPROVED {
        store
            .transition_wish(wish_id, WISH_STATUS_RUNNING, None, None, None, None)
            .await?;
    } else if wish.status != WISH_STATUS_RUNNING {
        tracing::warn!("music wish worker skipped wish {wish_id} with status {}", wish.status);
        return Ok(());
    }

    let run_id = format!("mwrun-{}-{}", wish_id, chrono::Utc::now().timestamp_millis());
    if let Err(err) = store
        .create_ai_run(NewMusicWishAiRunInput {
            run_id: run_id.clone(),
            wish_id: wish_id.to_string(),
            runner_program: config.runner_program.clone(),
        })
        .await
    {
        let reason = format!("failed to create music wish ai run: {err}");
        mark_wish_failed(&store, wish_id, reason).await;
        return Ok(());
    }

    let run_output = match run_wish_runner(store.clone(), &config, &wish, &run_id).await {
        Ok(output) => output,
        Err(err) => {
            let reason = err.to_string();
            let _ = store
                .finalize_ai_run(&run_id, WISH_AI_RUN_STATUS_FAILED, None, Some(&reason), None)
                .await;
            mark_wish_failed(&store, wish_id, reason).await;
            return Ok(());
        },
    };

    let result_json = match read_result_json(&run_output.result_file_path).await {
        Ok(j) => j,
        Err(err) => {
            let reason = format!(
                "music wish result file invalid: {err} path={} exit_code={:?}",
                run_output.result_file_path.display(),
                run_output.exit_code,
            );
            let _ = store
                .finalize_ai_run(
                    &run_id,
                    WISH_AI_RUN_STATUS_FAILED,
                    run_output.exit_code,
                    Some(&reason),
                    None,
                )
                .await;
            mark_wish_failed(&store, wish_id, reason).await;
            return Ok(());
        },
    };

    let ingested_song_id = result_json
        .get("ingested_song_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let reply_markdown = result_json
        .get("reply_markdown")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if let Err(err) = store
        .transition_wish(wish_id, WISH_STATUS_DONE, None, None, ingested_song_id.as_deref(), Some(&reply_markdown))
        .await
    {
        let reason = format!("failed to mark wish done: {err}");
        let _ = store
            .finalize_ai_run(
                &run_id,
                WISH_AI_RUN_STATUS_FAILED,
                run_output.exit_code,
                Some(&reason),
                Some(&reply_markdown),
            )
            .await;
        mark_wish_failed(&store, wish_id, reason).await;
        return Ok(());
    }

    let _ = store
        .finalize_ai_run(
            &run_id,
            WISH_AI_RUN_STATUS_SUCCESS,
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

async fn run_wish_runner(
    store: Arc<MusicWishStore>,
    config: &MusicWishWorkerConfig,
    wish: &MusicWishRecord,
    run_id: &str,
) -> Result<RunnerProcessOutput> {
    tokio::fs::create_dir_all(&config.result_dir)
        .await
        .with_context(|| {
            format!("failed to ensure music wish result dir {}", config.result_dir.display())
        })?;
    let result_file_path = build_result_file_path(&config.result_dir, &wish.wish_id);
    let _ = tokio::fs::remove_file(&result_file_path).await;

    let payload = WishWorkerPayload {
        wish_id: &wish.wish_id,
        song_name: &wish.song_name,
        artist_hint: wish.artist_hint.as_deref(),
        wish_message: &wish.wish_message,
        music_db_path: &config.music_db_path,
        skill_path: config.skill_path.display().to_string(),
    };

    let payload_json =
        serde_json::to_vec_pretty(&payload).context("failed to encode wish payload")?;
    let payload_path =
        std::env::temp_dir().join(format!("staticflow-music-wish-{}.json", wish.wish_id));
    tokio::fs::write(&payload_path, payload_json)
        .await
        .with_context(|| format!("failed to write payload {}", payload_path.display()))?;

    let mut command = Command::new(&config.runner_program);
    command.args(config.runner_args.clone());
    command.arg(payload_path.as_os_str());
    command.current_dir(&config.workdir);
    command.env("MUSIC_WISH_SKILL_PATH", &config.skill_path);
    command.env("MUSIC_DB_PATH", &config.music_db_path);
    command.env("MUSIC_WISH_RESULT_DIR", &config.result_dir);
    command.env("MUSIC_WISH_RESULT_PATH", &result_file_path);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .context("failed to execute music wish runner command")?;
    let stdout = child.stdout.take().context("missing runner stdout pipe")?;
    let stderr = child.stderr.take().context("missing runner stderr pipe")?;

    let sequence = Arc::new(AtomicI32::new(0));
    let stdout_handle = {
        let store = store.clone();
        let run_id = run_id.to_string();
        let wish_id = wish.wish_id.clone();
        let sequence = sequence.clone();
        tokio::spawn(async move {
            pump_child_stream(store, &run_id, &wish_id, "stdout", sequence, stdout).await
        })
    };
    let stderr_handle = {
        let store = store.clone();
        let run_id = run_id.to_string();
        let wish_id = wish.wish_id.clone();
        let sequence = sequence.clone();
        tokio::spawn(async move {
            pump_child_stream(store, &run_id, &wish_id, "stderr", sequence, stderr).await
        })
    };

    let status = match timeout(Duration::from_secs(config.timeout_seconds), child.wait()).await {
        Ok(result) => result.context("failed to wait music wish runner")?,
        Err(_) => {
            let _ = child.kill().await;
            anyhow::bail!("music wish runner timed out");
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

fn build_result_file_path(result_dir: &Path, wish_id: &str) -> PathBuf {
    let safe = sanitize_id_for_path(wish_id);
    result_dir.join(format!("wish-{safe}.json"))
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
    store: Arc<MusicWishStore>,
    run_id: &str,
    wish_id: &str,
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
            .append_ai_run_chunk(NewMusicWishAiRunChunkInput {
                chunk_id,
                run_id: run_id.to_string(),
                wish_id: wish_id.to_string(),
                stream: stream.to_string(),
                batch_index,
                content: line,
            })
            .await
        {
            tracing::warn!("failed to append music wish ai chunk run_id={run_id}: {err}");
        } else {
            accepted += 1;
        }
    }

    Ok(collected)
}

async fn mark_wish_failed(store: &MusicWishStore, wish_id: &str, message: String) {
    let _ = store
        .transition_wish(wish_id, WISH_STATUS_FAILED, None, Some(&message), None, None)
        .await;
}

fn parse_bool_env(raw: &str) -> bool {
    matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "y" | "on")
}
