// 邮件发送模块：通过 SMTP 发送 HTML 邮件
use crate::config::SmtpConfig;
use crate::redact::redact_sensitive_text;
use lettre::message::{header::ContentType, Mailbox};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{Message, SmtpTransport, Transport};
use std::time::Duration;

const SEND_MAX_ATTEMPTS: usize = 3;

/// 构建并发送 HTML 邮件
pub fn send_email(
    smtp_config: &SmtpConfig,
    from: &str,
    to_list: &[String],
    subject: &str,
    html_body: &str,
) -> Result<(), String> {
    // 解析发件人地址
    let from_mailbox: Mailbox = from
        .parse()
        .map_err(|e| format!("发件人地址无效 '{}': {}", from, e))?;

    // 构建邮件
    let mut builder = Message::builder()
        .from(from_mailbox)
        .subject(subject);

    // 添加所有收件人
    for addr in to_list {
        let to_mailbox: Mailbox = addr
            .parse()
            .map_err(|e| format!("收件人地址无效 '{}': {}", addr, e))?;
        builder = builder.to(to_mailbox);
    }

    let email = builder
        .header(ContentType::TEXT_HTML)
        .body(html_body.to_string())
        .map_err(|e| format!("构建邮件失败: {}", e))?;

    send_with_retry(|| {
        let transport = build_transport(smtp_config)?;
        transport
            .send(&email)
            .map_err(|e| format!("发送邮件失败: {}", e))?;
        Ok(())
    })
}

/// 构建 SMTP 传输通道
fn build_transport(config: &SmtpConfig) -> Result<SmtpTransport, String> {
    let creds = Credentials::new(config.user.clone(), config.password.clone());

    let tls_params = TlsParameters::new(config.host.clone())
        .map_err(|e| format!("TLS 参数创建失败: {}", e))?;

    let transport = if config.use_ssl {
        // 使用隐式 TLS（端口 465）
        SmtpTransport::relay(&config.host)
            .map_err(|e| format!("创建 SMTP 连接失败: {}", e))?
            .port(config.port)
            .credentials(creds)
            .tls(Tls::Wrapper(tls_params))
            .build()
    } else {
        // 使用 STARTTLS（端口 587）
        SmtpTransport::starttls_relay(&config.host)
            .map_err(|e| format!("创建 SMTP STARTTLS 连接失败: {}", e))?
            .port(config.port)
            .credentials(creds)
            .tls(Tls::Required(tls_params))
            .build()
    };

    Ok(transport)
}

fn send_with_retry<F>(mut send_once: F) -> Result<(), String>
where
    F: FnMut() -> Result<(), String>,
{
    for attempt in 1..=SEND_MAX_ATTEMPTS {
        match send_once() {
            Ok(()) => return Ok(()),
            Err(err) if attempt < SEND_MAX_ATTEMPTS => {
                eprintln!(
                    "[copilot-stop-notify] 邮件发送第 {} 次尝试失败，准备重试: {}",
                    attempt,
                    redact_sensitive_text(&err)
                );
                let delay = retry_delay(attempt);
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
            }
            Err(err) => return Err(err),
        }
    }

    Err("邮件发送失败：达到最大重试次数".to_string())
}

fn retry_delay(attempt: usize) -> Duration {
    #[cfg(test)]
    {
        let _ = attempt;
        Duration::ZERO
    }

    #[cfg(not(test))]
    {
        Duration::from_secs(attempt as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mailbox_parsing() {
        // 合法邮箱
        let result: Result<Mailbox, _> = "user@example.com".parse();
        assert!(result.is_ok());

        // 带显示名的邮箱
        let result: Result<Mailbox, _> = "Test User <user@example.com>".parse();
        assert!(result.is_ok());
    }

    #[test]
    fn test_invalid_mailbox() {
        let result: Result<Mailbox, _> = "not-an-email".parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_build_transport_ssl() {
        let config = SmtpConfig {
            host: "smtp.example.com".to_string(),
            port: 465,
            user: "user@example.com".to_string(),
            password: "password".to_string(),
            use_ssl: true,
            allow_insecure_plain: false,
        };
        let result = build_transport(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_transport_starttls() {
        let config = SmtpConfig {
            host: "smtp.example.com".to_string(),
            port: 587,
            user: "user@example.com".to_string(),
            password: "password".to_string(),
            use_ssl: false,
            allow_insecure_plain: false,
        };
        let result = build_transport(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_send_email_invalid_from() {
        let config = SmtpConfig {
            host: "smtp.example.com".to_string(),
            port: 465,
            user: "user".to_string(),
            password: "pass".to_string(),
            use_ssl: true,
            allow_insecure_plain: false,
        };
        let result = send_email(
            &config,
            "not-valid",
            &["recv@example.com".to_string()],
            "test",
            "<p>test</p>",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_send_with_retry_eventually_succeeds() {
        let mut attempts = 0;
        let result = send_with_retry(|| {
            attempts += 1;
            if attempts < 3 {
                Err(format!("临时失败 {}", attempts))
            } else {
                Ok(())
            }
        });

        assert!(result.is_ok());
        assert_eq!(attempts, 3);
    }

    #[test]
    fn test_send_with_retry_returns_last_error() {
        let mut attempts = 0;
        let result = send_with_retry(|| {
            attempts += 1;
            Err(format!("持续失败 {}", attempts))
        });

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("持续失败 3"));
        assert_eq!(attempts, SEND_MAX_ATTEMPTS);
    }
}
