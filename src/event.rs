// 事件模块：解析 Hook stdin 输入，过滤事件类型
use serde::Deserialize;

/// stdin 大小上限（10 MB）
const MAX_STDIN_SIZE: usize = 10 * 1024 * 1024;

/// Hook 输入结构体（VS Code Copilot 传入 stdin 的 JSON）
/// 注意：VS Code 实际发送 snake_case 字段名（hook_event_name, session_id），
/// 同时保留 camelCase 别名以兼容文档示例和未来可能的格式变更
#[derive(Debug, Deserialize)]
pub struct HookInput {
    /// 事件时间戳
    pub timestamp: Option<String>,
    /// 工作目录（不可信，不用于安全判断）
    pub cwd: Option<String>,
    /// 会话唯一标识
    #[serde(alias = "sessionId")]
    pub session_id: Option<String>,
    /// 事件名称（Stop / SubagentStop / ...）
    #[serde(alias = "hookEventName")]
    pub hook_event_name: String,
    /// transcript 文件路径
    #[serde(alias = "transcriptPath")]
    pub transcript_path: Option<String>,
    /// 是否因前一个 Stop hook 触发的继续运行（防止无限循环）
    #[serde(alias = "stopHookActive")]
    pub stop_hook_active: Option<bool>,
}

/// 从 stdin 读取并解析 Hook 输入
pub fn read_hook_input() -> Result<HookInput, String> {
    use std::io::Read;
    let mut buffer = Vec::new();
    let bytes_read = std::io::stdin()
        .take(MAX_STDIN_SIZE as u64)
        .read_to_end(&mut buffer)
        .map_err(|e| format!("读取 stdin 失败: {}", e))?;

    if bytes_read == 0 {
        return Err("stdin 为空，没有接收到 Hook 输入".to_string());
    }

    if bytes_read >= MAX_STDIN_SIZE {
        return Err(format!("stdin 超过大小限制 ({} bytes)", MAX_STDIN_SIZE));
    }

    serde_json::from_slice(&buffer)
        .map_err(|e| format!("解析 Hook 输入 JSON 失败: {}", e))
}

/// 判断是否应该处理该事件
/// 仅处理 Stop 事件，忽略 SubagentStop 和其他事件
/// 当 stop_hook_active=true 时跳过（防止无限循环）
pub fn should_process(input: &HookInput) -> Result<bool, String> {
    // 仅处理 Stop 事件
    if input.hook_event_name != "Stop" {
        return Ok(false);
    }

    // 如果已经是 stop hook 触发的继续运行，不再处理
    if input.stop_hook_active.unwrap_or(false) {
        return Ok(false);
    }

    // 必须有 transcript_path
    if input.transcript_path.is_none() {
        return Err("Stop 事件缺少 transcript_path".to_string());
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(event: &str, stop_active: Option<bool>, transcript: Option<&str>) -> HookInput {
        HookInput {
            timestamp: Some("2026-04-03T10:00:00.000Z".to_string()),
            cwd: Some("/workspace".to_string()),
            session_id: Some("test-session-123".to_string()),
            hook_event_name: event.to_string(),
            transcript_path: transcript.map(|s| s.to_string()),
            stop_hook_active: stop_active,
        }
    }

    #[test]
    fn test_should_process_stop_event() {
        let input = make_input("Stop", None, Some("/path/to/transcript.json"));
        assert!(should_process(&input).unwrap());
    }

    #[test]
    fn test_should_skip_subagent_stop() {
        let input = make_input("SubagentStop", None, Some("/path/to/transcript.json"));
        assert!(!should_process(&input).unwrap());
    }

    #[test]
    fn test_should_skip_other_events() {
        for event in &["SessionStart", "PreToolUse", "PostToolUse", "UserPromptSubmit"] {
            let input = make_input(event, None, Some("/path"));
            assert!(!should_process(&input).unwrap());
        }
    }

    #[test]
    fn test_should_skip_when_stop_hook_active() {
        let input = make_input("Stop", Some(true), Some("/path/to/transcript.json"));
        assert!(!should_process(&input).unwrap());
    }

    #[test]
    fn test_should_error_without_transcript_path() {
        let input = make_input("Stop", None, None);
        assert!(should_process(&input).is_err());
    }

    #[test]
    fn test_deserialize_hook_input() {
        let json = r#"{
            "timestamp": "2026-04-03T10:00:00.000Z",
            "cwd": "/workspace",
            "sessionId": "abc-123",
            "hookEventName": "Stop",
            "transcript_path": "/tmp/transcript.json",
            "stop_hook_active": false
        }"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.hook_event_name, "Stop");
        assert_eq!(input.session_id.as_deref(), Some("abc-123"));
        assert_eq!(
            input.transcript_path.as_deref(),
            Some("/tmp/transcript.json")
        );
        assert_eq!(input.stop_hook_active, Some(false));
    }

    #[test]
    fn test_deserialize_minimal_input() {
        // 只有必填字段
        let json = r#"{"hookEventName": "Stop"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.hook_event_name, "Stop");
        assert!(input.session_id.is_none());
        assert!(input.transcript_path.is_none());
    }

    #[test]
    fn test_deserialize_snake_case_input() {
        // VS Code 实际发送的 snake_case 格式
        let json = r#"{
            "timestamp": "2026-04-03T10:00:00.000Z",
            "session_id": "snake-test-001",
            "hook_event_name": "Stop",
            "transcript_path": "/tmp/transcript.json",
            "stop_hook_active": false
        }"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.hook_event_name, "Stop");
        assert_eq!(input.session_id.as_deref(), Some("snake-test-001"));
        assert_eq!(
            input.transcript_path.as_deref(),
            Some("/tmp/transcript.json")
        );
        assert_eq!(input.stop_hook_active, Some(false));
    }

    #[test]
    fn test_deserialize_minimal_snake_case() {
        // VS Code 最小 snake_case 输入
        let json = r#"{"hook_event_name": "Stop"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.hook_event_name, "Stop");
    }
}
