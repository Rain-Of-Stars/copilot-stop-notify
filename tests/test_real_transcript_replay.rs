// 真实 transcript 前缀回放测试：用于发现同一轮用户请求尚未结束时的提前 ready

use copilot_stop_notify::transcript::parse_transcript_snapshot;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct ReplayEvent {
    #[serde(rename = "type")]
    event_type: String,
}

fn load_event_lines(path: &Path) -> Vec<(usize, String)> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            serde_json::from_str::<ReplayEvent>(line)
                .ok()
                .map(|event| (index + 1, event.event_type))
        })
        .collect()
}

fn prefix_is_ready(lines: &[&str], line_count: usize) -> bool {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prefix.jsonl");
    let prefix = lines[..line_count].join("\n");
    fs::write(&path, prefix).unwrap();
    parse_transcript_snapshot(&path)
        .map(|snapshot| snapshot.is_ready_for_email())
        .unwrap_or(false)
}

fn has_continuation_before_next_user(events: &[(usize, String)], start_index: usize) -> bool {
    for (_, event_type) in events.iter().skip(start_index) {
        match event_type.as_str() {
            "user.message" => return false,
            "assistant.turn_start"
            | "assistant.message"
            | "assistant.turn_end"
            | "tool.execution_start"
            | "tool.execution_complete" => return true,
            _ => {}
        }
    }

    false
}

#[ignore = "需要设置 COPILOT_STOP_NOTIFY_TRANSCRIPT_DIR 指向真实 transcript 目录"]
#[test]
fn test_real_transcripts_do_not_become_ready_before_same_prompt_finishes() {
    let transcript_dir = std::env::var("COPILOT_STOP_NOTIFY_TRANSCRIPT_DIR")
        .expect("缺少 COPILOT_STOP_NOTIFY_TRANSCRIPT_DIR");
    let mut transcript_paths: Vec<PathBuf> = fs::read_dir(&transcript_dir)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|item| item.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
        .collect();
    transcript_paths.sort();

    let mut findings = Vec::new();

    for path in transcript_paths {
        let data = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = data.lines().collect();
        let events = load_event_lines(&path);

        for (event_index, (line_number, event_type)) in events.iter().enumerate() {
            if !matches!(event_type.as_str(), "assistant.turn_end" | "session.end") {
                continue;
            }

            if !prefix_is_ready(&lines, *line_number) {
                continue;
            }

            if has_continuation_before_next_user(&events, event_index + 1) {
                findings.push(format!(
                    "{} 第{}行在下一次 user.message 前已经 ready",
                    path.display(),
                    line_number
                ));
            }
        }
    }

    assert!(
        findings.is_empty(),
        "发现可能提前发信的真实 transcript 前缀:\n{}",
        findings.join("\n")
    );
}