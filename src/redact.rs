use std::path::Path;

pub fn safe_prefix(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub fn redact_sensitive_text(text: &str) -> String {
    let windows_redacted = redact_windows_user_paths(text);
    let unix_redacted = redact_unix_user_paths(&windows_redacted);
    redact_email_addresses(&unix_redacted)
}

pub fn summarize_path_for_display(path: &Path) -> String {
    summarize_path_str_for_display(&path.to_string_lossy())
}

pub fn summarize_path_str_for_display(path: &str) -> String {
    let trimmed = path.strip_prefix("file:///").unwrap_or(path);
    let normalized = trimmed.replace('\\', "/");
    let bytes = normalized.as_bytes();
    let is_absolute = normalized.starts_with('/')
        || (bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':');

    if !is_absolute {
        return redact_sensitive_text(trimmed);
    }

    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    if parts.is_empty() {
        return "[绝对路径已隐藏]".to_string();
    }

    let home_prefix_len = if parts.len() >= 3 && parts[0].ends_with(':') && parts[1].eq_ignore_ascii_case("Users") {
        Some(3usize)
    } else if parts.len() >= 2 && parts[0] == "Users" {
        Some(2usize)
    } else if parts.len() >= 2 && parts[0] == "home" {
        Some(2usize)
    } else {
        None
    };

    if let Some(prefix_len) = home_prefix_len {
        let tail_start = parts.len().saturating_sub(2).max(prefix_len);
        if tail_start < parts.len() {
            return redact_sensitive_text(&format!(".../{}", parts[tail_start..].join("/")));
        }

        return if prefix_len == 3 || parts[0] == "Users" {
            ".../Users/***".to_string()
        } else {
            ".../home/***".to_string()
        };
    }

    let tail_start = parts.len().saturating_sub(2);
    redact_sensitive_text(&format!(".../{}", parts[tail_start..].join("/")))
}

fn redact_email_addresses(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut result = String::with_capacity(text.len());
    let mut last = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] != b'@' {
            i += 1;
            continue;
        }

        let mut start = i;
        while start > 0 && is_email_local_byte(bytes[start - 1]) {
            start -= 1;
        }

        let mut end = i + 1;
        while end < bytes.len() && is_email_domain_byte(bytes[end]) {
            end += 1;
        }

        if start == i || end <= i + 1 {
            i += 1;
            continue;
        }

        let candidate = &text[start..end];
        if !looks_like_email_domain(candidate) {
            i += 1;
            continue;
        }

        let local = &text[start..i];
        let domain = &text[i + 1..end];
        let first = local.chars().next().unwrap_or('*');

        result.push_str(&text[last..start]);
        result.push(first);
        result.push_str("***@");
        result.push_str(domain);

        last = end;
        i = end;
    }

    result.push_str(&text[last..]);
    result
}

fn looks_like_email_domain(candidate: &str) -> bool {
    let Some((_, domain)) = candidate.split_once('@') else {
        return false;
    };
    let last_dot = domain.rfind('.').unwrap_or(0);
    last_dot > 0
        && last_dot < domain.len() - 1
        && domain[last_dot + 1..]
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic())
}

fn is_email_local_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'%' | b'+' | b'-')
}

fn is_email_domain_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')
}

fn redact_windows_user_paths(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut result = String::with_capacity(text.len());
    let mut last = 0usize;
    let mut i = 0usize;

    while i + 9 < bytes.len() {
        if !bytes[i].is_ascii_alphabetic() || bytes[i + 1] != b':' {
            i += 1;
            continue;
        }

        let mut cursor = i + 2;
        while cursor < bytes.len() && bytes[cursor] == b'\\' {
            cursor += 1;
        }
        if cursor == i + 2 || !starts_with_ignore_ascii(&bytes[cursor..], b"Users") {
            i += 1;
            continue;
        }

        cursor += 5;
        let user_sep_start = cursor;
        while cursor < bytes.len() && bytes[cursor] == b'\\' {
            cursor += 1;
        }
        if cursor == user_sep_start {
            i += 1;
            continue;
        }

        let user_start = cursor;
        while cursor < bytes.len()
            && bytes[cursor] != b'\\'
            && bytes[cursor] != b'/'
            && !bytes[cursor].is_ascii_whitespace()
        {
            cursor += 1;
        }
        if cursor == user_start {
            i += 1;
            continue;
        }

        result.push_str(&text[last..user_start]);
        result.push_str("***");
        last = cursor;
        i = cursor;
    }

    result.push_str(&text[last..]);
    result
}

fn redact_unix_user_paths(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut result = String::with_capacity(text.len());
    let mut last = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        let prefix_len = if starts_with_ascii(&bytes[i..], b"/Users/") {
            Some(7usize)
        } else if starts_with_ascii(&bytes[i..], b"/home/") {
            Some(6usize)
        } else {
            None
        };

        let Some(prefix_len) = prefix_len else {
            i += 1;
            continue;
        };

        let user_start = i + prefix_len;
        let mut cursor = user_start;
        while cursor < bytes.len()
            && bytes[cursor] != b'/'
            && bytes[cursor] != b'\\'
            && !bytes[cursor].is_ascii_whitespace()
        {
            cursor += 1;
        }
        if cursor == user_start {
            i += 1;
            continue;
        }

        result.push_str(&text[last..user_start]);
        result.push_str("***");
        last = cursor;
        i = cursor;
    }

    result.push_str(&text[last..]);
    result
}

fn starts_with_ascii(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len() && &haystack[..needle.len()] == needle
}

fn starts_with_ignore_ascii(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len()
        && haystack[..needle.len()]
            .iter()
            .zip(needle.iter())
            .all(|(actual, expected)| actual.eq_ignore_ascii_case(expected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_redact_sensitive_text_masks_email_and_paths() {
        let text = "C:\\Users\\alice\\workspace /Users/alice/project /home/alice/app alice@example.com";
        let redacted = redact_sensitive_text(text);
        assert!(!redacted.contains("alice@example.com"));
        assert!(!redacted.contains("C:\\Users\\alice\\workspace"));
        assert!(!redacted.contains("/Users/alice/project"));
        assert!(!redacted.contains("/home/alice/app"));
        assert!(redacted.contains("a***@example.com"));
        assert!(redacted.contains("C:\\Users\\***\\workspace"));
        assert!(redacted.contains("/Users/***/project"));
        assert!(redacted.contains("/home/***/app"));
    }

    #[test]
    fn test_summarize_path_for_display_hides_home_segments() {
        assert_eq!(
            summarize_path_for_display(Path::new("C:\\Users\\alice\\workspace\\src\\main.rs")),
            ".../src/main.rs"
        );
        assert_eq!(
            summarize_path_str_for_display("/Users/alice/project/src/lib.rs"),
            ".../src/lib.rs"
        );
        assert_eq!(
            summarize_path_str_for_display("/home/alice"),
            ".../home/***"
        );
    }
}