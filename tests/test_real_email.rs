// 真实邮件发送测试：使用实际 SMTP 配置发送邮件
// 运行方式: cargo test --test test_real_email -- --nocapture

use std::path::PathBuf;

/// 定位真实 SMTP 配置文件路径
fn real_env_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("COPILOT_STOP_NOTIF_ENV") {
        let candidate = PathBuf::from(path);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut candidates = vec![
        manifest_dir.join("copilot-stop-notif.env"),
        manifest_dir
            .join("hooks")
            .join("copilot-stop-notif")
            .join("copilot-stop-notif.env"),
    ];

    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        candidates.push(
            PathBuf::from(user_profile)
                .join(".copilot")
                .join("hooks")
                .join("copilot-stop-notif")
                .join("copilot-stop-notif.env"),
        );
    }

    candidates.into_iter().find(|path| path.is_file())
}

/// 构建模拟的多轮对话内容
fn build_mock_turns() -> Vec<copilot_stop_notify::transcript::Turn> {
    vec![
        copilot_stop_notify::transcript::Turn {
            role: "user".into(),
            content: "请帮我写一个 Rust 的 Hello World 程序".into(),
        },
        copilot_stop_notify::transcript::Turn {
            role: "assistant".into(),
            content: r#"好的，这是一个简单的 Rust 程序：

```rust
fn main() {
    println!("Hello, world!");
}
```

你可以通过以下步骤运行：
1. 创建项目：`cargo new hello`
2. 进入目录：`cd hello`
3. 运行程序：`cargo run`

**注意**：确保你已经安装了 Rust 工具链。"#
                .into(),
        },
        copilot_stop_notify::transcript::Turn {
            role: "user".into(),
            content: "谢谢！还有一个问题：\n- 如何添加第三方依赖？\n- 如何发布到 crates.io？".into(),
        },
        copilot_stop_notify::transcript::Turn {
            role: "assistant".into(),
            content: r#"## 添加依赖

在 `Cargo.toml` 的 `[dependencies]` 部分添加：

```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
```

## 发布到 crates.io

1. 注册 crates.io 帐号
2. 登录：`cargo login <your-token>`
3. 发布：`cargo publish`

*注意*：发布前请确保 `Cargo.toml` 中包含 `license`、`description` 等必填字段。"#
                .into(),
        },
        copilot_stop_notify::transcript::Turn {
            role: "tool".into(),
            content: "执行命令：cargo build\n退出码：0\n编译成功，无警告".into(),
        },
        copilot_stop_notify::transcript::Turn {
            role: "assistant".into(),
            content: "编译成功！项目已经可以正常运行了。".into(),
        },
    ]
}

fn mask_email(email: &str) -> String {
    let Some((local, domain)) = email.split_once('@') else {
        return email.to_string();
    };
    let first = local.chars().next().unwrap_or('*');
    format!("{}***@{}", first, domain)
}

#[ignore = "需要真实 SMTP 凭据，避免在常规测试中误发邮件"]
#[test]
fn test_real_smtp_send() {
    let Some(env_path) = real_env_path() else {
        eprintln!("[跳过] 未找到真实 SMTP 配置文件");
        return;
    };

    eprintln!("[测试] 使用配置文件: {}", env_path.display());

    // 1. 加载真实配置
    let cfg = copilot_stop_notify::config::Config::load(&env_path)
        .expect("加载配置失败");

    eprintln!("[测试] SMTP: {}:{} (SSL={})", cfg.smtp.host, cfg.smtp.port, cfg.smtp.use_ssl);
    eprintln!("[测试] 发件人: {}", mask_email(&cfg.email.from));
    eprintln!(
        "[测试] 收件人: {:?}",
        cfg.email.to.iter().map(|addr| mask_email(addr)).collect::<Vec<_>>()
    );

    // 2. 构建模拟对话
    let turns = build_mock_turns();
    eprintln!("[测试] 构建了 {} 轮模拟对话", turns.len());

    // 3. 渲染 HTML
    let html = copilot_stop_notify::html::render_email_html(
        &turns,
        Some("test-session-real-001"),
        Some("2026-04-03T16:00:00.000Z"),
        Some("D:\\project\\example-workspace"),
        true,
    );

    eprintln!("[测试] HTML 渲染完成，长度: {} bytes", html.len());
    assert!(html.contains("<!DOCTYPE html>"), "HTML 缺少 DOCTYPE");
    assert!(html.contains("Copilot 会话回顾"), "HTML 缺少标题");

    // 4. 发送真实邮件
    let subject = "[测试] Copilot 会话回顾 - 真实发信测试 2026-04-03";
    let result = copilot_stop_notify::email::send_email(
        &cfg.smtp,
        &cfg.email.from,
        &cfg.email.to,
        subject,
        &html,
    );

    match &result {
        Ok(()) => eprintln!("[测试] 邮件发送成功！"),
        Err(e) => eprintln!("[测试] 邮件发送失败: {}", e),
    }

    result.expect("真实邮件发送应成功");
}
