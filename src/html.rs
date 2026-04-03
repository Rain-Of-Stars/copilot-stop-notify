// HTML 渲染模块：将对话轮次渲染为邮件 HTML
use crate::redact::{redact_sensitive_text, safe_prefix};
use crate::transcript::Turn;

const MAX_RENDERED_TOTAL_CHARS: usize = 220_000;
const MAX_RENDERED_CHARS_PER_TURN: usize = 20_000;

struct RoleStyle {
    label: &'static str,
    icon: &'static str,
    border_color: &'static str,
    bg_color: &'static str,
    text_color: &'static str,
    icon_color: &'static str,
}

struct PreparedTurnContent {
    content: String,
    rendered_chars: usize,
    was_truncated: bool,
    stop_rendering: bool,
}

fn role_style(role: &str) -> RoleStyle {
    match role.to_lowercase().as_str() {
        "user" => RoleStyle {
            label: "用户",
            icon: "●",
            border_color: "#E5E7EB",
            bg_color: "#FFFFFF",
            text_color: "#111827",
            icon_color: "#9CA3AF",
        },
        "assistant" => RoleStyle {
            label: "Copilot 助手",
            icon: "✦",
            border_color: "#E0E7FF",
            bg_color: "#F8FAFC",
            text_color: "#334155",
            icon_color: "#4F46E5",
        },
        "system" => RoleStyle {
            label: "系统",
            icon: "⚙",
            border_color: "#FEF3C7",
            bg_color: "#FFFBEB",
            text_color: "#92400E",
            icon_color: "#D97706",
        },
        "tool" => RoleStyle {
            label: "工具",
            icon: "🛠",
            border_color: "#E5E7EB",
            bg_color: "#F9FAFB",
            text_color: "#4B5563",
            icon_color: "#6B7280",
        },
        _ => RoleStyle {
            label: "其他",
            icon: "◦",
            border_color: "#E5E7EB",
            bg_color: "#F9FAFB",
            text_color: "#4B5563",
            icon_color: "#6B7280",
        },
    }
}

pub fn render_email_html(
    turns: &[Turn],
    session_id: Option<&str>,
    timestamp: Option<&str>,
    cwd: Option<&str>,
    include_context: bool,
) -> String {
    let total = turns.len();
    let user_count = turns.iter().filter(|turn| turn.role == "user").count();
    let assistant_count = turns.iter().filter(|turn| turn.role == "assistant").count();
    let tool_count = turns.iter().filter(|turn| turn.role == "tool").count();
    let mut rendered_html = String::new();
    let mut remaining_chars = MAX_RENDERED_TOTAL_CHARS;
    let mut rendered_turns = 0usize;
    let mut omitted_turns = 0usize;
    let mut truncated_any = false;

    for turn in turns {
        if remaining_chars == 0 {
            omitted_turns += 1;
            continue;
        }

        let prepared = prepare_turn_content(&turn.content, remaining_chars);
        remaining_chars = remaining_chars.saturating_sub(prepared.rendered_chars);
        truncated_any |= prepared.was_truncated;
        rendered_html.push_str(&render_turn(&turn.role, &prepared.content, rendered_turns));
        rendered_turns += 1;

        if prepared.stop_rendering {
            omitted_turns += total.saturating_sub(rendered_turns);
            break;
        }
    }

    let notice_html = build_content_notice(truncated_any, omitted_turns);
    let session_html = session_id
        .map(|id| format!(" &nbsp;&middot;&nbsp; 会话: {}", escape_html(&safe_prefix(id, 12))))
        .unwrap_or_default();
    let path_html = if include_context {
        cwd.map(|path| format!("<br/>目录: {}", escape_html(&redact_sensitive_text(path))))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let content_html = if rendered_html.is_empty() {
        "<div style=\"margin-top:14px;padding:18px;border:1px dashed #CBD5E1;border-radius:12px;color:#64748B;text-align:center;\">无可展示的会话内容</div>".to_string()
    } else {
        rendered_html
    };

    format!(
        r##"<!DOCTYPE html>
<html lang="zh-CN" xmlns="http://www.w3.org/1999/xhtml">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Copilot 会话回顾</title>
  <!--[if mso]><style>table,td{{mso-table-lspace:0pt;mso-table-rspace:0pt;}}</style><![endif]-->
  <style>
    body, table, td, p, div {{ margin: 0; padding: 0; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif; }}
    body {{ background: #F1F5F9; color: #0F172A; -webkit-text-size-adjust: 100%; -ms-text-size-adjust: 100%; }}
    pre {{ margin: 12px 0 0 0; padding: 14px 16px; background: #1E293B; color: #E2E8F0; border-radius: 10px; overflow-x: auto; white-space: pre-wrap; word-break: break-word; font-size: 12px; line-height: 1.65; }}
    code {{ background: #EEF2FF; color: #312E81; padding: 1px 6px; border-radius: 6px; font-size: 12px; }}
    pre code {{ background: transparent; color: inherit; padding: 0; border-radius: 0; }}
    @media only screen and (max-width: 640px) {{
      .wrapper {{ padding: 0 4px !important; }}
      .panel {{ border-radius: 0 !important; }}
      .header-cell {{ padding: 22px 16px 18px 16px !important; }}
      .stats-cell {{ padding: 12px 16px !important; }}
      .content-cell {{ padding: 8px 12px 20px 12px !important; }}
      .footer-cell {{ padding: 16px 12px 18px 12px !important; }}
      .message-card {{ padding: 12px 14px !important; }}
    }}
  </style>
</head>
<body>
  <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0">
    <tr>
      <td align="center" style="padding: 24px 12px;">
        <table role="presentation" class="wrapper" width="100%" cellpadding="0" cellspacing="0" border="0" style="max-width: 880px;">
          <tr>
            <td class="panel" style="background: #FFFFFF; border: 1px solid #E2E8F0; border-radius: 20px; overflow: hidden; box-shadow: 0 18px 45px rgba(15, 23, 42, 0.08);">
              <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0">
                <tr>
                  <td class="header-cell" style="padding: 28px 32px 22px 32px; background: linear-gradient(135deg, #111827 0%, #1F2937 60%, #334155 100%); color: #F8FAFC;">
                    <div style="font-size: 12px; letter-spacing: 0.12em; text-transform: uppercase; color: #CBD5E1;">VS Code Hook</div>
                    <div style="margin-top: 10px; font-size: 30px; line-height: 1.2; font-weight: 700;">Copilot 会话回顾</div>
                    <div style="margin-top: 12px; font-size: 13px; line-height: 1.7; color: #E2E8F0;">时间: {timestamp}{session_html}{path_html}</div>
                  </td>
                </tr>
                <tr>
                  <td class="stats-cell" style="padding: 14px 32px; background: #F8FAFC; border-bottom: 1px solid #E2E8F0;">
                    <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0">
                      <tr>
                        <td style="font-size: 13px; color: #475569;">总轮次 <strong style="color: #0F172A;">{total}</strong></td>
                        <td style="font-size: 13px; color: #475569;">用户 <strong style="color: #0F172A;">{user_count}</strong></td>
                        <td style="font-size: 13px; color: #475569;">助手 <strong style="color: #0F172A;">{assistant_count}</strong></td>
                        <td style="font-size: 13px; color: #475569;">工具 <strong style="color: #0F172A;">{tool_count}</strong></td>
                      </tr>
                    </table>
                  </td>
                </tr>
                <tr>
                  <td class="content-cell" style="padding: 8px 24px 24px 24px;">{notice_html}{content_html}</td>
                </tr>
                <tr>
                  <td class="footer-cell" style="padding: 18px 24px 24px 24px; border-top: 1px solid #E2E8F0; color: #94A3B8; font-size: 12px;">此邮件已自动处理敏感路径、邮箱以及超长内容，适合作为会话结束通知预览。</td>
                </tr>
              </table>
            </td>
          </tr>
        </table>
      </td>
    </tr>
  </table>
</body>
</html>"##,
        timestamp = escape_html(timestamp.unwrap_or("未知时间")),
        session_html = session_html,
        path_html = path_html,
        total = total,
        user_count = user_count,
        assistant_count = assistant_count,
        tool_count = tool_count,
        notice_html = notice_html,
        content_html = content_html,
    )
}

fn render_turn(role: &str, content: &str, index: usize) -> String {
    let style = role_style(role);
    let body_html = format_markdown(content);

    format!(
        r#"<div class="message-card" style="margin-top: 14px; padding: 16px 18px; border: 1px solid {border_color}; background: {bg_color}; border-radius: 16px;">
  <div style="display: flex; align-items: center; justify-content: space-between; gap: 12px;">
    <div style="display: flex; align-items: center; gap: 10px; min-width: 0;">
      <span style="font-size: 16px; line-height: 1; color: {icon_color};">{icon}</span>
      <span style="font-size: 13px; font-weight: 700; color: {text_color};">{label}</span>
    </div>
    <span style="font-size: 11px; color: #94A3B8; white-space: nowrap;">#{sequence}</span>
  </div>
  <div style="margin-top: 12px; font-size: 14px; line-height: 1.8; color: {text_color}; word-break: break-word;">{body_html}</div>
</div>"#,
        border_color = style.border_color,
        bg_color = style.bg_color,
        icon_color = style.icon_color,
        icon = style.icon,
        label = style.label,
        sequence = index + 1,
        text_color = style.text_color,
        body_html = body_html,
    )
}

fn prepare_turn_content(content: &str, remaining_chars: usize) -> PreparedTurnContent {
    let sanitized = redact_sensitive_text(content);
    let total_chars = sanitized.chars().count();
    let limit = remaining_chars.min(MAX_RENDERED_CHARS_PER_TURN);

    if total_chars <= limit {
        return PreparedTurnContent {
            content: sanitized,
            rendered_chars: total_chars,
            was_truncated: false,
            stop_rendering: false,
        };
    }

    let notice = if total_chars > MAX_RENDERED_CHARS_PER_TURN {
        "该轮内容已截断"
    } else {
        "邮件总长度已截断"
    };
    let reserve = notice.chars().count() + 4;
    let content_limit = limit.saturating_sub(reserve);
    let mut truncated = truncate_chars(&sanitized, content_limit);
    if !truncated.is_empty() {
        truncated.push_str("\n\n");
    }
    truncated.push('[');
    truncated.push_str(notice);
    truncated.push(']');

    PreparedTurnContent {
        content: truncated,
        rendered_chars: limit,
        was_truncated: true,
        stop_rendering: total_chars > remaining_chars && limit == remaining_chars,
    }
}

fn build_content_notice(truncated_any: bool, omitted_turns: usize) -> String {
    let mut parts = Vec::new();
    if truncated_any {
        parts.push("邮件内容中的本机路径、邮箱和超长片段已自动脱敏或截断".to_string());
    }
    if omitted_turns > 0 {
        parts.push(format!("另有 {} 轮未写入邮件以控制体积", omitted_turns));
    }
    if parts.is_empty() {
        return String::new();
    }

    format!(
        "<div style=\"margin-top: 14px; padding: 10px 12px; border: 1px solid #FDE68A; background: #FFFBEB; border-radius: 10px; color: #92400E; font-size: 12px; line-height: 1.65;\">{}</div>",
        escape_html(&parts.join("；"))
    )
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn format_markdown(text: &str) -> String {
    let html = escape_html(text);
    let parts: Vec<&str> = html.split("```").collect();
    let has_unclosed_triple = parts.len() % 2 == 0;
    let mut result = String::new();

    for (index, part) in parts.iter().enumerate() {
        let is_unclosed_last = has_unclosed_triple && index == parts.len() - 1;
        if index % 2 == 1 && !is_unclosed_last {
            let split_pos = part.find('\n').unwrap_or(0);
            let (_, code) = part.split_at(split_pos);
            let code = code.trim_start_matches('\n').trim_end();
            result.push_str(&format!("<pre><code>{}</code></pre>", code));
        } else {
            let inline_parts: Vec<&str> = part.split('`').collect();
            let has_unclosed_inline = inline_parts.len() % 2 == 0;
            let mut inline_html = String::new();
            for (inline_index, inline_part) in inline_parts.iter().enumerate() {
                let is_unclosed_inline_last =
                    has_unclosed_inline && inline_index == inline_parts.len() - 1;
                if inline_index % 2 == 1 && !is_unclosed_inline_last {
                    inline_html.push_str(&format!("<code>{}</code>", inline_part));
                } else {
                    inline_html.push_str(&format_plain_markdown(inline_part));
                }
            }
            result.push_str(&inline_html);
        }
    }

    result
}

fn format_plain_markdown(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut result = String::new();
    let mut in_list = false;
    let mut table_rows: Vec<&str> = Vec::new();
    let mut index = 0usize;

    while index < lines.len() {
        let trimmed = lines[index].trim();

        if is_table_row(trimmed) {
            if in_list {
                result.push_str("</ul>");
                in_list = false;
            }

            table_rows.clear();
            while index < lines.len() && is_table_row(lines[index].trim()) {
                table_rows.push(lines[index].trim());
                index += 1;
            }
            result.push_str(&render_markdown_table(&table_rows));
            continue;
        }

        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            if !in_list {
                result.push_str("<ul style=\"margin:4px 0;padding-left:20px;\">");
                in_list = true;
            }
            let item_text = format_inline(&trimmed[2..]);
            result.push_str(&format!("<li style=\"margin:2px 0;\">{}</li>", item_text));
            index += 1;
            continue;
        }

        if is_ordered_list_item(trimmed) {
            if !in_list {
                result.push_str("<ul style=\"margin:4px 0;padding-left:20px;\">");
                in_list = true;
            }
            let dot_pos = trimmed.find('.').unwrap();
            let item_text = format_inline(trimmed[dot_pos + 1..].trim());
            result.push_str(&format!("<li style=\"margin:2px 0;\">{}</li>", item_text));
            index += 1;
            continue;
        }

        if in_list {
            result.push_str("</ul>");
            in_list = false;
        }

        if trimmed.starts_with("## ") {
            let heading = format_inline(&trimmed[3..]);
            result.push_str(&format!(
                "<div style=\"font-weight:600;font-size:15px;margin:12px 0 4px 0;\">{}</div>",
                heading
            ));
            index += 1;
            continue;
        }

        if trimmed.starts_with("# ") {
            let heading = format_inline(&trimmed[2..]);
            result.push_str(&format!(
                "<div style=\"font-weight:700;font-size:16px;margin:12px 0 4px 0;\">{}</div>",
                heading
            ));
            index += 1;
            continue;
        }

        if index > 0 {
            result.push_str("<br/>");
        }
        result.push_str(&format_inline(lines[index]));
        index += 1;
    }

    if in_list {
        result.push_str("</ul>");
    }

    result
}

fn is_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.matches('|').count() >= 2
}

fn is_table_separator(line: &str) -> bool {
    let trimmed = line.trim().trim_start_matches('|').trim_end_matches('|');
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|ch| ch == '-' || ch == ':' || ch == '|' || ch == ' ')
}

fn parse_table_cells(row: &str) -> Vec<String> {
    let trimmed = row.trim();
    let inner = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    inner.split('|').map(|cell| cell.trim().to_string()).collect()
}

fn render_markdown_table(rows: &[&str]) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let table_style = "width:100%;border-collapse:collapse;margin:8px 0;font-size:13px;";
    let th_style = "border:1px solid #E5E7EB;padding:6px 10px;background-color:#F8FAFC;font-weight:600;text-align:left;color:#334155;";
    let td_style = "border:1px solid #E5E7EB;padding:6px 10px;text-align:left;color:#475569;";
    let mut html = format!(
        "<table style=\"{}\" cellpadding=\"0\" cellspacing=\"0\">",
        table_style
    );

    let has_header = rows.len() >= 2 && is_table_separator(rows[1]);
    let data_start = if has_header { 2 } else { 0 };

    if has_header {
        let header_cells = parse_table_cells(rows[0]);
        html.push_str("<thead><tr>");
        for cell in &header_cells {
            html.push_str(&format!("<th style=\"{}\">{}</th>", th_style, format_inline(cell)));
        }
        html.push_str("</tr></thead>");
    }

    html.push_str("<tbody>");
    for row in rows.iter().skip(data_start) {
        if is_table_separator(row) {
            continue;
        }
        let cells = parse_table_cells(row);
        html.push_str("<tr>");
        for cell in &cells {
            html.push_str(&format!("<td style=\"{}\">{}</td>", td_style, format_inline(cell)));
        }
        html.push_str("</tr>");
    }
    html.push_str("</tbody></table>");

    html
}

fn format_inline(text: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0usize;

    while index < chars.len() {
        if index + 1 < chars.len() && chars[index] == '*' && chars[index + 1] == '*' {
            if let Some(end) = find_closing_marker(&chars, index + 2, &['*', '*']) {
                let inner: String = chars[index + 2..end].iter().collect();
                result.push_str(&format!("<strong>{}</strong>", inner));
                index = end + 2;
                continue;
            }
        }

        if chars[index] == '*' && (index + 1 >= chars.len() || chars[index + 1] != '*') {
            if let Some(end) = find_closing_single(&chars, index + 1, '*') {
                let inner: String = chars[index + 1..end].iter().collect();
                result.push_str(&format!("<em>{}</em>", inner));
                index = end + 1;
                continue;
            }
        }

        result.push(chars[index]);
        index += 1;
    }

    result
}

fn find_closing_marker(chars: &[char], start: usize, marker: &[char; 2]) -> Option<usize> {
    let mut index = start;
    while index + 1 < chars.len() {
        if chars[index] == marker[0] && chars[index + 1] == marker[1] {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn find_closing_single(chars: &[char], start: usize, marker: char) -> Option<usize> {
    (start..chars.len()).find(|&index| chars[index] == marker)
}

fn is_ordered_list_item(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }

    index > 0
        && index < bytes.len()
        && bytes[index] == b'.'
        && index + 1 < bytes.len()
        && bytes[index + 1] == b' '
}