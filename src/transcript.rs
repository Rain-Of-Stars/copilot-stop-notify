// Transcript 模块：读取、解析、验证会话记录
use crate::redact::{redact_sensitive_text, summarize_path_for_display, summarize_path_str_for_display};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// transcript 文件大小上限（50 MB）
const MAX_TRANSCRIPT_SIZE: u64 = 50 * 1024 * 1024;
/// 稳定窗口：文件在此时间内无修改视为稳定
const STABILITY_INTERVAL: Duration = Duration::from_secs(2);
/// 稳定检查最大尝试次数
const STABILITY_MAX_RETRIES: u32 = 5;
/// 补等 transcript 收束的最大重试次数
const COMPLETION_MAX_RETRIES: u32 = 6;
/// transcript 收束重试间隔
const COMPLETION_RETRY_INTERVAL: Duration = Duration::from_secs(1);

/// 对话轮次
#[derive(Debug, Clone)]
pub struct Turn {
    pub role: String,
    pub content: String,
}

/// transcript 快照：除轮次外，额外保留是否已完整收束和内容指纹
#[derive(Debug, Clone)]
pub struct TranscriptSnapshot {
    pub turns: Vec<Turn>,
    pub fingerprint: String,
    is_vscode_jsonl: bool,
    has_open_assistant_turn: bool,
    has_open_tool_execution: bool,
    last_closed_assistant_turn: Option<VscodeAssistantTurnState>,
    last_vscode_event_type: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct VscodeAssistantTurnState {
    last_message_had_tool_requests: bool,
    last_message_had_visible_content: bool,
}

impl TranscriptSnapshot {
    /// Stop 事件只在最后一条为完整且真正收尾的助手回复时才允许发信
    pub fn is_ready_for_email(&self) -> bool {
        let Some(last_turn) = self.turns.last() else {
            return false;
        };

        if last_turn.role != "assistant" {
            return false;
        }

        if !self.is_vscode_jsonl {
            return true;
        }

        if self.has_open_assistant_turn {
            return false;
        }

        if self.has_open_tool_execution {
            return false;
        }

        if !matches!(
            self.last_vscode_event_type.as_deref(),
            Some("assistant.turn_end") | Some("session.end")
        ) {
            return false;
        }

        matches!(
            self.last_closed_assistant_turn.as_ref(),
            Some(state)
                if state.last_message_had_visible_content
                    && !state.last_message_had_tool_requests
        )
    }
}

// ========== 反序列化辅助结构 ==========

/// 格式 A: 简单的 {role, content} 数组
#[derive(Deserialize)]
struct SimpleTurn {
    role: Option<String>,
    content: Option<serde_json::Value>,
    #[serde(rename = "type")]
    turn_type: Option<String>,
    message: Option<Box<SimpleTurn>>,
    // 其他可能的字段名
    text: Option<String>,
}

/// 格式 B: 对象包装
#[derive(Deserialize)]
struct TranscriptWrapper {
    turns: Option<Vec<serde_json::Value>>,
    messages: Option<Vec<serde_json::Value>>,
    conversation: Option<Vec<serde_json::Value>>,
    entries: Option<Vec<serde_json::Value>>,
}

// ========== VS Code Copilot Transcript JSONL 事件结构 ==========

/// VS Code Copilot 会话 JSONL 事件
#[derive(Deserialize)]
struct VscodeEvent {
    #[serde(rename = "type")]
    event_type: String,
    data: Option<serde_json::Value>,
}

/// 工具请求条目
#[derive(Deserialize)]
struct ToolRequest {
    name: Option<String>,
    #[serde(rename = "toolCallId")]
    _tool_call_id: Option<String>,
    arguments: Option<serde_json::Value>,
}

#[allow(dead_code)]
/// 检测对话是否为子智能体迭代（最后一条助手消息以 </final_answer> 结尾）
/// 仅保留给测试和离线诊断使用，主流程不再依赖该启发式判定
pub fn is_subagent_iteration(turns: &[Turn]) -> bool {
    // 从后向前找到最后一条助手消息
    for turn in turns.iter().rev() {
        if turn.role == "assistant" {
            let content = turn.content.trim();
            return content.contains("<final_answer>") && content.ends_with("</final_answer>");
        }
    }
    false
}

/// 验证 transcript 路径是否在允许的根目录内
pub fn validate_path(transcript_path: &str, allowed_roots: &[PathBuf]) -> Result<PathBuf, String> {
    let path = PathBuf::from(transcript_path);

    // 规范化路径（解析 .. 和符号链接）
    let canonical = path
        .canonicalize()
        .map_err(|e| format!(
            "transcript 路径无法解析: {} ({})",
            summarize_path_str_for_display(transcript_path),
            redact_sensitive_text(&e.to_string())
        ))?;

    // 检查路径是否在允许的根目录内
    let allowed = allowed_roots.iter().any(|root| {
        if let Ok(canonical_root) = root.canonicalize() {
            canonical.starts_with(&canonical_root)
        } else {
            false
        }
    });

    if !allowed {
        return Err(format!(
            "transcript 路径不在允许的目录内: {}",
            summarize_path_for_display(&canonical)
        ));
    }

    Ok(canonical)
}

/// 等待 transcript 文件稳定（不再被写入）
pub fn wait_for_stability(path: &Path) -> Result<(), String> {
    for attempt in 0..STABILITY_MAX_RETRIES {
        let metadata = std::fs::metadata(path)
            .map_err(|e| format!("获取 transcript 文件信息失败: {}", e))?;

        // 检查文件大小
        if metadata.len() > MAX_TRANSCRIPT_SIZE {
            return Err(format!(
                "transcript 文件过大: {} bytes (上限 {} bytes)",
                metadata.len(),
                MAX_TRANSCRIPT_SIZE
            ));
        }

        let mtime = metadata
            .modified()
            .map_err(|e| format!("获取修改时间失败: {}", e))?;

        let elapsed = SystemTime::now()
            .duration_since(mtime)
            .unwrap_or(Duration::ZERO);

        if elapsed >= STABILITY_INTERVAL {
            return Ok(());
        }

        // 等待后重试
        if attempt < STABILITY_MAX_RETRIES - 1 {
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    // 超过重试次数仍未稳定，仍然继续处理（避免因时钟偏移导致永远不触发）
    eprintln!("[警告] transcript 文件在稳定窗口内仍有变化，继续处理");
    Ok(())
}

/// 等待 transcript 既稳定又形成完整助手回复，并返回带指纹的快照
pub fn wait_for_complete_transcript(path: &Path) -> Result<TranscriptSnapshot, String> {
    wait_for_stability(path)?;

    let mut last_snapshot = parse_transcript_snapshot(path)?;
    if last_snapshot.is_ready_for_email() {
        return Ok(last_snapshot);
    }

    for _ in 0..COMPLETION_MAX_RETRIES {
        std::thread::sleep(COMPLETION_RETRY_INTERVAL);
        last_snapshot = parse_transcript_snapshot(path)?;
        if last_snapshot.is_ready_for_email() {
            return Ok(last_snapshot);
        }
    }

    Err("transcript 在等待后仍未形成最终助手回复，跳过发送".to_string())
}

#[allow(dead_code)]
/// 读取并解析 transcript 文件为对话轮次列表
/// 这是给库调用方保留的兼容入口，主流程改用快照接口
pub fn parse_transcript(path: &Path) -> Result<Vec<Turn>, String> {
    Ok(parse_transcript_snapshot(path)?.turns)
}

/// 读取并解析 transcript 文件为带状态的快照
pub fn parse_transcript_snapshot(path: &Path) -> Result<TranscriptSnapshot, String> {
    let data =
        std::fs::read_to_string(path).map_err(|e| format!("读取 transcript 失败: {}", e))?;

    let trimmed = data.trim();
    if trimmed.is_empty() {
        return Err("transcript 文件内容为空".to_string());
    }

    // 优先尝试 VS Code Copilot JSONL 格式（type + data 嵌套结构）
    if let Ok(snapshot) = try_parse_vscode_jsonl(trimmed) {
        if !snapshot.turns.is_empty() {
            return Ok(snapshot);
        }
    }

    // 尝试各种通用格式
    if let Ok(turns) = try_parse_array(trimmed) {
        return Ok(snapshot_from_turns(turns));
    }

    if let Ok(turns) = try_parse_wrapper(trimmed) {
        if !turns.is_empty() {
            return Ok(snapshot_from_turns(turns));
        }
    }

    if let Ok(turns) = try_parse_jsonl(trimmed) {
        if !turns.is_empty() {
            return Ok(snapshot_from_turns(turns));
        }
    }

    // 最后尝试将整个内容作为纯文本
    Ok(snapshot_from_turns(vec![Turn {
        role: "transcript".to_string(),
        content: truncate_string(trimmed, 500_000),
    }]))
}

// ========== VS Code Copilot Transcript JSONL 解析 ==========

/// 解析 VS Code Copilot 的 JSONL transcript 格式
/// 每行一个 JSON 事件：{"type":"user.message","data":{"content":"..."},"id":"...","timestamp":"...","parentId":"..."}
fn try_parse_vscode_jsonl(data: &str) -> Result<TranscriptSnapshot, String> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut saw_event = false;
    let mut is_vscode_format = false;
    let mut open_assistant_turns = 0usize;
    let mut open_tool_executions = 0usize;
    let mut current_assistant_turn: Option<VscodeAssistantTurnState> = None;
    let mut current_assistant_turn_index: Option<usize> = None;
    let mut last_closed_assistant_turn: Option<VscodeAssistantTurnState> = None;
    let mut last_vscode_event_type: Option<String> = None;

    for line in data.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<VscodeEvent>(trimmed) {
            saw_event = true;
            // 检测 VS Code 格式特征：type 字段包含 "." 分隔的事件名
            if event.event_type.contains('.') {
                is_vscode_format = true;
            }
            last_vscode_event_type = Some(event.event_type.clone());
            match event.event_type.as_str() {
                "assistant.turn_start" => {
                    open_assistant_turns = open_assistant_turns.saturating_add(1);
                    current_assistant_turn = Some(VscodeAssistantTurnState::default());
                    current_assistant_turn_index = None;
                }
                "assistant.turn_end" => {
                    open_assistant_turns = open_assistant_turns.saturating_sub(1);
                    if let Some(state) = current_assistant_turn.take() {
                        last_closed_assistant_turn = Some(state);
                    }
                    current_assistant_turn_index = None;
                }
                "tool.execution_start" => {
                    open_tool_executions = open_tool_executions.saturating_add(1);
                }
                "tool.execution_complete" => {
                    open_tool_executions = open_tool_executions.saturating_sub(1);
                }
                "user.message" => {
                    current_assistant_turn_index = None;
                    if let Some(turn) = extract_user_message(&event) {
                        turns.push(turn);
                    }
                }
                "assistant.message" => {
                    let turn_state = current_assistant_turn.get_or_insert_with(Default::default);
                    turn_state.last_message_had_tool_requests = assistant_message_has_tool_requests(&event);
                    turn_state.last_message_had_visible_content = assistant_message_has_visible_content(&event);

                    if let Some(turn) = extract_assistant_message(&event) {
                        // 仅在同一个 assistant turn 内合并多条 assistant.message。
                        if let Some(index) = current_assistant_turn_index {
                            if let Some(existing) = turns.get_mut(index) {
                                if !turn.content.is_empty() {
                                    if !existing.content.is_empty() {
                                        existing.content.push_str("\n\n");
                                    }
                                    existing.content.push_str(&turn.content);
                                }
                                continue;
                            }
                        }

                        if !turn.content.is_empty() {
                            turns.push(turn);
                            current_assistant_turn_index = Some(turns.len() - 1);
                        }
                    }
                }
                // 跳过噪声事件
                "session.start" | "session.end" => {
                    current_assistant_turn_index = None;
                    // 不生成对话轮次
                }
                _ => {
                    // 其他未知事件类型，忽略
                }
            }
        }
    }

    if !is_vscode_format || !saw_event {
        return Err("非 VS Code Copilot JSONL 格式".to_string());
    }

    // 清理：去除内容为空的轮次
    turns.retain(|t| !t.content.trim().is_empty());

    Ok(TranscriptSnapshot {
        fingerprint: conversation_fingerprint(&turns),
        turns,
        is_vscode_jsonl: true,
        has_open_assistant_turn: open_assistant_turns > 0 || current_assistant_turn.is_some(),
        has_open_tool_execution: open_tool_executions > 0,
        last_closed_assistant_turn,
        last_vscode_event_type,
    })
}

fn snapshot_from_turns(turns: Vec<Turn>) -> TranscriptSnapshot {
    TranscriptSnapshot {
        fingerprint: conversation_fingerprint(&turns),
        turns,
        is_vscode_jsonl: false,
        has_open_assistant_turn: false,
        has_open_tool_execution: false,
        last_closed_assistant_turn: None,
        last_vscode_event_type: None,
    }
}

fn conversation_fingerprint(turns: &[Turn]) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    fn hash_bytes(mut hash: u64, bytes: &[u8], prime: u64) -> u64 {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(prime);
        }
        hash
    }

    let mut hash = FNV_OFFSET;
    for turn in turns {
        hash = hash_bytes(hash, turn.role.as_bytes(), FNV_PRIME);
        hash = hash_bytes(hash, &[0xff], FNV_PRIME);
        hash = hash_bytes(hash, turn.content.as_bytes(), FNV_PRIME);
        hash = hash_bytes(hash, &[0x00], FNV_PRIME);
    }

    format!("{:016x}", hash)
}

/// 从 user.message 事件提取用户消息
fn extract_user_message(event: &VscodeEvent) -> Option<Turn> {
    let data = event.data.as_ref()?;
    let content = extract_content_from_value(data.get("content")?);
    if content.trim().is_empty() {
        return None;
    }
    Some(Turn {
        role: "user".to_string(),
        content,
    })
}

fn assistant_message_has_tool_requests(event: &VscodeEvent) -> bool {
    event
        .data
        .as_ref()
        .and_then(|data| data.get("toolRequests"))
        .and_then(|value| value.as_array())
        .map(|items| !items.is_empty())
        .unwrap_or(false)
}

fn assistant_message_has_visible_content(event: &VscodeEvent) -> bool {
    event
        .data
        .as_ref()
        .and_then(|data| data.get("content"))
        .map(extract_content_from_value)
        .map(|content| !content.trim().is_empty())
        .unwrap_or(false)
}

/// 从 assistant.message 事件提取助手消息
/// 合并 content 和 toolRequests
fn extract_assistant_message(event: &VscodeEvent) -> Option<Turn> {
    let data = event.data.as_ref()?;
    let mut parts: Vec<String> = Vec::new();

    // 提取主要回复内容
    if let Some(content_val) = data.get("content") {
        let text = extract_content_from_value(content_val);
        let trimmed = text.trim().to_string();
        if !trimmed.is_empty() {
            parts.push(trimmed);
        }
    }

    // 提取工具调用摘要
    if let Some(tool_requests) = data.get("toolRequests").and_then(|v| v.as_array()) {
        let tool_summaries: Vec<String> = tool_requests
            .iter()
            .filter_map(|tr| {
                if let Ok(req) = serde_json::from_value::<ToolRequest>(tr.clone()) {
                    let name = req.name.unwrap_or_else(|| "未知工具".to_string());
                    // 提取工具参数中的关键信息
                    let args_summary = extract_tool_args_summary(&req.arguments);
                    if args_summary.is_empty() {
                        Some(format!("🔧 调用 `{}`", name))
                    } else {
                        Some(format!("🔧 调用 `{}` — {}", name, args_summary))
                    }
                } else {
                    None
                }
            })
            .collect();
        if !tool_summaries.is_empty() {
            parts.push(tool_summaries.join("\n"));
        }
    }

    if parts.is_empty() {
        return None;
    }

    Some(Turn {
        role: "assistant".to_string(),
        content: parts.join("\n\n"),
    })
}

/// 从工具调用参数中提取简短摘要
fn extract_tool_args_summary(args: &Option<serde_json::Value>) -> String {
    let args = match args {
        Some(v) => v,
        None => return String::new(),
    };

    // 参数可能是 JSON 字符串或 JSON 对象
    let obj = if let Some(s) = args.as_str() {
        serde_json::from_str::<serde_json::Value>(s).ok()
    } else {
        Some(args.clone())
    };

    if let Some(obj) = obj {
        // 绝对路径仅保留尾部层级，避免在通知中泄露本机用户名与目录结构
        for key in &["filePath", "path"] {
            if let Some(val) = obj.get(key).and_then(|v| v.as_str()) {
                let display = truncate_string(&summarize_path_str_for_display(val), 120);
                return format!("`{}`", display);
            }
        }
        for key in &["command", "query", "pattern", "url"] {
            if let Some(val) = obj.get(key).and_then(|v| v.as_str()) {
                let display = truncate_string(val, 120);
                return format!("`{}`", display);
            }
        }
        // 对 urls 数组
        if let Some(urls) = obj.get("urls").and_then(|v| v.as_array()) {
            let url_strs: Vec<&str> = urls.iter().filter_map(|u| u.as_str()).take(2).collect();
            if !url_strs.is_empty() {
                return url_strs.join(", ");
            }
        }
    }
    String::new()
}

/// 从 serde_json::Value 提取文本内容（支持 string / array / object）
fn extract_content_from_value(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => {
            let parts: Vec<String> = arr
                .iter()
                .filter_map(|item| {
                    if let Some(s) = item.as_str() {
                        Some(s.to_string())
                    } else {
                        item.get("text").and_then(|t| t.as_str()).map(|t| t.to_string())
                    }
                })
                .collect();
            parts.join("\n")
        }
        serde_json::Value::Object(_) => {
            if let Some(t) = val.get("text").and_then(|t| t.as_str()) {
                t.to_string()
            } else {
                serde_json::to_string_pretty(val).unwrap_or_default()
            }
        }
        _ => String::new(),
    }
}

/// 格式 A: 顶层 JSON 数组
fn try_parse_array(data: &str) -> Result<Vec<Turn>, String> {
    let arr: Vec<serde_json::Value> =
        serde_json::from_str(data).map_err(|e| format!("非 JSON 数组: {}", e))?;
    Ok(values_to_turns(&arr))
}

/// 格式 B: 顶层 JSON 对象，内含 turns/messages/conversation/entries 字段
fn try_parse_wrapper(data: &str) -> Result<Vec<Turn>, String> {
    let wrapper: TranscriptWrapper =
        serde_json::from_str(data).map_err(|e| format!("非 JSON 对象: {}", e))?;

    let arr = wrapper
        .turns
        .or(wrapper.messages)
        .or(wrapper.conversation)
        .or(wrapper.entries)
        .ok_or_else(|| "对象中无已知的消息数组字段".to_string())?;

    Ok(values_to_turns(&arr))
}

/// 格式 C: JSONL（每行一个 JSON 对象）
fn try_parse_jsonl(data: &str) -> Result<Vec<Turn>, String> {
    let mut turns = Vec::new();
    let mut parse_ok = false;
    for line in data.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            parse_ok = true;
            if let Some(turn) = value_to_turn(&val) {
                turns.push(turn);
            }
        }
    }
    if parse_ok {
        Ok(turns)
    } else {
        Err("非 JSONL 格式".to_string())
    }
}

/// 将 JSON 值数组转为 Turn 列表
fn values_to_turns(values: &[serde_json::Value]) -> Vec<Turn> {
    values.iter().filter_map(value_to_turn).collect()
}

/// 将单个 JSON 值转为 Turn（支持多种结构）
fn value_to_turn(val: &serde_json::Value) -> Option<Turn> {
    if let Ok(st) = serde_json::from_value::<SimpleTurn>(val.clone()) {
        // 如果有 message 嵌套字段，递归提取
        if let Some(inner) = st.message {
            let role = inner
                .role
                .or(st.turn_type.clone())
                .unwrap_or_else(|| "unknown".to_string());
            let content = extract_content(inner.content.as_ref(), inner.text.as_deref());
            if !content.is_empty() {
                return Some(Turn { role, content });
            }
        }

        let role = st
            .role
            .or(st.turn_type)
            .unwrap_or_else(|| "unknown".to_string());
        let content = extract_content(st.content.as_ref(), st.text.as_deref());
        if !content.is_empty() {
            return Some(Turn { role, content });
        }
    }
    None
}

/// 从 content 字段提取文本（支持 string / array / object）
fn extract_content(content: Option<&serde_json::Value>, text_fallback: Option<&str>) -> String {
    if let Some(val) = content {
        match val {
            serde_json::Value::String(s) => return s.clone(),
            serde_json::Value::Array(arr) => {
                // 数组中每个元素可能是 {type: "text", text: "..."} 或纯字符串
                let parts: Vec<String> = arr
                    .iter()
                    .filter_map(|item| {
                        if let Some(s) = item.as_str() {
                            Some(s.to_string())
                        } else {
                            item.get("text").and_then(|t| t.as_str()).map(|t| t.to_string())
                        }
                    })
                    .collect();
                if !parts.is_empty() {
                    return parts.join("\n");
                }
            }
            serde_json::Value::Object(_) => {
                // 尝试提取 text 字段
                if let Some(t) = val.get("text").and_then(|t| t.as_str()) {
                    return t.to_string();
                }
                // 序列化为 JSON 字符串
                return serde_json::to_string_pretty(val).unwrap_or_default();
            }
            _ => {}
        }
    }
    text_fallback.unwrap_or("").to_string()
}

/// 截断过长字符串
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}... (内容已截断)", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::mpsc;

    fn temp_file(content: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.json");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        (dir, path)
    }

    #[test]
    fn test_parse_simple_array() {
        let json = r#"[
            {"role": "user", "content": "你好"},
            {"role": "assistant", "content": "你好！有什么可以帮你的？"}
        ]"#;
        let (_d, path) = temp_file(json);
        let turns = parse_transcript(&path).unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, "user");
        assert_eq!(turns[0].content, "你好");
        assert_eq!(turns[1].role, "assistant");
    }

    #[test]
    fn test_parse_wrapped_messages() {
        let json = r#"{"messages": [
            {"role": "system", "content": "你是助手"},
            {"role": "user", "content": "帮我写代码"},
            {"role": "assistant", "content": "好的，请问需要什么语言？"}
        ]}"#;
        let (_d, path) = temp_file(json);
        let turns = parse_transcript(&path).unwrap();
        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0].role, "system");
    }

    #[test]
    fn test_parse_nested_message() {
        let json = r#"[
            {"type": "send", "message": {"role": "user", "content": "测试嵌套"}},
            {"type": "receive", "message": {"role": "assistant", "content": "收到"}}
        ]"#;
        let (_d, path) = temp_file(json);
        let turns = parse_transcript(&path).unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, "user");
        assert_eq!(turns[0].content, "测试嵌套");
    }

    #[test]
    fn test_parse_jsonl() {
        let jsonl = r#"{"role": "user", "content": "第一句"}
{"role": "assistant", "content": "回复"}
{"role": "user", "content": "第二句"}"#;
        let (_d, path) = temp_file(jsonl);
        let turns = parse_transcript(&path).unwrap();
        assert_eq!(turns.len(), 3);
    }

    #[test]
    fn test_parse_array_content() {
        let json = r#"[
            {"role": "assistant", "content": [
                {"type": "text", "text": "第一段"},
                {"type": "text", "text": "第二段"}
            ]}
        ]"#;
        let (_d, path) = temp_file(json);
        let turns = parse_transcript(&path).unwrap();
        assert_eq!(turns.len(), 1);
        assert!(turns[0].content.contains("第一段"));
        assert!(turns[0].content.contains("第二段"));
    }

    #[test]
    fn test_validate_path_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.json");
        std::fs::write(&file_path, "[]").unwrap();

        let allowed = vec![dir.path().to_path_buf()];
        let result = validate_path(file_path.to_str().unwrap(), &allowed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_path_denied() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.json");
        std::fs::write(&file_path, "[]").unwrap();

        // 使用一个不同的目录作为允许列表
        let other_dir = tempfile::tempdir().unwrap();
        let allowed = vec![other_dir.path().to_path_buf()];
        let result = validate_path(file_path.to_str().unwrap(), &allowed);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_transcript() {
        let (_d, path) = temp_file("");
        let result = parse_transcript(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_truncate_string() {
        assert_eq!(truncate_string("short", 100), "short");
        let long = "a".repeat(200);
        let truncated = truncate_string(&long, 50);
        assert!(truncated.len() < 200);
        assert!(truncated.contains("截断"));
    }

    #[test]
    fn test_extract_string_content() {
        let val = serde_json::Value::String("hello".to_string());
        assert_eq!(extract_content(Some(&val), None), "hello");
    }

    #[test]
    fn test_extract_object_content() {
        let val = serde_json::json!({"text": "from object"});
        assert_eq!(extract_content(Some(&val), None), "from object");
    }

    #[test]
    fn test_text_fallback() {
        assert_eq!(extract_content(None, Some("fallback")), "fallback");
        assert_eq!(extract_content(None, None), "");
    }

    #[test]
    fn test_parse_vscode_copilot_jsonl() {
        // 模拟 VS Code Copilot 实际输出的 transcript JSONL 格式
        let jsonl = r#"{"type":"session.start","data":{"sessionId":"test-001","version":1},"id":"1","timestamp":"2026-04-03T02:13:06.168Z","parentId":null}
{"type":"user.message","data":{"content":"帮我写一个排序函数","attachments":[]},"id":"2","timestamp":"2026-04-03T02:13:06.168Z","parentId":"1"}
{"type":"assistant.turn_start","data":{"turnId":"0"},"id":"3","timestamp":"2026-04-03T02:13:06.168Z","parentId":"2"}
{"type":"assistant.message","data":{"messageId":"msg-1","content":"好的，我来写一个快速排序：\n\n```python\ndef quicksort(arr):\n    if len(arr) <= 1:\n        return arr\n    pivot = arr[0]\n    left = [x for x in arr[1:] if x <= pivot]\n    right = [x for x in arr[1:] if x > pivot]\n    return quicksort(left) + [pivot] + quicksort(right)\n```","toolRequests":[],"reasoningText":"用户需要排序函数"},"id":"4","timestamp":"2026-04-03T02:13:14.057Z","parentId":"3"}
{"type":"assistant.turn_end","data":{"turnId":"0"},"id":"5","timestamp":"2026-04-03T02:13:14.057Z","parentId":"4"}
{"type":"user.message","data":{"content":"再加一个归并排序","attachments":[]},"id":"6","timestamp":"2026-04-03T02:13:20.000Z","parentId":"5"}
{"type":"assistant.turn_start","data":{"turnId":"1"},"id":"7","timestamp":"2026-04-03T02:13:20.000Z","parentId":"6"}
{"type":"assistant.message","data":{"messageId":"msg-2","content":"","toolRequests":[{"toolCallId":"tc-1","name":"read_file","arguments":"{\"filePath\": \"sort.py\", \"startLine\": 1, \"endLine\": 50}","type":"function"}],"reasoningText":"先看看现有代码"},"id":"8","timestamp":"2026-04-03T02:13:22.000Z","parentId":"7"}
{"type":"tool.execution_start","data":{"toolCallId":"tc-1","toolName":"read_file"},"id":"9","timestamp":"2026-04-03T02:13:22.100Z","parentId":"8"}
{"type":"tool.execution_complete","data":{"toolCallId":"tc-1","success":true},"id":"10","timestamp":"2026-04-03T02:13:22.200Z","parentId":"9"}
{"type":"assistant.message","data":{"messageId":"msg-3","content":"这是归并排序的实现：\n\n```python\ndef mergesort(arr):\n    if len(arr) <= 1:\n        return arr\n    mid = len(arr) // 2\n    return merge(mergesort(arr[:mid]), mergesort(arr[mid:]))\n```","toolRequests":[]},"id":"11","timestamp":"2026-04-03T02:13:25.000Z","parentId":"10"}
{"type":"assistant.turn_end","data":{"turnId":"1"},"id":"12","timestamp":"2026-04-03T02:13:25.000Z","parentId":"11"}"#;

        let (_d, path) = temp_file(jsonl);
        let turns = parse_transcript(&path).unwrap();

        // 应该有 4 条有效轮次：2 条用户 + 2 条助手
        // 注意：第二轮助手包含合并的两条 assistant.message（工具调用 + 回复）
        assert!(turns.len() >= 3, "应至少有 3 条轮次，实际: {}", turns.len());

        // 第一条是用户消息
        assert_eq!(turns[0].role, "user");
        assert!(turns[0].content.contains("排序函数"));

        // 第二条是助手回复，包含快速排序代码
        assert_eq!(turns[1].role, "assistant");
        assert!(turns[1].content.contains("quicksort"));

        // 第三条是第二轮用户消息
        assert_eq!(turns[2].role, "user");
        assert!(turns[2].content.contains("归并排序"));

        // 第四条是助手回复，应合并工具调用和内容
        if turns.len() >= 4 {
            assert_eq!(turns[3].role, "assistant");
            assert!(turns[3].content.contains("mergesort") || turns[3].content.contains("read_file"));
        }

        // 不应有 session.start / turn_start / turn_end / tool.* 生成的轮次
        for turn in &turns {
            assert!(
                turn.role == "user" || turn.role == "assistant",
                "不应有非 user/assistant 角色: {}",
                turn.role
            );
        }
    }

    #[test]
    fn test_parse_vscode_tool_only_message() {
        // 测试只有工具调用没有文本内容的助手消息
        let jsonl = r#"{"type":"user.message","data":{"content":"检查文件"},"id":"1","timestamp":"2026-04-03T10:00:00Z","parentId":null}
{"type":"assistant.turn_start","data":{"turnId":"0"},"id":"2","timestamp":"2026-04-03T10:00:00Z","parentId":"1"}
{"type":"assistant.message","data":{"messageId":"m1","content":"","toolRequests":[{"toolCallId":"tc1","name":"list_dir","arguments":"{\"path\": \"/workspace\"}","type":"function"},{"toolCallId":"tc2","name":"read_file","arguments":"{\"filePath\": \"/workspace/main.rs\", \"startLine\": 1, \"endLine\": 50}","type":"function"}]},"id":"3","timestamp":"2026-04-03T10:00:02Z","parentId":"2"}
{"type":"assistant.turn_end","data":{"turnId":"0"},"id":"4","timestamp":"2026-04-03T10:00:02Z","parentId":"3"}"#;

        let (_d, path) = temp_file(jsonl);
        let turns = parse_transcript(&path).unwrap();

        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, "user");
        assert_eq!(turns[1].role, "assistant");
        // 工具调用摘要应包含工具名
        assert!(turns[1].content.contains("list_dir"), "工具调用应显示: {}", turns[1].content);
        assert!(turns[1].content.contains("read_file"), "工具调用应显示: {}", turns[1].content);
    }

    #[test]
    fn test_non_vscode_jsonl_still_works() {
        // 确保普通 JSONL 格式仍然正常解析
        let jsonl = r#"{"role": "user", "content": "你好"}
{"role": "assistant", "content": "你好！"}"#;
        let (_d, path) = temp_file(jsonl);
        let turns = parse_transcript(&path).unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, "user");
        assert_eq!(turns[1].role, "assistant");
    }

    #[test]
    fn test_vscode_snapshot_requires_closed_assistant_turn() {
        let jsonl = r#"{"type":"user.message","data":{"content":"检查状态"},"id":"1","timestamp":"2026-04-03T10:00:00Z","parentId":null}
{"type":"assistant.turn_start","data":{"turnId":"0"},"id":"2","timestamp":"2026-04-03T10:00:00Z","parentId":"1"}
{"type":"assistant.message","data":{"messageId":"m1","content":"处理中...","toolRequests":[]},"id":"3","timestamp":"2026-04-03T10:00:02Z","parentId":"2"}"#;

        let (_d, path) = temp_file(jsonl);
        let snapshot = parse_transcript_snapshot(&path).unwrap();

        assert_eq!(snapshot.turns.len(), 2);
        assert_eq!(snapshot.turns.last().map(|turn| turn.role.as_str()), Some("assistant"));
        assert!(!snapshot.is_ready_for_email());
    }

    #[test]
    fn test_vscode_snapshot_ready_after_turn_end() {
        let jsonl = r#"{"type":"user.message","data":{"content":"检查状态"},"id":"1","timestamp":"2026-04-03T10:00:00Z","parentId":null}
{"type":"assistant.turn_start","data":{"turnId":"0"},"id":"2","timestamp":"2026-04-03T10:00:00Z","parentId":"1"}
{"type":"assistant.message","data":{"messageId":"m1","content":"已经完成","toolRequests":[]},"id":"3","timestamp":"2026-04-03T10:00:02Z","parentId":"2"}
{"type":"assistant.turn_end","data":{"turnId":"0"},"id":"4","timestamp":"2026-04-03T10:00:02Z","parentId":"3"}"#;

        let (_d, path) = temp_file(jsonl);
        let snapshot = parse_transcript_snapshot(&path).unwrap();

        assert_eq!(snapshot.turns.len(), 2);
        assert!(snapshot.is_ready_for_email());
    }

    #[test]
    fn test_vscode_snapshot_not_ready_when_turn_ends_with_tool_requests() {
        let jsonl = r#"{"type":"user.message","data":{"content":"继续排查"},"id":"1","timestamp":"2026-04-03T10:00:00Z","parentId":null}
{"type":"assistant.turn_start","data":{"turnId":"0"},"id":"2","timestamp":"2026-04-03T10:00:00Z","parentId":"1"}
{"type":"assistant.message","data":{"messageId":"m1","content":"我先继续读代码并检查测试。","toolRequests":[{"toolCallId":"tc1","name":"read_file","arguments":"{\"filePath\": \"d:\\\\Github_project\\\\copilot-stop-notify\\\\src\\\\main.rs\", \"startLine\": 1, \"endLine\": 200}","type":"function"}]},"id":"3","timestamp":"2026-04-03T10:00:02Z","parentId":"2"}
{"type":"assistant.turn_end","data":{"turnId":"0"},"id":"4","timestamp":"2026-04-03T10:00:02Z","parentId":"3"}"#;

        let (_d, path) = temp_file(jsonl);
        let snapshot = parse_transcript_snapshot(&path).unwrap();

        assert_eq!(snapshot.turns.len(), 2);
        assert_eq!(snapshot.turns.last().map(|turn| turn.role.as_str()), Some("assistant"));
        assert!(!snapshot.is_ready_for_email());
    }

    #[test]
    fn test_vscode_snapshot_ready_after_tool_requests_followed_by_final_message() {
        let jsonl = r#"{"type":"user.message","data":{"content":"继续排查"},"id":"1","timestamp":"2026-04-03T10:00:00Z","parentId":null}
{"type":"assistant.turn_start","data":{"turnId":"0"},"id":"2","timestamp":"2026-04-03T10:00:00Z","parentId":"1"}
{"type":"assistant.message","data":{"messageId":"m1","content":"我先看下实现。","toolRequests":[{"toolCallId":"tc1","name":"read_file","arguments":"{\"filePath\": \"d:\\\\Github_project\\\\copilot-stop-notify\\\\src\\\\main.rs\", \"startLine\": 1, \"endLine\": 200}","type":"function"}]},"id":"3","timestamp":"2026-04-03T10:00:02Z","parentId":"2"}
{"type":"tool.execution_start","data":{"toolCallId":"tc1","toolName":"read_file"},"id":"4","timestamp":"2026-04-03T10:00:03Z","parentId":"3"}
{"type":"tool.execution_complete","data":{"toolCallId":"tc1","success":true},"id":"5","timestamp":"2026-04-03T10:00:04Z","parentId":"4"}
{"type":"assistant.message","data":{"messageId":"m2","content":"已经完成最终检查，可以给出结论了。","toolRequests":[]},"id":"6","timestamp":"2026-04-03T10:00:05Z","parentId":"5"}
{"type":"assistant.turn_end","data":{"turnId":"0"},"id":"7","timestamp":"2026-04-03T10:00:05Z","parentId":"6"}"#;

        let (_d, path) = temp_file(jsonl);
        let snapshot = parse_transcript_snapshot(&path).unwrap();

        assert_eq!(snapshot.turns.len(), 2);
        assert!(snapshot.is_ready_for_email());
    }

    #[test]
    fn test_vscode_snapshot_not_ready_when_last_event_is_tool_completion() {
        let jsonl = r#"{"type":"user.message","data":{"content":"继续排查"},"id":"1","timestamp":"2026-04-03T10:00:00Z","parentId":null}
{"type":"assistant.turn_start","data":{"turnId":"0"},"id":"2","timestamp":"2026-04-03T10:00:00Z","parentId":"1"}
{"type":"assistant.message","data":{"messageId":"m1","content":"我先做初步检索。","toolRequests":[{"toolCallId":"tc1","name":"search_subagent","arguments":"{\"query\": \"release workflow\"}","type":"function"}]},"id":"3","timestamp":"2026-04-03T10:00:02Z","parentId":"2"}
{"type":"assistant.turn_end","data":{"turnId":"0"},"id":"4","timestamp":"2026-04-03T10:00:02Z","parentId":"3"}
{"type":"tool.execution_start","data":{"toolCallId":"tc1","toolName":"search_subagent"},"id":"5","timestamp":"2026-04-03T10:00:03Z","parentId":"4"}
{"type":"assistant.turn_start","data":{"turnId":"1"},"id":"6","timestamp":"2026-04-03T10:00:04Z","parentId":"5"}
{"type":"assistant.message","data":{"messageId":"m2","content":"先给你一版初步结论。","toolRequests":[]},"id":"7","timestamp":"2026-04-03T10:00:05Z","parentId":"6"}
{"type":"assistant.turn_end","data":{"turnId":"1"},"id":"8","timestamp":"2026-04-03T10:00:05Z","parentId":"7"}
{"type":"tool.execution_complete","data":{"toolCallId":"tc1","success":true},"id":"9","timestamp":"2026-04-03T10:00:06Z","parentId":"8"}"#;

        let (_d, path) = temp_file(jsonl);
        let snapshot = parse_transcript_snapshot(&path).unwrap();

        assert_eq!(snapshot.turns.len(), 3);
        assert!(!snapshot.is_ready_for_email());
    }

    #[test]
    fn test_vscode_snapshot_not_ready_when_implicit_assistant_turn_remains_open() {
        let jsonl = r#"{"type":"user.message","data":{"content":"继续排查"},"id":"1","timestamp":"2026-04-03T10:00:00Z","parentId":null}
{"type":"assistant.turn_start","data":{"turnId":"0"},"id":"2","timestamp":"2026-04-03T10:00:00Z","parentId":"1"}
{"type":"assistant.message","data":{"messageId":"m1","content":"第一阶段已经完成。","toolRequests":[]},"id":"3","timestamp":"2026-04-03T10:00:02Z","parentId":"2"}
{"type":"assistant.turn_end","data":{"turnId":"0"},"id":"4","timestamp":"2026-04-03T10:00:02Z","parentId":"3"}
{"type":"assistant.message","data":{"messageId":"m2","content":"第二阶段还在继续处理中。","toolRequests":[]},"id":"5","timestamp":"2026-04-03T10:00:03Z","parentId":"4"}
{"type":"session.end","data":{"sessionId":"test"},"id":"6","timestamp":"2026-04-03T10:00:04Z","parentId":"5"}"#;

        let (_d, path) = temp_file(jsonl);
        let snapshot = parse_transcript_snapshot(&path).unwrap();

        assert_eq!(snapshot.turns.len(), 3);
        assert_eq!(snapshot.turns.last().map(|turn| turn.content.as_str()), Some("第二阶段还在继续处理中。"));
        assert!(!snapshot.is_ready_for_email());
    }

    #[test]
    fn test_wait_for_complete_transcript_waits_for_delayed_final_message() {
        let initial = r#"{"type":"user.message","data":{"content":"继续排查"},"id":"1","timestamp":"2026-04-03T10:00:00Z","parentId":null}
{"type":"assistant.turn_start","data":{"turnId":"0"},"id":"2","timestamp":"2026-04-03T10:00:00Z","parentId":"1"}
{"type":"assistant.message","data":{"messageId":"m1","content":"我先读取代码。","toolRequests":[{"toolCallId":"tc1","name":"read_file","arguments":"{\"filePath\": \"src/main.rs\"}","type":"function"}]},"id":"3","timestamp":"2026-04-03T10:00:02Z","parentId":"2"}
{"type":"tool.execution_start","data":{"toolCallId":"tc1","toolName":"read_file"},"id":"4","timestamp":"2026-04-03T10:00:03Z","parentId":"3"}
{"type":"tool.execution_complete","data":{"toolCallId":"tc1","success":true},"id":"5","timestamp":"2026-04-03T10:00:04Z","parentId":"4"}"#;
        let final_chunk = r#"
{"type":"assistant.message","data":{"messageId":"m2","content":"已经形成最终结论。","toolRequests":[]},"id":"6","timestamp":"2026-04-03T10:00:05Z","parentId":"5"}
{"type":"assistant.turn_end","data":{"turnId":"0"},"id":"7","timestamp":"2026-04-03T10:00:05Z","parentId":"6"}"#;

        let (_d, path) = temp_file(initial);

        // 先让初始文件通过稳定检查，再在等待轮询期间补写最终消息。
        std::thread::sleep(STABILITY_INTERVAL);

        let (ready_tx, ready_rx) = mpsc::channel();
        let append_path = path.clone();
        let writer = std::thread::spawn(move || {
            ready_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(1200));
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&append_path)
                .unwrap();
            file.write_all(final_chunk.as_bytes()).unwrap();
            file.flush().unwrap();
        });

        ready_rx.recv().unwrap();
        let snapshot = wait_for_complete_transcript(&path).unwrap();
        writer.join().unwrap();

        assert!(snapshot.is_ready_for_email());
        assert_eq!(snapshot.turns.len(), 2);
        let assistant_content = snapshot.turns.last().map(|turn| turn.content.as_str()).unwrap();
        assert!(assistant_content.contains("我先读取代码。"));
        assert!(assistant_content.contains("已经形成最终结论。"));
    }

    #[test]
    fn test_vscode_does_not_merge_assistant_messages_across_turn_boundaries() {
        let jsonl = r#"{"type":"user.message","data":{"content":"继续排查"},"id":"1","timestamp":"2026-04-03T10:00:00Z","parentId":null}
{"type":"assistant.turn_start","data":{"turnId":"0"},"id":"2","timestamp":"2026-04-03T10:00:00Z","parentId":"1"}
{"type":"assistant.message","data":{"messageId":"m1","content":"第一轮结论。","toolRequests":[]},"id":"3","timestamp":"2026-04-03T10:00:02Z","parentId":"2"}
{"type":"assistant.turn_end","data":{"turnId":"0"},"id":"4","timestamp":"2026-04-03T10:00:02Z","parentId":"3"}
{"type":"assistant.turn_start","data":{"turnId":"1"},"id":"5","timestamp":"2026-04-03T10:00:03Z","parentId":"4"}
{"type":"assistant.message","data":{"messageId":"m2","content":"第二轮结论。","toolRequests":[]},"id":"6","timestamp":"2026-04-03T10:00:04Z","parentId":"5"}
{"type":"assistant.turn_end","data":{"turnId":"1"},"id":"7","timestamp":"2026-04-03T10:00:04Z","parentId":"6"}"#;

        let (_d, path) = temp_file(jsonl);
        let snapshot = parse_transcript_snapshot(&path).unwrap();

        assert_eq!(snapshot.turns.len(), 3);
        assert_eq!(snapshot.turns[1].content, "第一轮结论。");
        assert_eq!(snapshot.turns[2].content, "第二轮结论。");
    }
}
