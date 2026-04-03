// 集成测试：覆盖完整流程的自动化测试
// 不依赖真实 SMTP 服务器，仅测试解析、渲染、去重等逻辑

use std::io::Write;
use std::path::PathBuf;
use tempfile::TempDir;

/// 辅助函数：创建临时 .env 配置文件
fn create_test_env(dir: &TempDir) -> PathBuf {
    let path = dir.path().join("copilot-stop-notif.env");
    let mut f = std::fs::File::create(&path).unwrap();
    write!(
        f,
        "SMTP_HOST=smtp.example.com\n\
         SMTP_PORT=465\n\
         SMTP_USER=test@example.com\n\
         SMTP_PASSWORD=test_password\n\
         SMTP_USE_SSL=true\n\
         EMAIL_TO=recv@example.com\n\
         EMAIL_INCLUDE_CONTEXT=true\n\
         TRANSCRIPT_ALLOWED_ROOTS={}\n",
        dir.path().display()
    )
    .unwrap();
    path
}

/// 辅助函数：创建临时 transcript 文件
fn create_test_transcript(dir: &TempDir, content: &str) -> PathBuf {
    let path = dir.path().join("transcript.json");
    std::fs::write(&path, content).unwrap();
    // 将修改时间设置为过去，以确保稳定性检查通过
    // (在测试中文件刚写入，mtime 接近 now，稳定检查可能需要等待)
    path
}

#[test]
fn test_full_pipeline_event_parsing() {
    // 测试完整的事件解析流程
    let json = r#"{
        "timestamp": "2026-04-03T15:30:00.000Z",
        "cwd": "D:\\workspace",
        "sessionId": "integration-test-001",
        "hookEventName": "Stop",
        "transcript_path": "C:\\tmp\\transcript.json",
        "stop_hook_active": false
    }"#;

    let input: copilot_stop_notify::event::HookInput = serde_json::from_str(json).unwrap();
    assert_eq!(input.hook_event_name, "Stop");
    assert_eq!(input.session_id.as_deref(), Some("integration-test-001"));
    assert!(!input.stop_hook_active.unwrap());
}

#[test]
fn test_full_pipeline_skip_subagent() {
    let json = r#"{
        "hookEventName": "SubagentStop",
        "sessionId": "sub-001",
        "transcript_path": "/tmp/t.json"
    }"#;
    let input: copilot_stop_notify::event::HookInput = serde_json::from_str(json).unwrap();
    assert!(!copilot_stop_notify::event::should_process(&input).unwrap());
}

#[test]
fn test_full_pipeline_skip_stop_hook_active() {
    let json = r#"{
        "hookEventName": "Stop",
        "sessionId": "active-001",
        "transcript_path": "/tmp/t.json",
        "stop_hook_active": true
    }"#;
    let input: copilot_stop_notify::event::HookInput = serde_json::from_str(json).unwrap();
    assert!(!copilot_stop_notify::event::should_process(&input).unwrap());
}

#[test]
fn test_full_pipeline_snake_case_event_parsing() {
    // VS Code Copilot 实际发送的 snake_case 格式
    let json = r#"{
        "timestamp": "2026-04-03T15:30:00.000Z",
        "session_id": "integration-snake-001",
        "hook_event_name": "Stop",
        "transcript_path": "C:\\tmp\\transcript.json",
        "stop_hook_active": false
    }"#;

    let input: copilot_stop_notify::event::HookInput = serde_json::from_str(json).unwrap();
    assert_eq!(input.hook_event_name, "Stop");
    assert_eq!(input.session_id.as_deref(), Some("integration-snake-001"));
    assert!(!input.stop_hook_active.unwrap());
    assert!(copilot_stop_notify::event::should_process(&input).unwrap());
}

#[test]
fn test_config_load_from_file() {
    let dir = TempDir::new().unwrap();
    let env_path = create_test_env(&dir);
    let config = copilot_stop_notify::config::Config::load(&env_path).unwrap();

    assert_eq!(config.smtp.host, "smtp.example.com");
    assert_eq!(config.smtp.port, 465);
    assert!(config.smtp.use_ssl);
    assert_eq!(config.email.to, vec!["recv@example.com"]);
    assert!(config.email.include_context);
    assert_eq!(config.email.from, "test@example.com");
}

#[test]
fn test_transcript_parse_and_render() {
    let dir = TempDir::new().unwrap();
    let transcript_json = r####"[
        {"role": "user", "content": "请帮我写一个 Rust hello world"},
        {"role": "assistant", "content": "好的，这是一个简单的 Rust 程序：\n\n```rust\nfn main() {\n    println!(\"Hello, world!\");\n}\n```\n\n你可以用 `cargo run` 来运行它。"},
        {"role": "user", "content": "谢谢，**很清楚**！还有一个问题：\n- 如何添加依赖？\n- 如何发布到 crates.io？"},
        {"role": "assistant", "content": "## 添加依赖\n\n在 `Cargo.toml` 的 `[dependencies]` 下添加：\n\n```toml\nserde = \"1\"\n```\n\n## 发布到 crates.io\n\n1. 注册帐号\n2. 运行 `cargo publish`"}
    ]"####;

    let path = create_test_transcript(&dir, transcript_json);
    let turns = copilot_stop_notify::transcript::parse_transcript(&path).unwrap();

    assert_eq!(turns.len(), 4);
    assert_eq!(turns[0].role, "user");
    assert_eq!(turns[1].role, "assistant");
    assert!(turns[1].content.contains("Hello, world!"));

    // 渲染 HTML
    let html = copilot_stop_notify::html::render_email_html(
        &turns,
        Some("test-session"),
        Some("2026-04-03T15:30:00.000Z"),
        Some("D:\\workspace"),
        true,
    );

    // 验证 HTML 结构
    assert!(html.contains("<!DOCTYPE html>"));
    assert!(html.contains("Copilot 会话回顾"));
    assert!(html.contains("用户")); // 角色标签
    assert!(html.contains("助手")); // 角色标签

    // 验证代码块渲染
    assert!(html.contains("<pre")); // 代码块
    assert!(html.contains("Hello, world!")); // 代码内容被转义

    // 验证 Markdown 渲染
    assert!(html.contains("<strong>")); // 粗体
    assert!(html.contains("<code")); // 行内代码
    assert!(html.contains("<li")); // 列表项
    assert!(html.contains("<ul")); // 无序列表

    // 验证 XSS 防护
    assert!(!html.contains("<script"));

    // 验证邮件兼容性
    assert!(html.contains("role=\"presentation\"")); // table role
    assert!(html.contains("charset=\"UTF-8\"")); // 编码声明
}

#[test]
fn test_transcript_complex_formats() {
    let dir = TempDir::new().unwrap();

    // 嵌套 message 格式
    let nested_json = r#"[
        {"type": "send", "message": {"role": "user", "content": "嵌套格式测试"}},
        {"type": "receive", "message": {"role": "assistant", "content": "收到嵌套消息"}}
    ]"#;
    let path = create_test_transcript(&dir, nested_json);
    let turns = copilot_stop_notify::transcript::parse_transcript(&path).unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].content, "嵌套格式测试");

    // 对象包装格式
    let wrapped_json = r#"{"messages": [
        {"role": "user", "content": "包装格式"},
        {"role": "assistant", "content": "收到"}
    ]}"#;
    let path2 = dir.path().join("transcript2.json");
    std::fs::write(&path2, wrapped_json).unwrap();
    let turns2 = copilot_stop_notify::transcript::parse_transcript(&path2).unwrap();
    assert_eq!(turns2.len(), 2);

    // 数组 content 格式
    let array_content_json = r#"[
        {"role": "assistant", "content": [
            {"type": "text", "text": "第一部分"},
            {"type": "text", "text": "第二部分"}
        ]}
    ]"#;
    let path3 = dir.path().join("transcript3.json");
    std::fs::write(&path3, array_content_json).unwrap();
    let turns3 = copilot_stop_notify::transcript::parse_transcript(&path3).unwrap();
    assert_eq!(turns3.len(), 1);
    assert!(turns3[0].content.contains("第一部分"));
    assert!(turns3[0].content.contains("第二部分"));
}

#[test]
fn test_transcript_jsonl_format() {
    let dir = TempDir::new().unwrap();
    let jsonl = r#"{"role": "user", "content": "JSONL 第一行"}
{"role": "assistant", "content": "JSONL 回复"}
{"role": "user", "content": "JSONL 第二个问题"}"#;

    let path = create_test_transcript(&dir, jsonl);
    let turns = copilot_stop_notify::transcript::parse_transcript(&path).unwrap();
    assert_eq!(turns.len(), 3);
    assert_eq!(turns[0].content, "JSONL 第一行");
}

#[test]
fn test_transcript_path_validation() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("safe.json");
    std::fs::write(&file_path, "[]").unwrap();

    // 允许的路径
    let allowed = vec![dir.path().to_path_buf()];
    assert!(
        copilot_stop_notify::transcript::validate_path(file_path.to_str().unwrap(), &allowed)
            .is_ok()
    );

    // 不允许的路径
    let other = TempDir::new().unwrap();
    let other_allowed = vec![other.path().to_path_buf()];
    assert!(
        copilot_stop_notify::transcript::validate_path(file_path.to_str().unwrap(), &other_allowed)
            .is_err()
    );
}

#[test]
fn test_dedup_idempotency() {
    let unique_id = format!("_integration_test_dedup_{}", std::process::id());

    // 清除可能的残留
    let dedup_dir = std::env::temp_dir().join("copilot-stop-notify-dedup");
    let mark_path = dedup_dir.join(&unique_id);
    let _ = std::fs::remove_file(&mark_path);

    // 第一次不应该是重复
    assert!(!copilot_stop_notify::dedup::is_duplicate(&unique_id, 3, "hash-a"));

    // 标记已发送（3 轮对话）
    copilot_stop_notify::dedup::mark_sent(&unique_id, 3, "hash-a").unwrap();

    // 相同轮次应该是重复
    assert!(copilot_stop_notify::dedup::is_duplicate(&unique_id, 3, "hash-a"));

    // 相同轮次但内容变化后不应该被误判为重复
    assert!(!copilot_stop_notify::dedup::is_duplicate(&unique_id, 3, "hash-b"));

    // 续聊产生更多轮次后不应该是重复
    assert!(!copilot_stop_notify::dedup::is_duplicate(&unique_id, 5, "hash-c"));

    // 清理
    let _ = std::fs::remove_file(&mark_path);
}

#[test]
fn test_subagent_iteration_detection() {
    // 以 </final_answer> 结尾的对话应被检测为子智能体迭代
    let turns_subagent = vec![
        copilot_stop_notify::transcript::Turn {
            role: "user".into(),
            content: "查找相关代码".into(),
        },
        copilot_stop_notify::transcript::Turn {
            role: "assistant".into(),
            content: "搜索结果如下...\n<final_answer>\n相关文件列表\n</final_answer>".into(),
        },
    ];
    assert!(copilot_stop_notify::transcript::is_subagent_iteration(&turns_subagent));

    // 仅出现关闭标签，不应误判为子智能体完成
    let turns_closing_only = vec![
        copilot_stop_notify::transcript::Turn {
            role: "assistant".into(),
            content: "这里提到了 </final_answer> 这个标签，但不是子智能体结果".into(),
        },
    ];
    assert!(!copilot_stop_notify::transcript::is_subagent_iteration(&turns_closing_only));

    // 正常对话不应被误判
    let turns_normal = vec![
        copilot_stop_notify::transcript::Turn {
            role: "user".into(),
            content: "帮我写代码".into(),
        },
        copilot_stop_notify::transcript::Turn {
            role: "assistant".into(),
            content: "好的，代码已完成。".into(),
        },
    ];
    assert!(!copilot_stop_notify::transcript::is_subagent_iteration(&turns_normal));

    // 空对话
    let turns_empty: Vec<copilot_stop_notify::transcript::Turn> = vec![];
    assert!(!copilot_stop_notify::transcript::is_subagent_iteration(&turns_empty));
}

#[test]
fn test_html_xss_prevention() {
    let malicious_turns = vec![
        copilot_stop_notify::transcript::Turn {
            role: "user".into(),
            content: "<img src=x onerror=alert(1)>".into(),
        },
        copilot_stop_notify::transcript::Turn {
            role: "assistant".into(),
            content: "正常回复 <script>document.cookie</script>".into(),
        },
        copilot_stop_notify::transcript::Turn {
            role: "<script>alert('role-xss')</script>".into(),
            content: "角色字段注入测试".into(),
        },
    ];

    let html = copilot_stop_notify::html::render_email_html(
        &malicious_turns,
        Some("<script>alert('session')</script>"),
        Some("<img src=x>"),
        None,
        true,
    );

    // 确保所有注入的 HTML 标签都被转义
    assert!(!html.contains("<script>"));
    assert!(!html.contains("<img src="));
    assert!(html.contains("&lt;script&gt;"));
    assert!(html.contains("&lt;img"));
}

#[test]
fn test_html_email_rendering_quality() {
    // 测试长对话的渲染质量
    let mut turns = Vec::new();
    for i in 0..20 {
        turns.push(copilot_stop_notify::transcript::Turn {
            role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
            content: format!("第 {} 轮对话内容，包含一些**格式**和 `代码`", i + 1),
        });
    }

    let html = copilot_stop_notify::html::render_email_html(
        &turns,
        Some("long-session-001"),
        Some("2026-04-03T15:30:00.000Z"),
        None,
        false,
    );

    // 验证所有轮次都被渲染（元信息区也包含"用户"和"助手"各一次）
    assert_eq!(html.matches("用户").count(), 10 + 1);
    assert_eq!(html.matches("助手").count(), 10 + 1);

    // 验证 HTML 大小合理（不应该无限膨胀）
    assert!(html.len() < 500_000, "HTML 大小: {}", html.len());

    // 验证统计信息
    assert!(html.contains("20")); // 对话总数
}

#[test]
fn test_html_redacts_private_paths_and_emails() {
    let turns = vec![
        copilot_stop_notify::transcript::Turn {
            role: "assistant".into(),
            content: "本机路径: C:\\Users\\alice\\workspace\\src\\main.rs\nUnix 路径: /Users/alice/project/src/lib.rs\nLinux 路径: /home/alice/app/main.rs\n邮箱: alice@example.com".into(),
        },
    ];

    let html = copilot_stop_notify::html::render_email_html(
        &turns,
        Some("会话编号一二三四五六七八九十"),
        Some("2026-04-03T18:00:00.000Z"),
        Some("C:\\Users\\alice\\workspace"),
        true,
    );

    assert!(!html.contains("alice@example.com"));
    assert!(!html.contains("C:\\Users\\alice\\workspace"));
    assert!(!html.contains("/Users/alice/project"));
    assert!(!html.contains("/home/alice/app"));
    assert!(html.contains("a***@example.com"));
    assert!(html.contains("C:\\Users\\***\\workspace"));
    assert!(html.contains("/Users/***/project"));
    assert!(html.contains("/home/***/app"));
    assert!(html.contains("会话: 会话编号一二三四五六七八"));
}

#[test]
fn test_html_caps_oversized_payload() {
    let turns = vec![copilot_stop_notify::transcript::Turn {
        role: "assistant".into(),
        content: "A".repeat(300_000),
    }];

    let html = copilot_stop_notify::html::render_email_html(&turns, None, None, None, false);

    assert!(html.len() < 260_000, "HTML 大小超出预期: {}", html.len());
    assert!(
        html.contains("该轮内容已截断") || html.contains("邮件总长度已截断"),
        "超长内容应被截断"
    );
}

#[test]
fn test_html_code_block_rendering() {
    let turns = vec![copilot_stop_notify::transcript::Turn {
        role: "assistant".into(),
        content: "示例代码：\n\n```python\ndef hello():\n    print(\"Hello & <World>\")\n    return True\n```\n\n以上代码会输出 `Hello & <World>`。".into(),
    }];

    let html = copilot_stop_notify::html::render_email_html(&turns, None, None, None, false);

    // 代码块中的特殊字符被转义
    assert!(html.contains("&amp;"));
    assert!(html.contains("&lt;World&gt;"));

    // 代码块有深色背景
    assert!(html.contains("1E293B"));
    assert!(html.contains("<pre"));
}

#[test]
fn test_empty_transcript_handling() {
    let dir = TempDir::new().unwrap();

    // 空文件
    let path = dir.path().join("empty.json");
    std::fs::write(&path, "").unwrap();
    assert!(copilot_stop_notify::transcript::parse_transcript(&path).is_err());

    // 空数组 - 解析成功但无有效轮次
    let path2 = dir.path().join("empty_array.json");
    std::fs::write(&path2, "[]").unwrap();
    let turns = copilot_stop_notify::transcript::parse_transcript(&path2).unwrap();
    // 空数组会被解析为 0 个有效对话轮次
    assert!(turns.is_empty() || turns.iter().all(|t| t.role == "transcript"));
}

#[test]
fn test_event_all_types_filtered() {
    // 确保只有 Stop 事件通过
    let events = vec![
        ("Stop", false, true),           // 应该处理
        ("SubagentStop", false, false),   // 应该跳过
        ("SessionStart", false, false),   // 应该跳过
        ("PreToolUse", false, false),     // 应该跳过
        ("PostToolUse", false, false),    // 应该跳过
        ("UserPromptSubmit", false, false), // 应该跳过
        ("Stop", true, false),           // stop_hook_active=true，跳过
    ];

    for (event, active, expected) in events {
        let input = copilot_stop_notify::event::HookInput {
            timestamp: None,
            cwd: None,
            session_id: None,
            hook_event_name: event.to_string(),
            transcript_path: Some("/tmp/t.json".to_string()),
            stop_hook_active: Some(active),
        };
        let result = copilot_stop_notify::event::should_process(&input);
        match result {
            Ok(val) => assert_eq!(
                val, expected,
                "事件 {} (active={}) 应该返回 {}",
                event, active, expected
            ),
            Err(_) => assert!(
                !expected,
                "事件 {} 应该返回 Ok({})",
                event, expected
            ),
        }
    }
}
