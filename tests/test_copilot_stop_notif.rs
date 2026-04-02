use copilot_stop_notif::{
    ConversationMessage, TranscriptData, build_email_html, build_email_html_with_options,
    build_email_text_body, build_email_text_body_with_options, build_hook_payload,
    build_plain_text_body, build_plain_text_body_with_options, build_vscode_notify_payload,
    format_message, format_message_with_options, load_email_config, load_hook_input_from_reader,
    load_transcript_data, run_hook, run_notify, validate_transcript_path,
};
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Cursor, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::{Builder, tempdir};

fn start_fake_smtp_server() -> (u16, Arc<AtomicUsize>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let mail_count = Arc::new(AtomicUsize::new(0));
    let mail_counter = Arc::clone(&mail_count);

    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);

        loop {
            match listener.accept() {
                Ok((stream, _)) => handle_smtp_client(stream, &mail_counter),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        break;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("SMTP 测试服务监听失败: {error}"),
            }
        }
    });

    (port, mail_count, handle)
}

fn handle_smtp_client(mut stream: TcpStream, mail_count: &Arc<AtomicUsize>) {
    stream.set_nonblocking(false).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    stream.write_all(b"220 localhost ready\r\n").unwrap();
    stream.flush().unwrap();

    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).unwrap();
        if bytes == 0 {
            break;
        }

        let command = line.trim_end_matches(['\r', '\n']);

        if command.starts_with("EHLO") || command.starts_with("HELO") {
            stream
                .write_all(b"250-localhost\r\n250-AUTH PLAIN LOGIN\r\n250 OK\r\n")
                .unwrap();
        } else if command.eq_ignore_ascii_case("AUTH LOGIN") {
            stream.write_all(b"334 VXNlcm5hbWU6\r\n").unwrap();
            stream.flush().unwrap();

            let mut username = String::new();
            reader.read_line(&mut username).unwrap();

            stream.write_all(b"334 UGFzc3dvcmQ6\r\n").unwrap();
            stream.flush().unwrap();

            let mut password = String::new();
            reader.read_line(&mut password).unwrap();
            stream
                .write_all(b"235 2.7.0 Authentication successful\r\n")
                .unwrap();
        } else if command.starts_with("AUTH ") {
            stream
                .write_all(b"235 2.7.0 Authentication successful\r\n")
                .unwrap();
        } else if command.starts_with("MAIL FROM:") || command.starts_with("RCPT TO:") {
            stream.write_all(b"250 2.1.0 Ok\r\n").unwrap();
        } else if command.eq_ignore_ascii_case("DATA") {
            stream
                .write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n")
                .unwrap();
            stream.flush().unwrap();

            loop {
                let mut data_line = String::new();
                let bytes = reader.read_line(&mut data_line).unwrap();
                if bytes == 0 || data_line == ".\r\n" || data_line == ".\n" {
                    break;
                }
            }

            mail_count.fetch_add(1, Ordering::SeqCst);
            stream.write_all(b"250 2.0.0 Queued\r\n").unwrap();
        } else if command.eq_ignore_ascii_case("QUIT") {
            stream.write_all(b"221 2.0.0 Bye\r\n").unwrap();
            stream.flush().unwrap();
            break;
        } else {
            stream.write_all(b"250 2.0.0 Ok\r\n").unwrap();
        }

        stream.flush().unwrap();
    }
}

#[test]
fn test_build_hook_payload_extracts_vscode_event_stream() {
    let hook_input = json!({
        "cwd": "D:/workspace/copilot-stop-notif",
        "sessionId": "session-event-log",
        "hookEventName": "Stop"
    });
    let transcript = TranscriptData::Json(json!([
        {
            "type": "session.start",
            "data": {
                "sessionId": "session-event-log"
            }
        },
        {
            "type": "user.message",
            "data": {
                "content": "请帮我创建 Hook"
            }
        },
        {
            "type": "assistant.message",
            "data": {
                "content": "我会先检查当前配置。"
            }
        }
    ]));

    let payload = build_hook_payload(&hook_input, Some(&transcript));

    assert_eq!(payload.event_type, "stop");
    assert_eq!(
        payload.transcript,
        vec![
            ConversationMessage {
                role: "human".to_string(),
                text: "请帮我创建 Hook".to_string(),
            },
            ConversationMessage {
                role: "assistant".to_string(),
                text: "我会先检查当前配置。".to_string(),
            },
        ]
    );
}

#[test]
fn test_load_transcript_data_supports_jsonl() {
    let temp_dir = tempdir().unwrap();
    let transcript_path = temp_dir.path().join("transcript.jsonl");
    fs::write(
        &transcript_path,
        "{\"role\":\"user\",\"content\":\"甲\"}\n{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"乙\"}]}}\n",
    )
    .unwrap();

    let transcript = load_transcript_data(&transcript_path).unwrap().unwrap();
    let payload = build_hook_payload(
        &json!({"cwd": "D:/workspace/copilot-stop-notif"}),
        Some(&transcript),
    );

    assert_eq!(
        payload.transcript,
        vec![
            ConversationMessage {
                role: "human".to_string(),
                text: "甲".to_string(),
            },
            ConversationMessage {
                role: "assistant".to_string(),
                text: "乙".to_string(),
            },
        ]
    );
}

#[test]
fn test_build_plain_text_body_uses_last_messages_and_omits_context_by_default() {
    let hook_input = json!({
        "cwd": "D:/workspace/copilot-stop-notif",
        "sessionId": "session-123",
        "hookEventName": "Stop"
    });
    let transcript = TranscriptData::Json(json!({
        "messages": [
            {"role": "user", "content": "第一问"},
            {"role": "assistant", "content": "第一答"},
            {"role": "user", "content": "第二问"},
            {"role": "assistant", "content": [{"type": "text", "text": "第二答"}]}
        ]
    }));

    let payload = build_hook_payload(&hook_input, Some(&transcript));
    let body = build_plain_text_body(&payload);

    assert!(body.contains("第二问"));
    assert!(body.contains("第二答"));
    assert!(body.contains("事件类型: stop"));
    assert!(!body.contains("工作目录:"));
    assert!(!body.contains("会话ID:"));
}

#[test]
fn test_build_plain_text_body_with_options_includes_context() {
    let hook_input = json!({
        "cwd": "D:/workspace/copilot-stop-notif",
        "sessionId": "session-123",
        "hookEventName": "Stop"
    });
    let transcript = TranscriptData::Json(json!({
        "messages": [
            {"role": "user", "content": "问题"},
            {"role": "assistant", "content": "回答"}
        ]
    }));

    let payload = build_hook_payload(&hook_input, Some(&transcript));
    let body = build_plain_text_body_with_options(&payload, true);

    assert!(body.contains("工作目录: D:/workspace/copilot-stop-notif"));
    assert!(body.contains("会话ID: session-123"));
}

#[test]
fn test_load_email_config_reads_email_only_keys() {
    let temp_dir = tempdir().unwrap();
    let env_path = temp_dir.path().join(".env");
    fs::write(
        &env_path,
        [
            "SMTP_HOST=smtp.example.com",
            "SMTP_PORT=465",
            "SMTP_USER=user@example.com",
            "SMTP_PASSWORD=secret",
            "SMTP_USE_SSL=true",
            "EMAIL_FROM=user@example.com",
            "EMAIL_TO=first@example.com, second@example.com",
        ]
        .join("\n"),
    )
    .unwrap();

    let config = load_email_config(&env_path).unwrap();

    assert_eq!(config.smtp_host, "smtp.example.com");
    assert_eq!(config.smtp_port, 465);
    assert_eq!(config.recipients.len(), 2);
    assert!(config.smtp_use_ssl);
    assert!(!config.smtp_allow_insecure_plain);
    assert!(!config.email_include_context);
}

#[test]
fn test_load_email_config_defaults_email_from_to_smtp_user() {
    let temp_dir = tempdir().unwrap();
    let env_path = temp_dir.path().join(".env");
    fs::write(
        &env_path,
        [
            "SMTP_HOST=smtp.example.com",
            "SMTP_PORT=465",
            "SMTP_USER=user@example.com",
            "SMTP_PASSWORD=secret",
            "EMAIL_TO=first@example.com",
        ]
        .join("\n"),
    )
    .unwrap();

    let config = load_email_config(&env_path).unwrap();

    assert_eq!(config.email_from, "user@example.com");
}

#[test]
fn test_load_email_config_supports_email_include_context() {
    let temp_dir = tempdir().unwrap();
    let env_path = temp_dir.path().join(".env");
    fs::write(
        &env_path,
        [
            "SMTP_HOST=smtp.example.com",
            "SMTP_PORT=465",
            "SMTP_USER=user@example.com",
            "SMTP_PASSWORD=secret",
            "EMAIL_TO=first@example.com",
            "EMAIL_INCLUDE_CONTEXT=true",
        ]
        .join("\n"),
    )
    .unwrap();

    let config = load_email_config(&env_path).unwrap();

    assert!(config.email_include_context);
}

#[test]
fn test_load_email_config_rejects_non_email_legacy_channels() {
    let temp_dir = tempdir().unwrap();
    let env_path = temp_dir.path().join(".env");
    fs::write(
        &env_path,
        [
            "NOTIFY_CHANNELS=email,wecom",
            "SMTP_HOST=smtp.example.com",
            "SMTP_PORT=465",
            "SMTP_USER=user@example.com",
            "SMTP_PASSWORD=secret",
            "EMAIL_TO=first@example.com",
        ]
        .join("\n"),
    )
    .unwrap();

    let error = load_email_config(&env_path).unwrap_err();

    assert!(error.to_string().contains("仅支持 email"));
}

#[test]
fn test_load_email_config_errors_do_not_expose_absolute_path() {
    let temp_dir = tempdir().unwrap();
    let env_dir = temp_dir.path().join("env-dir");
    fs::create_dir_all(&env_dir).unwrap();

    let error = load_email_config(&env_dir).unwrap_err().to_string();

    assert!(error.contains("配置文件 不是普通文件"));
    assert!(!error.contains(&env_dir.to_string_lossy().to_string()));
}

#[test]
fn test_build_hook_payload_keeps_recent_messages_only() {
    let transcript = TranscriptData::Json(json!({
        "messages": (0..30)
            .map(|index| json!({
                "role": if index % 2 == 0 { "user" } else { "assistant" },
                "content": format!("消息-{index}")
            }))
            .collect::<Vec<_>>()
    }));

    let payload = build_hook_payload(&json!({}), Some(&transcript));

    assert_eq!(payload.transcript.len(), 24);
    assert_eq!(payload.transcript.first().unwrap().text, "消息-6");
    assert_eq!(payload.transcript.last().unwrap().text, "消息-29");
}

#[test]
fn test_build_vscode_notify_payload_fallback_omits_raw_hook_input() {
    let hook_input = json!({
        "cwd": "D:/secret/project",
        "sessionId": "session-secret",
        "hookEventName": "Stop",
        "transcript_path": "D:/secret/project/transcript.jsonl",
    });

    let payload = build_vscode_notify_payload(&hook_input, None);
    let body = build_email_text_body_with_options("vscode-copilot", "stop", &payload, false);

    assert!(body.contains("未读取到可用 transcript"));
    assert!(!body.contains("D:/secret/project"));
    assert!(!body.contains("session-secret"));
}

#[test]
fn test_validate_transcript_path_keeps_default_roots_when_extra_roots_configured() {
    let temp_dir = tempdir().unwrap();
    let cwd_dir = temp_dir.path().join("cwd-root");
    let extra_dir = temp_dir.path().join("extra-root");
    fs::create_dir_all(&cwd_dir).unwrap();
    fs::create_dir_all(&extra_dir).unwrap();

    let transcript_path = cwd_dir.join("transcript.jsonl");
    fs::write(
        &transcript_path,
        "{\"role\":\"assistant\",\"content\":\"ok\"}\n",
    )
    .unwrap();

    let mut env_map = HashMap::new();
    env_map.insert(
        "TRANSCRIPT_ALLOWED_ROOTS".to_string(),
        extra_dir.to_string_lossy().to_string(),
    );

    let hook_input = json!({ "cwd": cwd_dir.to_string_lossy().to_string() });

    validate_transcript_path(&transcript_path, &hook_input, &env_map).unwrap();
}

#[test]
fn test_validate_transcript_path_does_not_trust_hook_cwd() {
    let current_dir = fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
    let temp_root = fs::canonicalize(std::env::temp_dir()).unwrap_or_else(|_| std::env::temp_dir());
    if current_dir.starts_with(&temp_root) {
        return;
    }

    let temp_dir = Builder::new()
        .prefix("copilot-stop-notif-untrusted-")
        .tempdir_in(&current_dir)
        .unwrap();
    let transcript_path = temp_dir.path().join("transcript.jsonl");
    fs::write(
        &transcript_path,
        "{\"role\":\"assistant\",\"content\":\"ok\"}\n",
    )
    .unwrap();

    let env_map = HashMap::new();
    let hook_input = json!({ "cwd": current_dir.to_string_lossy().to_string() });

    let error = validate_transcript_path(&transcript_path, &hook_input, &env_map).unwrap_err();

    assert!(error.to_string().contains("不在允许目录内"));
}

#[test]
fn test_validate_transcript_path_expands_windows_style_env_vars() {
    if !cfg!(windows) {
        return;
    }

    let user_profile = std::env::var("USERPROFILE").unwrap();
    let custom_root = PathBuf::from(&user_profile).join("copilot-stop-notif-expand-root");
    if custom_root.exists() {
        fs::remove_dir_all(&custom_root).unwrap();
    }
    fs::create_dir_all(&custom_root).unwrap();

    let transcript_path = custom_root.join("transcript.jsonl");
    fs::write(
        &transcript_path,
        "{\"role\":\"assistant\",\"content\":\"ok\"}\n",
    )
    .unwrap();

    let mut env_map = HashMap::new();
    env_map.insert(
        "TRANSCRIPT_ALLOWED_ROOTS".to_string(),
        "%USERPROFILE%\\copilot-stop-notif-expand-root".to_string(),
    );

    let hook_input = json!({});
    let result = validate_transcript_path(&transcript_path, &hook_input, &env_map);

    fs::remove_dir_all(&custom_root).unwrap();

    result.unwrap();
}

#[test]
fn test_validate_transcript_path_rejects_hard_linked_transcript() {
    let current_dir = fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
    let temp_root = fs::canonicalize(std::env::temp_dir()).unwrap_or_else(|_| std::env::temp_dir());
    if current_dir.starts_with(&temp_root) {
        return;
    }

    let temp_dir = Builder::new()
        .prefix("copilot-stop-notif-hardlink-")
        .tempdir_in(&current_dir)
        .unwrap();
    let blocked_dir = temp_dir.path().join("blocked-root");
    let allowed_dir = temp_dir.path().join("allowed-root");
    fs::create_dir_all(&blocked_dir).unwrap();
    fs::create_dir_all(&allowed_dir).unwrap();

    let source_path = blocked_dir.join("outside.jsonl");
    let transcript_path = allowed_dir.join("transcript.jsonl");
    fs::write(
        &source_path,
        "{\"role\":\"assistant\",\"content\":\"ok\"}\n",
    )
    .unwrap();
    fs::hard_link(&source_path, &transcript_path).unwrap();

    let mut env_map = HashMap::new();
    env_map.insert(
        "TRANSCRIPT_ALLOWED_ROOTS".to_string(),
        allowed_dir.to_string_lossy().to_string(),
    );

    let error = validate_transcript_path(&transcript_path, &json!({}), &env_map).unwrap_err();

    assert!(error.to_string().contains("硬链接"));
}

#[test]
fn test_run_hook_ignores_oversized_transcript() {
    let temp_dir = tempdir().unwrap();
    let env_path = temp_dir.path().join(".env");
    fs::write(
        &env_path,
        [
            "SMTP_HOST=smtp.example.com",
            "SMTP_PORT=465",
            "SMTP_USER=user@example.com",
            "SMTP_PASSWORD=secret",
            "SMTP_USE_SSL=true",
            "EMAIL_FROM=user@example.com",
            "EMAIL_TO=first@example.com",
        ]
        .join("\n"),
    )
    .unwrap();

    let transcript_path = temp_dir.path().join("huge-transcript.jsonl");
    fs::write(&transcript_path, "a".repeat(1_100_000)).unwrap();

    let input = json!({
        "cwd": "D:/workspace/copilot-stop-notif",
        "sessionId": "session-oversized",
        "hookEventName": "Stop",
        "transcript_path": transcript_path,
    })
    .to_string();

    let summary = run_hook(&mut Cursor::new(input), &env_path, true).unwrap();

    assert!(summary.ok);
    assert!(!summary.sent);
    assert_eq!(summary.event_type, "stop");
    assert_eq!(summary.channel_results.get("email"), Some(&false));
}

#[test]
fn test_run_notify_ignores_non_target_codex_event() {
    let temp_dir = tempdir().unwrap();
    let env_path = temp_dir.path().join("unused.env");
    let summary = run_notify(
        &mut Cursor::new(Vec::<u8>::new()),
        env_path.as_path(),
        true,
        Some(r#"{"type":"session-start"}"#),
    )
    .unwrap();

    assert!(summary.ok);
    assert!(summary.ignored);
    assert_eq!(summary.source, "codex");
    assert_eq!(summary.event_type, "session-start");
}

#[test]
fn test_run_notify_dry_run_reports_email_channel() {
    let temp_dir = tempdir().unwrap();
    let env_path = temp_dir.path().join(".env");
    fs::write(
        &env_path,
        [
            "SMTP_HOST=smtp.example.com",
            "SMTP_PORT=465",
            "SMTP_USER=user@example.com",
            "SMTP_PASSWORD=secret",
            "SMTP_USE_SSL=true",
            "EMAIL_TO=first@example.com, second@example.com",
        ]
        .join("\n"),
    )
    .unwrap();

    let payload = json!({
        "type": "agent-turn-complete",
        "cwd": "D:/workspace/copilot-stop-notif",
        "input-messages": ["请处理"],
        "last-assistant-message": "已完成",
    })
    .to_string();

    let summary = run_notify(
        &mut Cursor::new(Vec::<u8>::new()),
        env_path.as_path(),
        true,
        Some(&payload),
    )
    .unwrap();

    assert!(summary.ok);
    assert_eq!(summary.channel_count, 1);
    assert_eq!(summary.recipient_count, 2);
    assert_eq!(summary.channel_results.get("email"), Some(&false));
}

#[test]
fn test_run_hook_ignores_vscode_subagent_stop() {
    let temp_dir = tempdir().unwrap();
    let env_path = temp_dir.path().join("unused.env");
    let input = json!({
        "hookEventName": "Stop",
        "agent_id": "subagent-456",
        "agent_type": "Plan",
        "sessionId": "session-subagent",
        "cwd": "D:/workspace/copilot-stop-notif",
    })
    .to_string();

    let summary = run_hook(&mut Cursor::new(input), &env_path, true).unwrap();

    assert!(summary.ok);
    assert!(summary.ignored);
    assert_eq!(summary.source, "vscode-copilot");
    assert_eq!(summary.event_type, "stop");
}

#[test]
fn test_run_hook_ignores_active_stop_hook() {
    let temp_dir = tempdir().unwrap();
    let env_path = temp_dir.path().join("unused.env");
    let input = json!({
        "hookEventName": "Stop",
        "stop_hook_active": true,
        "sessionId": "session-still-running",
        "cwd": "D:/workspace/copilot-stop-notif",
    })
    .to_string();

    let summary = run_hook(&mut Cursor::new(input), &env_path, true).unwrap();

    assert!(summary.ok);
    assert!(summary.ignored);
    assert_eq!(summary.source, "vscode-copilot");
    assert_eq!(summary.event_type, "stop");
}

#[test]
fn test_run_hook_deduplicates_successful_vscode_stop_email() {
    let (port, mail_count, server_handle) = start_fake_smtp_server();
    let temp_dir = tempdir().unwrap();
    let env_path = temp_dir.path().join(".env");
    fs::write(
        &env_path,
        [
            "SMTP_HOST=127.0.0.1",
            &format!("SMTP_PORT={port}"),
            "SMTP_USER=user@example.com",
            "SMTP_PASSWORD=secret",
            "SMTP_USE_SSL=false",
            "SMTP_ALLOW_INSECURE_PLAIN=true",
            "EMAIL_TO=recipient@example.com",
        ]
        .join("\n"),
    )
    .unwrap();

    let session_id = format!("session-{}", temp_dir.path().display());
    let input = json!({
        "hookEventName": "Stop",
        "sessionId": session_id,
        "cwd": "D:/workspace/copilot-stop-notif",
    })
    .to_string();

    let first = run_hook(&mut Cursor::new(input.clone()), &env_path, false).unwrap();
    let second = run_hook(&mut Cursor::new(input), &env_path, false).unwrap();

    server_handle.join().unwrap();

    assert!(first.ok);
    assert!(first.sent);
    assert!(!first.ignored);
    assert!(second.ok);
    assert!(second.ignored);
    assert!(!second.sent);
    assert_eq!(mail_count.load(Ordering::SeqCst), 1);
}

#[test]
fn test_load_hook_input_rejects_oversized_stdin() {
    let oversized = format!(
        "{{\"hookEventName\":\"Stop\",\"payload\":\"{}\"}}",
        "a".repeat(270_000)
    );

    let error = load_hook_input_from_reader(&mut Cursor::new(oversized)).unwrap_err();

    assert!(error.to_string().contains("Hook stdin 超过大小限制"));
}

#[test]
fn test_format_message_preserves_codex_content() {
    let long_reply = "B".repeat(3000);
    let data = json!({
        "type": "agent-turn-complete",
        "cwd": "/tmp/project",
        "input-messages": ["请处理", {"content": [{"type": "text", "text": "第二段"}]}],
        "last-assistant-message": long_reply,
    });

    let (title, content) = format_message("codex", "agent-turn-complete", &data);

    assert_eq!(title, "Codex 任务完成");
    assert!(content.contains("请处理"));
    assert!(content.contains("第二段"));
    assert!(content.contains("agent-turn-complete"));
    assert!(content.contains(&"B".repeat(512)));
    assert!(!content.contains("/tmp/project"));
}

#[test]
fn test_format_message_with_options_includes_context() {
    let data = json!({
        "type": "agent-turn-complete",
        "cwd": "/tmp/project",
        "input-messages": ["请处理"],
        "last-assistant-message": "已完成",
    });

    let (_, content) = format_message_with_options("codex", "agent-turn-complete", &data, true);

    assert!(content.contains("**工作目录**: /tmp/project"));
}

#[test]
fn test_build_email_text_body_omits_context_by_default() {
    let data = json!({
        "type": "agent-turn-complete",
        "thread-id": "abc-123-def",
        "turn-id": "turn-789",
        "cwd": "/tmp/project",
        "input-messages": ["你好"],
        "last-assistant-message": "你好，有什么可以帮你的？",
    });

    let body = build_email_text_body("codex", "agent-turn-complete", &data);

    assert!(!body.contains("线程ID:"));
    assert!(!body.contains("轮次ID:"));
    assert!(!body.contains("工作目录:"));
    assert!(body.contains("事件类型: agent-turn-complete"));
}

#[test]
fn test_build_email_text_body_with_options_includes_metadata() {
    let data = json!({
        "type": "agent-turn-complete",
        "thread-id": "abc-123-def",
        "turn-id": "turn-789",
        "cwd": "/tmp/project",
        "input-messages": ["你好"],
        "last-assistant-message": "你好，有什么可以帮你的？",
    });

    let body = build_email_text_body_with_options("codex", "agent-turn-complete", &data, true);

    assert!(body.contains("线程ID: abc-123-def"));
    assert!(body.contains("轮次ID: turn-789"));
    assert!(body.contains("你好"));
    assert!(body.contains("有什么可以帮你的"));
}

#[test]
fn test_build_email_text_body_with_options_omits_context_fields() {
    let data = json!({
        "type": "agent-turn-complete",
        "thread-id": "abc-123-def",
        "turn-id": "turn-789",
        "cwd": "/tmp/project",
        "input-messages": ["你好"],
        "last-assistant-message": "你好，有什么可以帮你的？",
    });

    let body = build_email_text_body_with_options("codex", "agent-turn-complete", &data, false);

    assert!(!body.contains("线程ID:"));
    assert!(!body.contains("轮次ID:"));
    assert!(!body.contains("工作目录:"));
    assert!(body.contains("事件类型: agent-turn-complete"));
}

#[test]
fn test_build_email_html_omits_context_by_default() {
    let data = json!({
        "type": "agent-turn-complete",
        "thread-id": "abc-123-def",
        "turn-id": "turn-789",
        "cwd": "/tmp/project",
        "input-messages": ["你好"],
        "last-assistant-message": "你好，有什么可以帮你的？",
    });

    let html = build_email_html("Codex 任务完成", "codex", &data);

    assert!(html.contains("#059669"));
    assert!(!html.contains("abc-123-def"));
    assert!(!html.contains("turn-789"));
    assert!(!html.contains("/tmp/project"));
    assert!(html.contains("agent-turn-complete"));
    assert!(html.contains("copilot-stop-notify"));
}

#[test]
fn test_build_email_html_with_options_uses_codex_theme_and_metadata() {
    let data = json!({
        "type": "agent-turn-complete",
        "thread-id": "abc-123-def",
        "turn-id": "turn-789",
        "cwd": "/tmp/project",
        "input-messages": ["你好"],
        "last-assistant-message": "你好，有什么可以帮你的？",
    });

    let html = build_email_html_with_options("Codex 任务完成", "codex", &data, true);

    assert!(html.contains("#059669"));
    assert!(html.contains("abc-123-def"));
    assert!(html.contains("turn-789"));
    assert!(html.contains("agent-turn-complete"));
    assert!(html.contains("你好"));
    assert!(html.contains("有什么可以帮你的"));
}

#[test]
fn test_build_email_html_with_options_omits_context_fields() {
    let data = json!({
        "type": "agent-turn-complete",
        "thread-id": "abc-123-def",
        "turn-id": "turn-789",
        "cwd": "/tmp/project",
        "input-messages": ["你好"],
        "last-assistant-message": "你好，有什么可以帮你的？",
    });

    let html = build_email_html_with_options("Codex 任务完成", "codex", &data, false);

    assert!(!html.contains("abc-123-def"));
    assert!(!html.contains("turn-789"));
    assert!(!html.contains("/tmp/project"));
    assert!(html.contains("agent-turn-complete"));
}

#[test]
fn test_build_email_html_renders_bold_markup() {
    let data = json!({
        "transcript": [
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {"type": "text", "text": "这是**重点**内容"}
                    ]
                }
            }
        ]
    });

    let html = build_email_html("标题", "claude-code", &data);

    assert!(html.contains("<strong>重点</strong>"));
}
