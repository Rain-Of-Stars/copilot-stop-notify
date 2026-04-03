// copilot-stop-notify: VS Code Copilot 会话结束后自动发送邮件通知
// 作为 VS Code Hook 运行，从 stdin 接收事件 JSON，读取 transcript，渲染 HTML 邮件并发送

mod config;
mod dedup;
mod email;
mod event;
mod html;
mod redact;
mod transcript;

use std::path::PathBuf;
use std::process;
use redact::{redact_sensitive_text, safe_prefix, summarize_path_for_display};

/// 解析 --env-file 命令行参数
fn parse_env_file_arg() -> Option<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--env-file" {
            if let Some(path) = args.get(i + 1) {
                return Some(PathBuf::from(path));
            }
        }
        i += 1;
    }
    None
}

fn main() {
    // Hook 输出必须是合法 JSON，无论成功还是失败都在 stdout 输出 {}
    match run() {
        Ok(()) => {
            println!("{{}}");
            process::exit(0);
        }
        Err(e) => {
            eprintln!("[copilot-stop-notify 错误] {}", redact_sensitive_text(&e));
            println!("{{}}");
            // 退出码 0：不阻塞 VS Code 正常流程
            process::exit(0);
        }
    }
}

fn run() -> Result<(), String> {
    // 1. 读取并解析 stdin
    let input = event::read_hook_input()?;

    // 2. 过滤事件：仅处理 Stop，忽略 SubagentStop 和其他
    if !event::should_process(&input)? {
        eprintln!(
            "[copilot-stop-notify] 跳过事件: {} (stop_hook_active={:?})",
            input.hook_event_name,
            input.stop_hook_active
        );
        return Ok(());
    }

    let session_for_log = input
        .session_id
        .as_deref()
        .map(|sid| safe_prefix(sid, 12))
        .unwrap_or_else(|| "未知".to_string());
    eprintln!("[copilot-stop-notify] 处理 Stop 事件，会话: {}", session_for_log);

    // 3. 加载配置（优先使用 --env-file 指定的路径）
    let env_path = if let Some(p) = parse_env_file_arg() {
        if p.is_file() {
            p
        } else {
            return Err(format!(
                "指定的 env 文件不存在: {}",
                summarize_path_for_display(&p)
            ));
        }
    } else {
        config::find_env_file()?
    };
    let cfg = config::Config::load(&env_path)?;

    // 4. 验证 transcript 路径
    let transcript_str = input.transcript_path.as_deref().unwrap(); // should_process 已确保存在
    let transcript_path = transcript::validate_path(transcript_str, &cfg.transcript.allowed_roots)?;

    // 5. 等待 transcript 文件稳定并收束为完整助手回复
    let snapshot = transcript::wait_for_complete_transcript(&transcript_path)?;

    // 6. 解析 transcript
    let turns = snapshot.turns;
    if turns.is_empty() {
        eprintln!("[copilot-stop-notify] transcript 无有效内容，跳过发送");
        return Ok(());
    }

    eprintln!("[copilot-stop-notify] 解析到 {} 轮对话", turns.len());

    // 7. 子智能体结束只代表内部迭代完成，不应抢先触发 Stop 邮件
    if transcript::is_subagent_iteration(&turns) {
        eprintln!(
            "[copilot-stop-notify] 检测到子智能体 </final_answer> 收尾，跳过本次发送并保留后续主会话通知机会"
        );
        return Ok(());
    }

    // 8. Session 幂等去重（基于 session_id + 轮次数 + transcript 指纹）
    //    续聊或同轮次内容收敛变化时允许再次发送
    let turn_count = turns.len();
    if let Some(ref sid) = input.session_id {
        if dedup::is_duplicate(sid, turn_count, &snapshot.fingerprint) {
            eprintln!(
                "[copilot-stop-notify] 会话 {} 已发送过通知（轮次数 {} 与内容指纹均未变化），跳过",
                safe_prefix(sid, 12),
                turn_count
            );
            return Ok(());
        }
    }

    // 9. 渲染 HTML 邮件
    let email_html = html::render_email_html(
        &turns,
        input.session_id.as_deref(),
        input.timestamp.as_deref(),
        input.cwd.as_deref(),
        cfg.email.include_context,
    );

    // 10. 构建邮件主题
    let subject = build_subject(&input);

    // 11. 发送邮件
    email::send_email(
        &cfg.smtp,
        &cfg.email.from,
        &cfg.email.to,
        &subject,
        &email_html,
    )?;

    let recipients: Vec<String> = cfg
        .email
        .to
        .iter()
        .map(|addr| redact_sensitive_text(addr))
        .collect();
    eprintln!("[copilot-stop-notify] 邮件发送成功，收件人: {:?}", recipients);

    // 12. 标记已发送（去重，记录轮次数和内容指纹）
    if let Some(ref sid) = input.session_id {
        dedup::mark_sent(sid, turn_count, &snapshot.fingerprint)?;
    }

    Ok(())
}

/// 构建邮件主题
fn build_subject(input: &event::HookInput) -> String {
    let time_part = input
        .timestamp
        .as_deref()
        .and_then(|t| t.split('T').next())
        .unwrap_or("未知时间");

    let session_part = input
        .session_id
        .as_deref()
        .map(|s| {
            // 用 char_indices 安全截取，避免多字节 UTF-8 字符边界 panic
            match s.char_indices().nth(8) {
                Some((i, _)) => &s[..i],
                None => s,
            }
        })
        .unwrap_or("未知");

    format!(
        "[Copilot 会话回顾] {} ({})",
        time_part, session_part
    )
}
