//! Command-line helpers for inspecting usage journals.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};

use crate::{reader::JournalReader, writer::parse_sequence_from_file_name};

/// Run the `llm-usage-journal` CLI from process arguments.
pub fn run_from_env() -> Result<()> {
    run(env::args().collect())
}

/// Run the CLI from explicit arguments.
pub fn run(args: Vec<String>) -> Result<()> {
    let command = args.get(1).map(String::as_str).ok_or_else(usage_error)?;
    match command {
        "list" => {
            let dir = required_flag_value(&args, "--dir")?;
            list_command(Path::new(dir))
        },
        "inspect" => {
            let file = args.get(2).ok_or_else(usage_error)?;
            inspect_command(Path::new(file))
        },
        "stats" => {
            let dir = required_flag_value(&args, "--dir")?;
            stats_command(Path::new(dir))
        },
        "dump" => {
            let file = args.get(2).ok_or_else(usage_error)?;
            let limit = optional_flag_value(&args, "--limit")
                .unwrap_or("50")
                .parse::<usize>()
                .context("failed to parse --limit")?;
            dump_command(Path::new(file), limit)
        },
        "grep" => {
            let dir = required_flag_value(&args, "--dir")?;
            let key_name = optional_flag_value(&args, "--key-name");
            let event_id = optional_flag_value(&args, "--event-id");
            let since_ms = optional_flag_value(&args, "--since")
                .map(parse_duration_ms)
                .transpose()?;
            grep_command(Path::new(dir), key_name, event_id, since_ms)
        },
        _ => Err(usage_error()),
    }
}

fn list_command(root: &Path) -> Result<()> {
    for line in collect_list_lines(root)? {
        println!("{line}");
    }
    Ok(())
}

fn collect_list_lines(root: &Path) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    for state in ["active", "sealed", "consuming", "bad"] {
        let dir = root.join(state);
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&dir)
            .with_context(|| format!("failed to read journal dir `{}`", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let bytes = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let sequence = parse_sequence_from_file_name(&file_name)
                .map(|sequence| sequence.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let events = count_events_if_complete(&path)
                .map(|count| count.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            lines.push(format!(
                "{state}\tsequence={sequence}\tbytes={bytes}\tevents={events}\t{}",
                path.display()
            ));
        }
    }
    lines.sort();
    Ok(lines)
}

fn count_events_if_complete(path: &Path) -> Option<usize> {
    JournalReader::open(path)
        .ok()?
        .read_all_batches()
        .ok()
        .map(|batches| {
            batches
                .iter()
                .map(|batch| batch.events.len())
                .sum::<usize>()
        })
}

fn inspect_command(path: &Path) -> Result<()> {
    let batches = JournalReader::open(path)?.read_all_batches()?;
    let event_count = batches
        .iter()
        .map(|batch| batch.events.len())
        .sum::<usize>();
    println!("ok\t{}\tblocks={}\tevents={event_count}", path.display(), batches.len());
    Ok(())
}

fn stats_command(root: &Path) -> Result<()> {
    let mut files = 0u64;
    let mut bytes = 0u64;
    for state in ["active", "sealed", "consuming", "bad"] {
        let dir = root.join(state);
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if entry.path().is_file() {
                files = files.saturating_add(1);
                bytes = bytes.saturating_add(entry.metadata().map(|meta| meta.len()).unwrap_or(0));
            }
        }
    }
    println!("files={files}\tbytes={bytes}");
    Ok(())
}

fn dump_command(path: &Path, limit: usize) -> Result<()> {
    let mut emitted = 0usize;
    for batch in JournalReader::open(path)?.read_all_batches()? {
        for event in batch.events {
            if emitted >= limit {
                return Ok(());
            }
            println!("{}", serde_json::to_string(&event)?);
            emitted = emitted.saturating_add(1);
        }
    }
    Ok(())
}

fn grep_command(
    root: &Path,
    key_name: Option<&str>,
    event_id: Option<&str>,
    since_ms: Option<i64>,
) -> Result<()> {
    let threshold = since_ms.map(|age| now_ms().saturating_sub(age));
    for path in journal_files(root)? {
        let Ok(reader) = JournalReader::open(&path) else {
            continue;
        };
        let Ok(batches) = reader.read_all_batches() else {
            continue;
        };
        for batch in batches {
            for event in batch.events {
                if let Some(key_name) = key_name {
                    if event.key_name != key_name {
                        continue;
                    }
                }
                if let Some(event_id) = event_id {
                    if event.event_id != event_id {
                        continue;
                    }
                }
                if let Some(threshold) = threshold {
                    if event.created_at_ms < threshold {
                        continue;
                    }
                }
                println!("{}", serde_json::to_string(&event)?);
            }
        }
    }
    Ok(())
}

fn journal_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for state in ["sealed", "active", "consuming"] {
        let dir = root.join(state);
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn required_flag_value<'a>(args: &'a [String], flag: &str) -> Result<&'a str> {
    optional_flag_value(args, flag).ok_or_else(|| anyhow!("{flag} is required"))
}

fn optional_flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|window| window[0] == flag)
        .map(|window| window[1].as_str())
}

fn parse_duration_ms(value: &str) -> Result<i64> {
    let (number, multiplier) = match value.as_bytes().last().copied() {
        Some(b's') => (&value[..value.len() - 1], 1_000),
        Some(b'm') => (&value[..value.len() - 1], 60_000),
        Some(b'h') => (&value[..value.len() - 1], 3_600_000),
        Some(b'd') => (&value[..value.len() - 1], 86_400_000),
        _ => (value, 1),
    };
    Ok(number.parse::<i64>()?.saturating_mul(multiplier))
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn usage_error() -> anyhow::Error {
    anyhow!(
        "usage: llm-usage-journal list --dir <root>\nusage: llm-usage-journal inspect \
         <file>\nusage: llm-usage-journal stats --dir <root>\nusage: llm-usage-journal dump \
         <file> --limit 50\nusage: llm-usage-journal grep --dir <root> [--key-name <name>] \
         [--event-id <id>] [--since <duration>]"
    )
}

#[cfg(test)]
mod tests {
    use llm_access_core::{
        provider::{ProtocolFamily, ProviderType},
        usage::{UsageEvent, UsageStreamDetails, UsageTiming},
    };

    use crate::{cli::collect_list_lines, JournalConfig, JournalWriter};

    #[test]
    fn list_lines_include_sequence_bytes_and_event_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut writer =
            JournalWriter::open(JournalConfig::new(dir.path().to_path_buf())).expect("writer");
        writer
            .append_events(&[test_usage_event("evt-cli-list")])
            .expect("append");
        writer.seal_current_file().expect("seal");

        let lines = collect_list_lines(dir.path()).expect("list lines");

        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("sealed\tsequence=0\t"));
        assert!(lines[0].contains("\tevents=1\t"));
    }

    fn test_usage_event(event_id: &str) -> UsageEvent {
        UsageEvent {
            event_id: event_id.to_string(),
            created_at_ms: 1_700_000_000_000,
            provider_type: ProviderType::Kiro,
            protocol_family: ProtocolFamily::Anthropic,
            key_id: "key-1".to_string(),
            key_name: "for-yangshu".to_string(),
            account_name: Some("acct-1".to_string()),
            account_group_id_at_event: Some("group-1".to_string()),
            route_strategy_at_event: None,
            request_method: "POST".to_string(),
            request_url: "/v1/messages".to_string(),
            endpoint: "/v1/messages".to_string(),
            model: Some("claude-opus-4-7".to_string()),
            mapped_model: Some("claude-opus-4-7".to_string()),
            status_code: 200,
            request_body_bytes: Some(17),
            quota_failover_count: 0,
            routing_diagnostics_json: Some("{\"route\":\"fixed\"}".to_string()),
            input_uncached_tokens: 10,
            input_cached_tokens: 20,
            output_tokens: 30,
            billable_tokens: 40,
            credit_usage: Some("0.12".to_string()),
            usage_missing: false,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{\"user-agent\":\"test\"}".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some("{\"model\":\"m\"}".to_string()),
            upstream_request_body_json: Some("{\"upstream\":true}".to_string()),
            full_request_json: Some("{\"model\":\"m\"}".to_string()),
            error_message: None,
            error_body: None,
            timing: UsageTiming::default(),
            stream: UsageStreamDetails::default(),
        }
    }
}
