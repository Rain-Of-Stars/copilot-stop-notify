// 配置模块：从 .env 文件加载 SMTP 和邮件配置
use crate::redact::summarize_path_for_display;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// SMTP 服务器配置
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub use_ssl: bool,
    /// 保留字段：当前版本始终强制 TLS，明文 SMTP 不受支持
    #[allow(dead_code)]
    pub allow_insecure_plain: bool,
}

/// 邮件配置
pub struct EmailConfig {
    pub from: String,
    pub to: Vec<String>,
    pub include_context: bool,
}

/// Transcript 安全配置
pub struct TranscriptConfig {
    pub allowed_roots: Vec<PathBuf>,
}

/// 完整配置
pub struct Config {
    pub smtp: SmtpConfig,
    pub email: EmailConfig,
    pub transcript: TranscriptConfig,
}

impl Config {
    /// 从 .env 文件路径加载配置
    pub fn load(env_path: &Path) -> Result<Self, String> {
        let vars = parse_env_file(env_path)?;

        let smtp_host = required(&vars, "SMTP_HOST")?;
        let smtp_port = required(&vars, "SMTP_PORT")?
            .parse::<u16>()
            .map_err(|_| "SMTP_PORT 必须是有效端口号".to_string())?;
        let smtp_user = required(&vars, "SMTP_USER")?;
        let smtp_password = required(&vars, "SMTP_PASSWORD")?;
        let smtp_use_ssl = optional_bool(&vars, "SMTP_USE_SSL", true);
        let smtp_allow_insecure_plain = optional_bool(&vars, "SMTP_ALLOW_INSECURE_PLAIN", false);

        let email_from = optional(&vars, "EMAIL_FROM").unwrap_or_else(|| smtp_user.clone());
        let email_to_raw = required(&vars, "EMAIL_TO")?;
        let email_to: Vec<String> = email_to_raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if email_to.is_empty() {
            return Err("EMAIL_TO 至少需要一个收件人".to_string());
        }
        let email_include_context = optional_bool(&vars, "EMAIL_INCLUDE_CONTEXT", false);

        // 构建 transcript 允许读取的根目录白名单
        let mut allowed_roots = default_allowed_roots();
        if let Some(extra) = optional(&vars, "TRANSCRIPT_ALLOWED_ROOTS") {
            for root in extra.split(';') {
                let expanded = expand_env_vars(root.trim());
                if !expanded.is_empty() {
                    allowed_roots.push(PathBuf::from(expanded));
                }
            }
        }

        Ok(Config {
            smtp: SmtpConfig {
                host: smtp_host,
                port: smtp_port,
                user: smtp_user,
                password: smtp_password,
                use_ssl: smtp_use_ssl,
                allow_insecure_plain: smtp_allow_insecure_plain,
            },
            email: EmailConfig {
                from: email_from,
                to: email_to,
                include_context: email_include_context,
            },
            transcript: TranscriptConfig { allowed_roots },
        })
    }
}

/// 手工解析 .env 文件（不依赖 dotenvy crate，减少依赖）
fn parse_env_file(path: &Path) -> Result<HashMap<String, String>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("无法读取配置文件 {}: {}", summarize_path_for_display(path), e))?;

    let mut map = HashMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        // 跳过空行和注释
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(pos) = trimmed.find('=') {
            let key = trimmed[..pos].trim().to_string();
            let value = trimmed[pos + 1..].trim().to_string();
            if !key.is_empty() {
                map.insert(key, value);
            }
        }
    }
    Ok(map)
}

/// 获取必填配置项
fn required(vars: &HashMap<String, String>, key: &str) -> Result<String, String> {
    vars.get(key)
        .filter(|v| !v.is_empty())
        .cloned()
        .ok_or_else(|| format!("缺少必填配置项: {}", key))
}

/// 获取可选配置项
fn optional(vars: &HashMap<String, String>, key: &str) -> Option<String> {
    vars.get(key).filter(|v| !v.is_empty()).cloned()
}

/// 获取可选布尔配置项
fn optional_bool(vars: &HashMap<String, String>, key: &str, default: bool) -> bool {
    match vars.get(key).map(|s| s.to_lowercase()).as_deref() {
        Some("true") | Some("1") | Some("yes") => true,
        Some("false") | Some("0") | Some("no") => false,
        _ => default,
    }
}

/// 展开环境变量占位符（如 %APPDATA%）
fn expand_env_vars(s: &str) -> String {
    let mut result = s.to_string();
    // 最多扩展 16 次，防止变量值本身含 % 导致无限循环
    for _ in 0..16 {
        if let Some(start) = result.find('%') {
            if let Some(end) = result[start + 1..].find('%') {
                let var_name = &result[start + 1..start + 1 + end];
                let replacement = std::env::var(var_name).unwrap_or_default();
                result = format!("{}{}{}", &result[..start], replacement, &result[start + 2 + end..]);
            } else {
                break;
            }
        } else {
            break;
        }
    }
    result
}

/// 默认允许读取 transcript 的根目录
fn default_allowed_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    // VS Code workspaceStorage 目录
    if let Ok(appdata) = std::env::var("APPDATA") {
        roots.push(
            PathBuf::from(&appdata)
                .join("Code")
                .join("User")
                .join("workspaceStorage"),
        );
        // VS Code Insiders
        roots.push(
            PathBuf::from(&appdata)
                .join("Code - Insiders")
                .join("User")
                .join("workspaceStorage"),
        );
    }

    // 系统临时目录
    roots.push(std::env::temp_dir());

    roots
}

/// 查找 .env 配置文件：从可执行文件位置向上搜索
pub fn find_env_file() -> Result<PathBuf, String> {
    // 1. 检查环境变量指定的路径
    if let Ok(path) = std::env::var("COPILOT_STOP_NOTIF_ENV") {
        let p = PathBuf::from(&path);
        if p.is_file() {
            return Ok(p);
        }
    }

    // 2. 从可执行文件位置向上搜索
    if let Ok(exe_path) = std::env::current_exe() {
        let mut dir = exe_path.parent().map(|p| p.to_path_buf());
        while let Some(d) = dir {
            let candidate = d.join("copilot-stop-notif.env");
            if candidate.is_file() {
                return Ok(candidate);
            }
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }

    // 3. 从当前工作目录搜索
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("copilot-stop-notif.env");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err("找不到 copilot-stop-notif.env 配置文件".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 创建临时 .env 文件用于测试
    fn create_temp_env(content: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.env");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        (dir, path)
    }

    #[test]
    fn test_parse_env_file_basic() {
        let (_dir, path) = create_temp_env(
            "SMTP_HOST=smtp.example.com\n\
             SMTP_PORT=465\n\
             # 这是注释\n\
             SMTP_USER=user@example.com\n\
             SMTP_PASSWORD=secret123\n\
             EMAIL_TO=recv@example.com\n",
        );
        let vars = parse_env_file(&path).unwrap();
        assert_eq!(vars.get("SMTP_HOST").unwrap(), "smtp.example.com");
        assert_eq!(vars.get("SMTP_PORT").unwrap(), "465");
        assert_eq!(vars.get("SMTP_USER").unwrap(), "user@example.com");
        assert_eq!(vars.get("SMTP_PASSWORD").unwrap(), "secret123");
        assert_eq!(vars.get("EMAIL_TO").unwrap(), "recv@example.com");
        assert!(vars.get("# 这是注释").is_none());
    }

    #[test]
    fn test_load_config_success() {
        let (_dir, path) = create_temp_env(
            "SMTP_HOST=smtp.example.com\n\
             SMTP_PORT=465\n\
             SMTP_USER=user@example.com\n\
             SMTP_PASSWORD=secret\n\
             SMTP_USE_SSL=true\n\
             EMAIL_TO=a@b.com, c@d.com\n\
             EMAIL_INCLUDE_CONTEXT=false\n",
        );
        let config = Config::load(&path).unwrap();
        assert_eq!(config.smtp.host, "smtp.example.com");
        assert_eq!(config.smtp.port, 465);
        assert!(config.smtp.use_ssl);
        assert_eq!(config.email.to.len(), 2);
        assert_eq!(config.email.to[0], "a@b.com");
        assert_eq!(config.email.to[1], "c@d.com");
        assert_eq!(config.email.from, "user@example.com"); // 默认使用 SMTP_USER
        assert!(!config.email.include_context);
    }

    #[test]
    fn test_load_config_missing_required() {
        let (_dir, path) = create_temp_env("SMTP_HOST=smtp.example.com\n");
        let result = Config::load(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_expand_env_vars() {
        // 测试不含变量的字符串
        assert_eq!(expand_env_vars("/plain/path"), "/plain/path");

        // 设置一个测试环境变量
        std::env::set_var("_TEST_CSN_VAR", "expanded_value");
        assert_eq!(
            expand_env_vars("prefix/%_TEST_CSN_VAR%/suffix"),
            "prefix/expanded_value/suffix"
        );
        std::env::remove_var("_TEST_CSN_VAR");
    }

    #[test]
    fn test_optional_bool_parsing() {
        let mut vars = HashMap::new();
        vars.insert("A".into(), "true".into());
        vars.insert("B".into(), "false".into());
        vars.insert("C".into(), "1".into());
        vars.insert("D".into(), "0".into());

        assert!(optional_bool(&vars, "A", false));
        assert!(!optional_bool(&vars, "B", true));
        assert!(optional_bool(&vars, "C", false));
        assert!(!optional_bool(&vars, "D", true));
        // 不存在的键使用默认值
        assert!(optional_bool(&vars, "MISSING", true));
        assert!(!optional_bool(&vars, "MISSING", false));
    }
}
