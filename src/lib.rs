use anyhow::{Context, Result, anyhow, bail};
use chrono::Local;
use lettre::message::{Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::io::{AsHandle, AsRawHandle};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};

pub const DEFAULT_ENV_FILE: &str = ".github/hooks/copilot-stop-notif.env";

const EMAIL_CHANNEL_NAME: &str = "email";
const MAX_ENV_FILE_BYTES: u64 = 64 * 1024;
const MAX_HOOK_INPUT_BYTES: u64 = 256 * 1024;
const MAX_TRANSCRIPT_BYTES: u64 = 1024 * 1024;
const MAX_TRANSCRIPT_MESSAGES: usize = 24;
const MAX_MESSAGE_CHARS: usize = 4000;
const MAX_FALLBACK_CHARS: usize = 8000;
const EMAIL_FOOTER_BRAND: &str = "copilot-stop-notify";

const CONTAINER_KEYS: [&str; 10] = [
    "messages",
    "transcript",
    "conversation",
    "turns",
    "items",
    "entries",
    "events",
    "chunks",
    "children",
    "nodes",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationMessage {
    pub role: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptData {
    Json(Value),
    RawText(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookPayload {
    pub cwd: String,
    pub session_id: String,
    pub event_type: String,
    pub transcript: Vec<ConversationMessage>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NotificationRequest {
    pub source: String,
    pub event_type: String,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailConfig {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_user: String,
    pub smtp_password: String,
    pub smtp_use_ssl: bool,
    pub smtp_allow_insecure_plain: bool,
    pub email_from: String,
    pub recipients: Vec<String>,
    pub email_include_context: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HookRunSummary {
    pub ok: bool,
    pub sent: bool,
    pub ignored: bool,
    pub source: String,
    pub channel_count: usize,
    pub success_count: usize,
    pub recipient_count: usize,
    pub title: String,
    pub event_type: String,
    pub channel_results: BTreeMap<String, bool>,
    pub error: Option<String>,
}

impl HookRunSummary {
    fn success(
        source: String,
        event_type: String,
        title: String,
        recipient_count: usize,
        email_sent: bool,
        dry_run: bool,
    ) -> Self {
        let mut channel_results = BTreeMap::new();
        channel_results.insert(EMAIL_CHANNEL_NAME.to_string(), !dry_run && email_sent);

        Self {
            ok: dry_run || email_sent,
            sent: !dry_run && email_sent,
            ignored: false,
            source,
            channel_count: 1,
            success_count: usize::from(!dry_run && email_sent),
            recipient_count,
            title,
            event_type,
            channel_results,
            error: None,
        }
    }

    fn failure(
        source: String,
        event_type: String,
        title: String,
        recipient_count: usize,
        message: impl Into<String>,
    ) -> Self {
        let mut channel_results = BTreeMap::new();
        channel_results.insert(EMAIL_CHANNEL_NAME.to_string(), false);

        Self {
            ok: false,
            sent: false,
            ignored: false,
            source,
            channel_count: 1,
            success_count: 0,
            recipient_count,
            title,
            event_type,
            channel_results,
            error: Some(message.into()),
        }
    }

    fn ignored(source: String, event_type: String) -> Self {
        Self {
            ok: true,
            sent: false,
            ignored: true,
            source,
            channel_count: 0,
            success_count: 0,
            recipient_count: 0,
            title: String::new(),
            event_type,
            channel_results: BTreeMap::new(),
            error: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            sent: false,
            ignored: false,
            source: String::new(),
            channel_count: 0,
            success_count: 0,
            recipient_count: 0,
            title: String::new(),
            event_type: String::new(),
            channel_results: BTreeMap::new(),
            error: Some(message.into()),
        }
    }
}

pub fn load_env_file(path: &Path) -> Result<HashMap<String, String>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let raw_text = read_text_file_limited(path, MAX_ENV_FILE_BYTES, "配置文件")?;
    let mut env_map = HashMap::new();

    for line in raw_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if let Some((key, value)) = trimmed.split_once('=') {
            env_map.insert(key.trim().to_string(), value.trim().to_string());
        }
    }

    Ok(env_map)
}

pub fn load_email_config(path: &Path) -> Result<EmailConfig> {
    let env_map = load_env_file(path)?;
    load_email_config_from_map(&env_map)
}

fn load_email_config_from_map(env_map: &HashMap<String, String>) -> Result<EmailConfig> {
    validate_legacy_channels(env_map)?;

    let smtp_host = required_config(env_map, "SMTP_HOST")?;
    let smtp_port = config_value(env_map, "SMTP_PORT")
        .unwrap_or_else(|| "465".to_string())
        .parse::<u16>()
        .context("SMTP_PORT 不是合法端口号")?;
    if smtp_port == 0 {
        bail!("SMTP_PORT 不能为 0");
    }

    let smtp_user = required_config(env_map, "SMTP_USER")?;
    let smtp_password = required_config(env_map, "SMTP_PASSWORD")?;
    let smtp_use_ssl = config_value(env_map, "SMTP_USE_SSL")
        .map(|value| parse_bool(&value))
        .unwrap_or(true);
    let smtp_allow_insecure_plain = config_value(env_map, "SMTP_ALLOW_INSECURE_PLAIN")
        .map(|value| parse_bool(&value))
        .unwrap_or(false);
    let email_from = config_value(env_map, "EMAIL_FROM").unwrap_or_else(|| smtp_user.clone());
    let email_include_context = config_value(env_map, "EMAIL_INCLUDE_CONTEXT")
        .map(|value| parse_bool(&value))
        .unwrap_or(false);
    let recipients = required_config(env_map, "EMAIL_TO")?
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    if recipients.is_empty() {
        bail!("EMAIL_TO 至少需要一个收件人");
    }

    Ok(EmailConfig {
        smtp_host,
        smtp_port,
        smtp_user,
        smtp_password,
        smtp_use_ssl,
        smtp_allow_insecure_plain,
        email_from,
        recipients,
        email_include_context,
    })
}

fn validate_legacy_channels(env_map: &HashMap<String, String>) -> Result<()> {
    let Some(channels) = config_value(env_map, "NOTIFY_CHANNELS") else {
        return Ok(());
    };

    for channel in channels.split(',').map(str::trim).filter(|item| !item.is_empty()) {
        if !channel.eq_ignore_ascii_case(EMAIL_CHANNEL_NAME) {
            bail!(
                "当前版本仅支持 email 通知渠道，请移除不支持的配置项: {channel}"
            );
        }
    }

    Ok(())
}

pub fn load_hook_input_from_reader<R: Read>(reader: &mut R) -> Result<Value> {
    let raw_input = read_text_from_reader_limited(reader, MAX_HOOK_INPUT_BYTES, "Hook stdin")?;

    if raw_input.trim().is_empty() {
        bail!("Hook stdin 为空");
    }

    let parsed = serde_json::from_str::<Value>(&raw_input).context("Hook stdin 不是合法 JSON")?;
    if !parsed.is_object() {
        bail!("Hook stdin 必须是 JSON 对象");
    }

    Ok(parsed)
}

pub fn load_transcript_data(path: &Path) -> Result<Option<TranscriptData>> {
    let raw_text = read_text_file_limited(path, MAX_TRANSCRIPT_BYTES, "transcript")?;

    if raw_text.trim().is_empty() {
        return Ok(None);
    }

    if let Ok(value) = serde_json::from_str::<Value>(&raw_text) {
        return Ok(Some(TranscriptData::Json(value)));
    }

    let mut records = Vec::new();
    for line in raw_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match serde_json::from_str::<Value>(trimmed) {
            Ok(value) => records.push(value),
            Err(_) => return Ok(Some(TranscriptData::RawText(raw_text))),
        }
    }

    if records.is_empty() {
        return Ok(None);
    }

    Ok(Some(TranscriptData::Json(Value::Array(records))))
}

pub fn build_hook_payload(
    hook_input: &Value,
    transcript_data: Option<&TranscriptData>,
) -> HookPayload {
    let mut transcript = transcript_data.map(extract_messages).unwrap_or_default();

    if transcript.is_empty() {
        transcript.push(ConversationMessage {
            role: "assistant".to_string(),
            text: fallback_text(hook_input, transcript_data),
        });
    }

    HookPayload {
        cwd: extract_string_field(hook_input, &["cwd"]),
        session_id: extract_string_field(hook_input, &["sessionId", "session_id"]),
        event_type: extract_string_field(hook_input, &["hookEventName", "notify_event_type"])
            .to_lowercase(),
        transcript,
    }
}

pub fn build_plain_text_body(payload: &HookPayload) -> String {
    build_plain_text_body_with_options(payload, false)
}

pub fn build_plain_text_body_with_options(payload: &HookPayload, include_context: bool) -> String {
    let now = now_text();
    let last_user = last_message_by_role(payload, "human").unwrap_or("(无内容)");
    let last_ai = last_message_by_role(payload, "assistant").unwrap_or("(无内容)");
    let mut metadata = vec![
        format!("时间: {now}"),
        format!("事件类型: {}", payload.event_type),
    ];

    if include_context {
        metadata.push(format!(
            "工作目录: {}",
            if payload.cwd.is_empty() {
                "N/A"
            } else {
                payload.cwd.as_str()
            }
        ));
        metadata.push(format!(
            "会话ID: {}",
            if payload.session_id.is_empty() {
                "N/A"
            } else {
                payload.session_id.as_str()
            }
        ));
    }

    format!(
        "{}\n\n用户指令:\n{last_user}\n\nAI 回复:\n{last_ai}",
        metadata.join("\n")
    )
}

pub fn send_email(
    config: &EmailConfig,
    title: &str,
    text_body: &str,
    html_body: &str,
) -> Result<()> {
    let from: Mailbox = config
        .email_from
        .parse()
        .context("发件人地址格式不正确")?;

    let mut builder = Message::builder().from(from).subject(title);
    for recipient in &config.recipients {
        let mailbox: Mailbox = recipient
            .parse()
            .context("收件人地址格式不正确")?;
        builder = builder.to(mailbox);
    }

    let message = builder.multipart(
        MultiPart::alternative()
            .singlepart(SinglePart::plain(text_body.to_string()))
            .singlepart(SinglePart::html(html_body.to_string())),
    )?;

    let transport = if config.smtp_use_ssl {
        SmtpTransport::relay(&config.smtp_host)
            .context("创建 SMTPS 连接失败")?
            .port(config.smtp_port)
            .credentials(Credentials::new(
                config.smtp_user.clone(),
                config.smtp_password.clone(),
            ))
            .build()
    } else if config.smtp_allow_insecure_plain {
        SmtpTransport::builder_dangerous(&config.smtp_host)
            .port(config.smtp_port)
            .credentials(Credentials::new(
                config.smtp_user.clone(),
                config.smtp_password.clone(),
            ))
            .build()
    } else {
        SmtpTransport::starttls_relay(&config.smtp_host)
            .context("创建 STARTTLS 连接失败")?
            .port(config.smtp_port)
            .credentials(Credentials::new(
                config.smtp_user.clone(),
                config.smtp_password.clone(),
            ))
            .build()
    };

    transport
        .send(&message)
        .map_err(|error| anyhow!("SMTP 发送失败: {error}"))?;

    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
enum ParsedNotifyInput {
    Request(NotificationRequest),
    Ignored { source: String, event_type: String },
}

pub fn build_vscode_notify_payload(
    hook_input: &Value,
    transcript_data: Option<&TranscriptData>,
) -> Value {
    let payload = build_hook_payload(hook_input, transcript_data);
    let transcript = payload
        .transcript
        .iter()
        .map(|message| {
            json!({
                "type": message.role,
                "message": {
                    "content": [
                        {
                            "type": "text",
                            "text": message.text,
                        }
                    ]
                }
            })
        })
        .collect::<Vec<_>>();

    json!({
        "cwd": payload.cwd,
        "session_id": payload.session_id,
        "notify_source": "vscode-copilot",
        "notify_event_type": payload.event_type,
        "transcript": transcript,
    })
}

fn parse_notify_request<R: Read>(
    reader: &mut R,
    payload_json: Option<&str>,
    env_map: &HashMap<String, String>,
) -> Result<ParsedNotifyInput> {
    if let Some(raw_payload) = payload_json.map(str::trim).filter(|value| !value.is_empty()) {
        let payload =
            serde_json::from_str::<Value>(raw_payload).context("命令行 JSON 不是合法 JSON")?;
        if !payload.is_object() {
            bail!("命令行 JSON 必须是对象");
        }

        let event_type = extract_string_field(&payload, &["type"]);
        if event_type != "agent-turn-complete" {
            return Ok(ParsedNotifyInput::Ignored {
                source: "codex".to_string(),
                event_type,
            });
        }

        return Ok(ParsedNotifyInput::Request(NotificationRequest {
            source: "codex".to_string(),
            event_type,
            data: payload,
        }));
    }

    let hook_input = load_hook_input_from_reader(reader)?;
    let transcript_path = extract_string_field(&hook_input, &["transcript_path", "transcriptPath"]);
    let is_vscode_hook = hook_input.get("hookEventName").is_some() || !transcript_path.is_empty();

    if is_vscode_hook {
        let transcript_data = load_vscode_transcript(&hook_input, &transcript_path, env_map);
        let data = build_vscode_notify_payload(&hook_input, transcript_data.as_ref());
        let event_type = extract_string_field(&data, &["notify_event_type"]);

        return Ok(ParsedNotifyInput::Request(NotificationRequest {
            source: "vscode-copilot".to_string(),
            event_type,
            data,
        }));
    }

    let source = extract_string_field(&hook_input, &["notify_source"]);
    let event_type = extract_string_field(&hook_input, &["notify_event_type"]);

    Ok(ParsedNotifyInput::Request(NotificationRequest {
        source: if source.is_empty() {
            "claude-code".to_string()
        } else {
            source.to_ascii_lowercase()
        },
        event_type: if event_type.is_empty() {
            "stop".to_string()
        } else {
            event_type.to_ascii_lowercase()
        },
        data: hook_input,
    }))
}

fn load_vscode_transcript(
    hook_input: &Value,
    transcript_path: &str,
    env_map: &HashMap<String, String>,
) -> Option<TranscriptData> {
    if transcript_path.is_empty() {
        return None;
    }

    let path = PathBuf::from(transcript_path);
    if !path.is_file() {
        return None;
    }

    if validate_transcript_path(&path, hook_input, env_map).is_err() {
        eprintln!("transcript_path 已忽略");
        return None;
    }

    load_transcript_data(&path).ok().flatten()
}

pub fn validate_transcript_path(
    path: &Path,
    hook_input: &Value,
    env_map: &HashMap<String, String>,
) -> Result<()> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    if !matches!(extension.as_deref(), Some("json") | Some("jsonl")) {
        bail!("transcript 扩展名不受支持");
    }

    let canonical_path = fs::canonicalize(path).context("无法解析 transcript 路径")?;
    let allowed_roots = collect_allowed_transcript_roots(hook_input, env_map);

    if allowed_roots
        .iter()
        .any(|root| canonical_path.starts_with(root))
    {
        ensure_path_has_single_link(path, "transcript")?;
        return Ok(());
    }

    bail!("transcript 路径不在允许目录内")
}

fn collect_allowed_transcript_roots(
    _hook_input: &Value,
    env_map: &HashMap<String, String>,
) -> Vec<PathBuf> {
    let mut roots = default_transcript_roots();

    if let Some(configured) = config_value(env_map, "TRANSCRIPT_ALLOWED_ROOTS") {
        for item in configured.split(';').map(str::trim).filter(|item| !item.is_empty()) {
            let expanded = expand_env_placeholders(item);
            if let Some(path) = canonicalize_existing_dir(PathBuf::from(expanded)) {
                push_unique_root(&mut roots, path);
            }
        }
    }

    roots
}

fn default_transcript_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(path) = code_workspace_storage_dir().and_then(canonicalize_existing_dir) {
        push_unique_root(&mut roots, path);
    }

    if let Some(path) = canonicalize_existing_dir(env::temp_dir()) {
        push_unique_root(&mut roots, path);
    }

    roots
}

fn push_unique_root(roots: &mut Vec<PathBuf>, path: PathBuf) {
    if !roots.iter().any(|root| root == &path) {
        roots.push(path);
    }
}

fn expand_env_placeholders(input: &str) -> String {
    let mut output = String::new();
    let chars = input.chars().collect::<Vec<_>>();
    let mut index = 0;

    while index < chars.len() {
        if chars[index] == '%' {
            let start = index + 1;
            if let Some(offset) = chars[start..].iter().position(|value| *value == '%') {
                let end = start + offset;
                let key = chars[start..end].iter().collect::<String>();

                if !key.is_empty() && let Some(value) = env::var_os(&key) {
                    output.push_str(&value.to_string_lossy());
                    index = end + 1;
                    continue;
                }
            }
        }

        output.push(chars[index]);
        index += 1;
    }

    output
}

fn code_workspace_storage_dir() -> Option<PathBuf> {
    if let Some(appdata) = env::var_os("APPDATA") {
        return Some(
            PathBuf::from(appdata)
                .join("Code")
                .join("User")
                .join("workspaceStorage"),
        );
    }

    if let Some(config_home) = env::var_os("XDG_CONFIG_HOME") {
        return Some(
            PathBuf::from(config_home)
                .join("Code")
                .join("User")
                .join("workspaceStorage"),
        );
    }

    home_dir().map(|path| {
        path.join(".config")
            .join("Code")
            .join("User")
            .join("workspaceStorage")
    })
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(PathBuf::from))
}

fn canonicalize_existing_dir(path: PathBuf) -> Option<PathBuf> {
    if !path.is_dir() {
        return None;
    }

    fs::canonicalize(path).ok()
}

#[cfg(unix)]
fn ensure_path_has_single_link(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::metadata(path).with_context(|| format!("读取 {label} 元数据失败"))?;

    if metadata.nlink() > 1 {
        bail!("{label} 不能使用硬链接文件")
    }

    Ok(())
}

#[cfg(windows)]
fn ensure_path_has_single_link(path: &Path, label: &str) -> Result<()> {
    let file = File::open(path).with_context(|| format!("读取 {label} 失败"))?;
    let mut file_info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    let result = unsafe {
        GetFileInformationByHandle(
            file.as_handle().as_raw_handle() as _,
            file_info.as_mut_ptr(),
        )
    };

    if result == 0 {
        return Err(std::io::Error::last_os_error()).context(format!("读取 {label} 元数据失败"));
    }

    let file_info = unsafe { file_info.assume_init() };
    if file_info.nNumberOfLinks > 1 {
        bail!("{label} 不能使用硬链接文件")
    }

    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn ensure_path_has_single_link(_path: &Path, _label: &str) -> Result<()> {
    Ok(())
}

pub fn extract_conversation(data: &Value, source: &str) -> Vec<ConversationMessage> {
    let mut messages = Vec::new();

    match source {
        "claude-code" | "vscode-copilot" => {
            if let Some(items) = data.get("transcript").and_then(Value::as_array) {
                for item in items {
                    let msg_type = extract_string_field(item, &["type"]);
                    let role = match msg_type.as_str() {
                        "human" => "user",
                        "assistant" => "assistant",
                        _ => continue,
                    };

                    let text = item
                        .get("message")
                        .map(text_from_value)
                        .unwrap_or_else(|| text_from_value(item));

                    if !text.is_empty() {
                        messages.push(ConversationMessage {
                            role: role.to_string(),
                            text,
                        });
                    }
                }
            }
        }
        "codex" => {
            if let Some(items) = data.get("input-messages").and_then(Value::as_array) {
                for item in items {
                    let text = match item {
                        Value::String(text) => text.trim().to_string(),
                        Value::Object(map) => map
                            .get("content")
                            .map(text_from_value)
                            .unwrap_or_else(|| text_from_value(item)),
                        _ => String::new(),
                    };

                    if !text.is_empty() {
                        messages.push(ConversationMessage {
                            role: "user".to_string(),
                            text,
                        });
                    }
                }
            }

            if let Some(last_message) = data.get("last-assistant-message") {
                let text = text_from_value(last_message);
                if !text.is_empty() {
                    messages.push(ConversationMessage {
                        role: "assistant".to_string(),
                        text,
                    });
                }
            }

            if messages.is_empty() {
                messages.push(ConversationMessage {
                    role: "assistant".to_string(),
                    text: pretty_json(data),
                });
            }
        }
        _ => {
            messages.push(ConversationMessage {
                role: "assistant".to_string(),
                text: pretty_json(data),
            });
        }
    }

    messages
}

pub fn format_message(source: &str, event_type: &str, data: &Value) -> (String, String) {
    format_message_with_options(source, event_type, data, false)
}

pub fn format_message_with_options(
    source: &str,
    event_type: &str,
    data: &Value,
    include_context: bool,
) -> (String, String) {
    let now = now_text();

    match source {
        "claude-code" | "vscode-copilot" => {
            let conversation = extract_conversation(data, source);
            let last_user = conversation
                .iter()
                .rev()
                .find(|message| message.role == "user")
                .map(|message| message.text.as_str())
                .unwrap_or("(无内容)");
            let last_ai = conversation
                .iter()
                .rev()
                .find(|message| message.role == "assistant")
                .map(|message| message.text.as_str())
                .unwrap_or("(无内容)");
            let title = if source == "vscode-copilot" {
                "Copilot 任务完成"
            } else {
                "Claude Code 任务完成"
            };
            let mut metadata = vec![format!("**时间**: {now}")];
            if include_context {
                let session_id = data
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(|value| shorten(value, 8))
                    .unwrap_or_else(|| "N/A".to_string());
                metadata.push(format!(
                    "**工作目录**: {}",
                    data.get("cwd").and_then(Value::as_str).unwrap_or("N/A")
                ));
                metadata.push(format!("**会话ID**: {session_id}"));
            }

            (
                title.to_string(),
                format!(
                    "{}\n\n**用户指令**:\n{last_user}\n\n**AI 回复**:\n{last_ai}",
                    metadata.join("\n")
                ),
            )
        }
        "codex" => {
            let conversation = extract_conversation(data, source);
            let user_message = conversation
                .iter()
                .filter(|message| message.role == "user")
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let ai_message = conversation
                .iter()
                .rev()
                .find(|message| message.role == "assistant")
                .map(|message| message.text.as_str())
                .unwrap_or("(无内容)");

            let mut metadata = vec![
                format!("**时间**: {now}"),
                format!("**事件类型**: {event_type}"),
            ];
            if include_context {
                metadata.push(format!(
                    "**工作目录**: {}",
                    data.get("cwd").and_then(Value::as_str).unwrap_or("N/A")
                ));
            }

            (
                "Codex 任务完成".to_string(),
                format!(
                    "{}\n\n**用户指令**:\n{user_message}\n\n**AI 回复**:\n{ai_message}",
                    metadata.join("\n"),
                    user_message = if user_message.is_empty() {
                        "(无内容)"
                    } else {
                        user_message.as_str()
                    },
                ),
            )
        }
        _ => (
            "AI 任务完成".to_string(),
            format!(
                "**时间**: {now}\n**来源**: {source}\n\n**数据**:\n```json\n{}\n```",
                pretty_json(data)
            ),
        ),
    }
}

pub fn build_email_html(title: &str, source: &str, data: &Value) -> String {
    build_email_html_with_options(title, source, data, false)
}

pub fn build_email_html_with_options(
    title: &str,
    source: &str,
    data: &Value,
    include_context: bool,
) -> String {
    let (accent, accent_light, accent_dark) = match source {
        "claude-code" => ("#D97706", "#FEF3C7", "#92400E"),
        "vscode-copilot" => ("#0969DA", "#DDF4FF", "#0550AE"),
        "codex" => ("#059669", "#D1FAE5", "#065F46"),
        _ => ("#2563EB", "#DBEAFE", "#1E40AF"),
    };

    let now = now_text();
    let mut meta_rows = vec![("完成时间".to_string(), now.clone())];

    let derived_event_type = if source == "codex" {
        extract_string_field(data, &["type"])
    } else {
        extract_string_field(data, &["notify_event_type", "event_type"])
    };
    if !derived_event_type.is_empty() {
        meta_rows.push(("事件类型".to_string(), derived_event_type));
    }

    if include_context {
        meta_rows.extend(collect_email_context_fields(data));
    }

    let meta_html = meta_rows
        .iter()
        .map(|(key, value)| {
            format!(
                "<tr><td style=\"padding:8px 14px;color:#6B7280;font-size:13px;white-space:nowrap;vertical-align:top;border-bottom:1px solid #F3F4F6;\">{}</td><td style=\"padding:8px 14px;color:#111827;font-size:13px;word-break:break-word;overflow-wrap:anywhere;border-bottom:1px solid #F3F4F6;\">{}</td></tr>",
                escape_html(key),
                escape_html(value)
            )
        })
        .collect::<String>();

    let conversation = extract_conversation(data, source);
    let conversation_html = if conversation.is_empty() {
        format!(
            "<tr><td style=\"padding:0 24px 16px;\"><pre style=\"margin:0;padding:16px;background:#1F2937;color:#E5E7EB;font-size:12px;line-height:1.6;font-family:'SF Mono','Fira Code',Consolas,monospace;border-radius:8px;white-space:pre-wrap;word-break:break-word;overflow-wrap:anywhere;\">{}</pre></td></tr>",
            escape_html(&pretty_json(data))
        )
    } else {
        conversation
            .iter()
            .map(|message| {
                let (role_label, role_color, bg_color, border_color) = if message.role == "user" {
                    ("USER", "#4F46E5", "#EEF2FF", "#6366F1")
                } else {
                    ("AI ASSISTANT", "#047857", "#F0FDF4", "#10B981")
                };

                format!(
                    "<tr><td style=\"padding:0 24px 12px;\"><table width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" style=\"border-collapse:collapse;\"><tr><td style=\"padding:14px 16px;background:{bg_color};border-left:3px solid {border_color};border-radius:4px;\"><div style=\"font-size:11px;font-weight:700;color:{role_color};text-transform:uppercase;letter-spacing:0.5px;margin-bottom:8px;\">{role_label}</div><div style=\"font-size:14px;color:#1F2937;line-height:1.7;word-break:break-word;overflow-wrap:anywhere;\">{}</div></td></tr></table></td></tr>",
                    text_to_html(&message.text)
                )
            })
            .collect::<String>()
    };

    format!(
        "<!DOCTYPE html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><meta http-equiv=\"x-ua-compatible\" content=\"IE=edge\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1.0\"><meta name=\"x-apple-disable-message-reformatting\"></head><body style=\"margin:0;padding:0;background-color:#F3F4F6;width:100% !important;min-width:100%;-webkit-text-size-adjust:100%;-ms-text-size-adjust:100%;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,'Helvetica Neue',Arial,sans-serif;\"><table width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" style=\"border-collapse:collapse;background:#F3F4F6;\"><tr><td align=\"center\" style=\"padding:32px 16px;\"><table width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" style=\"border-collapse:collapse;background:#FFFFFF;max-width:640px;width:100%;table-layout:fixed;border-radius:12px;overflow:hidden;box-shadow:0 4px 24px rgba(0,0,0,0.08);\"><tr><td style=\"height:4px;background:{accent};font-size:0;line-height:0;\">&nbsp;</td></tr><tr><td style=\"padding:24px 24px 16px;\"><table width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" style=\"border-collapse:collapse;\"><tr><td style=\"vertical-align:middle;\"><span style=\"font-size:20px;font-weight:700;color:#111827;\">{}</span></td><td align=\"right\" style=\"vertical-align:middle;\"><span style=\"display:inline-block;padding:4px 12px;background:{accent_light};color:{accent_dark};font-size:11px;font-weight:600;border-radius:20px;letter-spacing:0.3px;\">COMPLETED</span></td></tr></table></td></tr><tr><td style=\"padding:0 24px;\"><hr style=\"border:none;border-top:1px solid #E5E7EB;margin:0;\"></td></tr><tr><td style=\"padding:16px 24px;\"><table width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" style=\"border-collapse:collapse;background:#F9FAFB;border-radius:8px;border:1px solid #E5E7EB;\">{meta_html}</table></td></tr><tr><td style=\"padding:8px 24px 12px;\"><span style=\"font-size:12px;font-weight:600;color:#6B7280;text-transform:uppercase;letter-spacing:0.5px;\">Conversation</span></td></tr>{conversation_html}<tr><td style=\"padding:16px 24px;background:#F9FAFB;border-top:1px solid #E5E7EB;\"><table width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" style=\"border-collapse:collapse;\"><tr><td style=\"font-size:11px;color:#9CA3AF;\">{}</td><td align=\"right\" style=\"font-size:11px;color:#9CA3AF;\">{}</td></tr></table></td></tr></table><table width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" style=\"border-collapse:collapse;max-width:640px;width:100%;\"><tr><td align=\"center\" style=\"padding:16px 0;font-size:11px;color:#9CA3AF;\">此邮件由系统自动生成，请勿直接回复</td></tr></table></td></tr></table></body></html>",
        escape_html(title),
        EMAIL_FOOTER_BRAND,
        escape_html(&now)
    )
}

pub fn build_email_text_body(source: &str, event_type: &str, data: &Value) -> String {
    build_email_text_body_with_options(source, event_type, data, false)
}

pub fn build_email_text_body_with_options(
    source: &str,
    event_type: &str,
    data: &Value,
    include_context: bool,
) -> String {
    let now = now_text();
    let conversation = extract_conversation(data, source);
    let last_user = conversation
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.text.as_str())
        .unwrap_or("(无内容)");
    let last_ai = conversation
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .map(|message| message.text.as_str())
        .unwrap_or("(无内容)");

    let mut metadata = vec![
        format!("时间: {now}"),
        format!("来源: {source}"),
        format!("事件类型: {event_type}"),
    ];

    if include_context {
        for (label, value) in collect_email_context_fields(data) {
            metadata.push(format!("{label}: {value}"));
        }
    }

    format!(
        "{}\n\n用户指令:\n{last_user}\n\nAI 回复:\n{last_ai}",
        metadata.join("\n")
    )
}

pub fn run_notify<R: Read>(
    reader: &mut R,
    env_file: &Path,
    dry_run: bool,
    payload_json: Option<&str>,
) -> Result<HookRunSummary> {
    let env_map = load_env_file(env_file)?;
    let request = match parse_notify_request(reader, payload_json, &env_map)? {
        ParsedNotifyInput::Ignored { source, event_type } => {
            return Ok(HookRunSummary::ignored(source, event_type));
        }
        ParsedNotifyInput::Request(request) => request,
    };

    let NotificationRequest {
        source,
        event_type,
        data,
    } = request;
    let (title, _) = format_message(&source, &event_type, &data);
    let config = match load_email_config_from_map(&env_map) {
        Ok(config) => config,
        Err(error) => {
            return Ok(HookRunSummary::failure(
                source,
                event_type,
                title,
                0,
                error.to_string(),
            ));
        }
    };
    let recipient_count = config.recipients.len();

    if dry_run {
        return Ok(HookRunSummary::success(
            source,
            event_type,
            title,
            recipient_count,
            false,
            true,
        ));
    }

    let text_body = build_email_text_body_with_options(
        &source,
        &event_type,
        &data,
        config.email_include_context,
    );
    let html_body = build_email_html_with_options(
        &title,
        &source,
        &data,
        config.email_include_context,
    );

    match send_email(&config, &title, &text_body, &html_body) {
        Ok(()) => Ok(HookRunSummary::success(
            source,
            event_type,
            title,
            recipient_count,
            true,
            false,
        )),
        Err(error) => {
            eprintln!("{error}");
            Ok(HookRunSummary::failure(
                source,
                event_type,
                title,
                recipient_count,
                error.to_string(),
            ))
        }
    }
}

pub fn run_hook<R: Read>(reader: &mut R, env_file: &Path, dry_run: bool) -> Result<HookRunSummary> {
    run_notify(reader, env_file, dry_run, None)
}

fn config_value(env_map: &HashMap<String, String>, key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .or_else(|| env_map.get(key).cloned())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn required_config(env_map: &HashMap<String, String>, key: &str) -> Result<String> {
    config_value(env_map, key).ok_or_else(|| anyhow!("缺少配置项: {key}"))
}

fn parse_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn extract_messages(transcript_data: &TranscriptData) -> Vec<ConversationMessage> {
    let mut raw_messages = Vec::new();
    if let TranscriptData::Json(value) = transcript_data {
        collect_messages(value, &mut raw_messages);
    }

    let mut messages = Vec::new();
    for message in raw_messages {
        if message.text.is_empty() {
            continue;
        }

        let bounded_message = ConversationMessage {
            role: message.role,
            text: truncate_text(&message.text, MAX_MESSAGE_CHARS),
        };

        if messages.last() != Some(&bounded_message) {
            messages.push(bounded_message);
        }
    }

    if messages.len() > MAX_TRANSCRIPT_MESSAGES {
        messages = messages.split_off(messages.len() - MAX_TRANSCRIPT_MESSAGES);
    }

    messages
}

fn collect_messages(node: &Value, messages: &mut Vec<ConversationMessage>) {
    match node {
        Value::Array(items) => {
            for item in items {
                collect_messages(item, messages);
            }
        }
        Value::Object(map) => {
            if let Some(message) = extract_vscode_event_message(map) {
                messages.push(message);
                return;
            }

            let role = map
                .get("role")
                .and_then(Value::as_str)
                .and_then(normalized_role)
                .or_else(|| {
                    map.get("type")
                        .and_then(Value::as_str)
                        .and_then(normalized_role)
                });

            if let Some(role) = role {
                for key in ["content", "message", "text", "value", "parts"] {
                    if let Some(value) = map.get(key) {
                        let text = text_from_value(value);
                        if !text.is_empty() {
                            messages.push(ConversationMessage {
                                role: role.to_string(),
                                text,
                            });
                            return;
                        }
                    }
                }
            }

            let mut found_container = false;
            for key in CONTAINER_KEYS {
                if let Some(value) = map.get(key)
                    && matches!(value, Value::Array(_) | Value::Object(_))
                {
                    found_container = true;
                    collect_messages(value, messages);
                }
            }

            if found_container {
                return;
            }

            for value in map.values() {
                if matches!(value, Value::Array(_) | Value::Object(_)) {
                    collect_messages(value, messages);
                }
            }
        }
        _ => {}
    }
}

fn extract_vscode_event_message(node: &Map<String, Value>) -> Option<ConversationMessage> {
    let event_type = node.get("type")?.as_str()?;
    if !event_type.ends_with(".message") {
        return None;
    }

    let role = normalized_role(event_type.split('.').next()?)?;
    let data = node.get("data")?.as_object()?;
    let mut text = data.get("content").map(text_from_value).unwrap_or_default();
    if text.is_empty() && let Some(message) = data.get("message") {
        text = text_from_value(message);
    }

    if text.is_empty() {
        return None;
    }

    Some(ConversationMessage {
        role: role.to_string(),
        text,
    })
}

fn text_from_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_string(),
        Value::Array(items) => items
            .iter()
            .map(text_from_value)
            .filter(|item| !item.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("text")
                && let Some(text) = map.get("text").and_then(Value::as_str)
            {
                return text.trim().to_string();
            }

            for key in ["text", "value"] {
                if let Some(text) = map.get(key).and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    return text.trim().to_string();
                }
            }

            for key in ["content", "message", "parts", "items"] {
                if let Some(nested) = map.get(key) {
                    let text = text_from_value(nested);
                    if !text.is_empty() {
                        return text;
                    }
                }
            }

            String::new()
        }
        _ => String::new(),
    }
}

fn normalized_role(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "user" | "human" | "prompt" => Some("human"),
        "assistant" | "model" | "ai" | "copilot" => Some("assistant"),
        _ => None,
    }
}

fn extract_string_field(value: &Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            return text.trim().to_string();
        }
    }

    String::new()
}

fn fallback_text(hook_input: &Value, transcript_data: Option<&TranscriptData>) -> String {
    let message = match transcript_data {
        Some(TranscriptData::RawText(_)) => {
            "已读取 transcript，但内容不是可识别的会话 JSON，已省略原文。"
        }
        Some(TranscriptData::Json(_)) => "已读取 transcript，但未提取到可显示的对话内容。",
        None => {
            let event_type = extract_string_field(hook_input, &["hookEventName", "notify_event_type"]);
            if event_type.is_empty() {
                "未读取到可用 transcript，已省略原始 Hook 输入。"
            } else {
                "未读取到可用 transcript，已省略原始 Hook 输入，仅保留事件摘要。"
            }
        }
    };

    truncate_text(message, MAX_FALLBACK_CHARS)
}

fn pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| json!({ "raw": value }).to_string())
}

fn last_message_by_role<'a>(payload: &'a HookPayload, role: &str) -> Option<&'a str> {
    payload
        .transcript
        .iter()
        .rev()
        .find(|message| message.role == role)
        .map(|message| message.text.as_str())
}

fn now_text() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn shorten(text: &str, max_len: usize) -> String {
    if text.chars().count() <= max_len {
        return text.to_string();
    }

    text.chars().take(max_len).collect::<String>() + "..."
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn text_to_html(text: &str) -> String {
    let mut html_parts = Vec::new();
    let mut in_code = false;
    let mut code_lines = Vec::new();

    for line in text.lines() {
        if line.trim().starts_with("```") {
            if in_code {
                html_parts.push(render_code_block(&code_lines.join("\n")));
                code_lines.clear();
                in_code = false;
            } else {
                in_code = true;
            }
            continue;
        }

        if in_code {
            code_lines.push(line.to_string());
            continue;
        }

        if line.trim().is_empty() {
            html_parts.push("<br>".to_string());
        } else {
            html_parts.push(format!("{}<br>", render_inline_markup(line)));
        }
    }

    if in_code && !code_lines.is_empty() {
        html_parts.push(render_code_block(&code_lines.join("\n")));
    }

    html_parts.join("\n")
}

fn render_code_block(code: &str) -> String {
    format!(
        "<pre style=\"margin:8px 0;padding:12px 16px;background:#1F2937;color:#E5E7EB;font-size:12px;line-height:1.6;font-family:'SF Mono','Fira Code',Consolas,monospace;border-radius:6px;white-space:pre-wrap;word-break:break-all;overflow:hidden;\">{}</pre>",
        escape_html(code)
    )
}

fn render_inline_markup(text: &str) -> String {
    let escaped = escape_html(text);
    let mut rendered = String::new();
    let mut rest = escaped.as_str();

    while let Some(start) = rest.find("**") {
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("**") else {
            rendered.push_str(rest);
            return rendered;
        };

        rendered.push_str(&rest[..start]);
        rendered.push_str("<strong>");
        rendered.push_str(&after_start[..end]);
        rendered.push_str("</strong>");
        rest = &after_start[end + 2..];
    }

    rendered.push_str(rest);
    rendered
}

fn collect_email_context_fields(data: &Value) -> Vec<(String, String)> {
    let mut fields = Vec::new();

    if let Some(cwd) = data
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        fields.push(("工作目录".to_string(), cwd.to_string()));
    }

    let session_id = extract_string_field(data, &["session_id", "sessionId"]);
    if !session_id.is_empty() {
        fields.push(("会话ID".to_string(), session_id));
    }

    let thread_id = extract_string_field(data, &["thread-id"]);
    if !thread_id.is_empty() {
        fields.push(("线程ID".to_string(), thread_id));
    }

    let turn_id = extract_string_field(data, &["turn-id"]);
    if !turn_id.is_empty() {
        fields.push(("轮次ID".to_string(), turn_id));
    }

    fields
}

fn read_text_file_limited(path: &Path, max_bytes: u64, label: &str) -> Result<String> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("读取 {label} 元数据失败"))?;
    let file_type = metadata.file_type();

    if file_type.is_symlink() {
        bail!("{label} 不能是符号链接");
    }
    if !file_type.is_file() {
        bail!("{label} 不是普通文件");
    }
    if metadata.len() > max_bytes {
        bail!("{label} 超过大小限制({max_bytes} 字节)");
    }

    let file = File::open(path).with_context(|| format!("读取 {label} 失败"))?;
    let mut raw_bytes = Vec::with_capacity(metadata.len().min(max_bytes) as usize + 1);
    file.take(max_bytes + 1)
        .read_to_end(&mut raw_bytes)
        .with_context(|| format!("读取 {label} 失败"))?;

    if raw_bytes.len() as u64 > max_bytes {
        bail!("{label} 超过大小限制({max_bytes} 字节)");
    }

    String::from_utf8(raw_bytes).with_context(|| format!("{label} 不是 UTF-8 文本"))
}

fn read_text_from_reader_limited<R: Read>(
    reader: &mut R,
    max_bytes: u64,
    label: &str,
) -> Result<String> {
    let mut raw_bytes = Vec::with_capacity(max_bytes.min(8192) as usize + 1);
    reader
        .take(max_bytes + 1)
        .read_to_end(&mut raw_bytes)
        .with_context(|| format!("读取 {label} 失败"))?;

    if raw_bytes.len() as u64 > max_bytes {
        bail!("{label} 超过大小限制({max_bytes} 字节)");
    }

    String::from_utf8(raw_bytes).with_context(|| format!("{label} 不是 UTF-8 文本"))
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut truncated = text.chars().take(max_chars).collect::<String>();
    truncated.push_str("\n\n[内容已截断]");
    truncated
}
