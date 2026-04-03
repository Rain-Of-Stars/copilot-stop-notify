use copilot_stop_notify::html::render_email_html;
use copilot_stop_notify::transcript::Turn;

#[test]
fn test_render_to_file() {
    let turns = vec![
        Turn {
            role: "user".into(),
            content: "帮我用 Rust 写一个并发下载器。要求用到 tokio 和 reqwest。".into(),
        },
        Turn {
            role: "assistant".into(),
            content: "好的，这是一个使用 `tokio` 和 `reqwest` 的并发下载器示例：\n\n```rust\n#[tokio::main]\nasync fn main() {\n    // ...\n}\n```\n\n这里使用了 `FuturesUnordered` 来管理并发任务。".into(),
        },
        Turn {
            role: "tool".into(),
            content: r"Running \cargo add tokio reqwest\... Success.".into(),
        },
        Turn {
            role: "assistant".into(),
            content: "依赖添加完成。".into(),
        },
    ];

    let html = render_email_html(
        &turns,
        Some("sess-12345"),
        Some("2026-04-03T16:00:00Z"),
        Some("D:\\project\\example-workspace"),
        true
    );

    let output = std::env::temp_dir().join("copilot-stop-notify-test.html");
    std::fs::write(&output, html).unwrap();
    eprintln!("测试 HTML 已生成: {}", output.display());
}
