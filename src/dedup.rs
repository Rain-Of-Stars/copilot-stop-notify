// 幂等去重模块：防止同一会话重复发送邮件
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// 去重标记过期时间（24 小时）
const MARK_EXPIRY: Duration = Duration::from_secs(24 * 60 * 60);

/// 获取去重目录
fn dedup_dir() -> PathBuf {
    std::env::temp_dir().join("copilot-stop-notify-dedup")
}

/// 检查该 session_id 是否已经发送过邮件，且对话轮次没有变化
/// 如果用户在已发送邮件的会话中续聊产生了新轮次，则不算重复
pub fn is_duplicate(session_id: &str, current_turn_count: usize, current_fingerprint: &str) -> bool {
    let path = dedup_dir().join(sanitize_filename(session_id));
    if !path.exists() {
        return false;
    }

    if is_mark_expired(&path, SystemTime::now()) {
        let _ = std::fs::remove_file(&path);
        return false;
    }

    // 读取已存储的轮次数，如果当前轮次更多说明有新对话，不算重复
    if let Ok(content) = std::fs::read_to_string(&path) {
        match parse_stored_turn_count(&content) {
            Some(stored_count) if current_turn_count > stored_count => {
                return false;
            }
            Some(stored_count) if current_turn_count == stored_count => {
                if let Some(stored_fingerprint) = parse_stored_fingerprint(&content) {
                    return stored_fingerprint == current_fingerprint;
                }
                // 旧格式文件（无 fingerprint）视为重复以保持向后兼容
                return true;
            }
            Some(_) => return true,
            None => return true,
        }
    }

    true
}

/// 标记该 session_id 已发送，记录当前对话轮次数
pub fn mark_sent(session_id: &str, turn_count: usize, fingerprint: &str) -> Result<(), String> {
    let dir = dedup_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建去重目录失败: {}", e))?;

    let path = dir.join(sanitize_filename(session_id));
    let content = format!(
        "turns:{}\nfingerprint:{}\n{}",
        turn_count,
        fingerprint,
        chrono::Local::now().to_rfc3339()
    );
    let temp_path = dir.join(format!(
        "{}.{}.tmp",
        sanitize_filename(session_id),
        std::process::id()
    ));

    std::fs::write(&temp_path, content.as_bytes())
        .map_err(|e| format!("写入去重标记失败: {}", e))?;

    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }

    std::fs::rename(&temp_path, &path).map_err(|e| {
        let _ = std::fs::remove_file(&temp_path);
        format!("落盘去重标记失败: {}", e)
    })?;

    // 清理过期标记（不阻塞主流程）
    let _ = cleanup_old_marks(&dir);

    Ok(())
}

/// 清理过期的去重标记
fn cleanup_old_marks(dir: &Path) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("读取去重目录失败: {}", e))?;

    let now = SystemTime::now();
    for entry in entries.flatten() {
        if let Ok(metadata) = entry.metadata() {
            if let Ok(modified) = metadata.modified() {
                if is_age_expired(modified, now) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
    Ok(())
}

fn is_mark_expired(path: &Path, now: SystemTime) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };

    is_age_expired(modified, now)
}

fn is_age_expired(modified: SystemTime, now: SystemTime) -> bool {
    now.duration_since(modified)
        .map(|age| age > MARK_EXPIRY)
        .unwrap_or(false)
}

/// 从去重标记文件内容中解析已存储的对话轮次数
fn parse_stored_turn_count(content: &str) -> Option<usize> {
    for line in content.lines() {
        if let Some(count_str) = line.strip_prefix("turns:") {
            return count_str.trim().parse().ok();
        }
    }
    None
}

fn parse_stored_fingerprint(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(fingerprint) = line.strip_prefix("fingerprint:") {
            let trimmed = fingerprint.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// 将 session_id 转为安全的文件名
fn sanitize_filename(s: &str) -> String {
    let sanitized: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();

    // 限制长度
    if sanitized.len() > 128 {
        sanitized[..128].to_string()
    } else if sanitized.is_empty() {
        "_empty_".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dedup_workflow() {
        let test_id = format!("_test_dedup_{}", std::process::id());

        // 初始状态不应是重复
        // 先清除可能的残留
        let dir = dedup_dir();
        let path = dir.join(sanitize_filename(&test_id));
        let _ = std::fs::remove_file(&path);

        assert!(!is_duplicate(&test_id, 3, "hash-v1"));

        // 标记为已发送，记录 3 轮对话
        mark_sent(&test_id, 3, "hash-v1").unwrap();

        // 相同轮次数应检测为重复
        assert!(is_duplicate(&test_id, 3, "hash-v1"));

        // 更少轮次也应为重复
        assert!(is_duplicate(&test_id, 2, "hash-v1"));

        // 更多轮次（续聊）不应为重复
        assert!(!is_duplicate(&test_id, 5, "hash-v2"));

        // 清理
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_same_turn_count_new_fingerprint_not_duplicate() {
        let test_id = format!("_test_dedup_fp_{}", std::process::id());
        let dir = dedup_dir();
        let path = dir.join(sanitize_filename(&test_id));
        let _ = std::fs::remove_file(&path);

        mark_sent(&test_id, 4, "hash-old").unwrap();

        assert!(!is_duplicate(&test_id, 4, "hash-new"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("abc-123"), "abc-123");
        assert_eq!(sanitize_filename("a/b\\c:d"), "a_b_c_d");
        assert_eq!(sanitize_filename(""), "_empty_");

        // 长字符串截断
        let long = "a".repeat(200);
        assert!(sanitize_filename(&long).len() <= 128);
    }

    #[test]
    fn test_sanitize_special_chars() {
        assert_eq!(sanitize_filename("session<>|?*"), "session_____");
        assert_eq!(sanitize_filename("normal_id-123"), "normal_id-123");
    }

    #[test]
    fn test_cleanup_does_not_crash() {
        // 对不存在的目录不应崩溃
        let non_existent = PathBuf::from("/tmp/copilot-stop-notify-dedup-nonexistent-test");
        // cleanup_old_marks 应该返回错误但不 panic
        let _ = cleanup_old_marks(&non_existent);
    }

    #[test]
    fn test_age_expiry_detection() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let expired = now - MARK_EXPIRY - Duration::from_secs(1);
        let fresh = now - MARK_EXPIRY + Duration::from_secs(1);

        assert!(is_age_expired(expired, now));
        assert!(!is_age_expired(fresh, now));
    }
}
