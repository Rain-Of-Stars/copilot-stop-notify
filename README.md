# copilot-stop-notify

> **VS Code Copilot 会话结束自动邮件通知工具**

当 VS Code Copilot Chat 完成一次对话（触发 `Stop` 事件）后，自动将完整对话记录以精美 HTML 邮件的形式发送到你的邮箱。

---

## 目录

- [功能特性](#功能特性)
- [工作原理](#工作原理)
- [快速开始](#快速开始)
  - [第一步：下载并解压 ZIP 安装包](#第一步下载并解压-zip-安装包)
  - [第二步：填写配置文件](#第二步填写配置文件)
  - [第三步：注册 VS Code Hook](#第三步注册-vs-code-hook)
- [配置参考](#配置参考)
  - [SMTP 配置](#smtp-配置)
  - [邮件配置](#邮件配置)
  - [高级配置](#高级配置)
- [常用邮箱 SMTP 配置速查](#常用邮箱-smtp-配置速查)
- [从源码构建](#从源码构建)
- [安全说明](#安全说明)
- [故障排查](#故障排查)

---

## 功能特性

| 特性 | 说明 |
|------|------|
| 🔔 **自动触发** | 挂载为 VS Code Copilot `Stop` Hook，无需手动操作 |
| 📧 **HTML 邮件** | 渲染对话气泡风格的精美 HTML 邮件，区分用户/助手/工具消息 |
| 🔒 **自动脱敏** | 邮件内容自动隐藏本机用户名、邮箱地址和绝对路径 |
| 🔁 **幂等去重** | 基于 `session_id` + 内容指纹，同一会话不重复发送 |
| ⏳ **稳定感知** | 等待 transcript 文件写入稳定、AI 助手真正完成回复后再发信 |
| 🌐 **跨平台** | 支持 Windows、Linux 和 macOS（x86_64 / Apple Silicon） |
| 🧩 **零依赖部署** | 单一可执行文件，无需安装运行时 |

---

## 工作原理

```
VS Code Copilot 会话结束
        │
        ▼ Stop 事件
 copilot-stop-notify (本工具)
        │
        ├─ 读取 stdin Hook 事件 JSON
        ├─ 过滤：只处理 Stop，忽略 SubagentStop
        ├─ 等待 transcript JSONL 文件稳定
        ├─ 解析对话轮次（用户 / 助手 / 工具）
        ├─ 敏感信息脱敏
        ├─ 渲染 HTML 邮件
        ├─ 幂等去重检查
        └─ SMTP 发送邮件
```

---

## 快速开始

### 第一步：下载并解压 ZIP 安装包

从 [Releases 页面](../../releases) 下载对应平台的 ZIP 安装包：

| 平台 | 文件名 |
|------|--------|
| Windows (x86_64) | `copilot-stop-notify-windows-x86_64.zip` |
| Linux (x86_64) | `copilot-stop-notify-linux-x86_64.zip` |
| macOS (Intel) | `copilot-stop-notify-macos-x86_64.zip` |
| macOS (Apple Silicon) | `copilot-stop-notify-macos-arm64.zip` |

将 ZIP **直接解压到用户主目录下的 `.copilot` 文件夹**，无需手动创建目录：

**Windows（PowerShell）：**
```powershell
Expand-Archive copilot-stop-notify-windows-x86_64.zip -DestinationPath "$env:USERPROFILE\.copilot"
```

**Linux / macOS：**
```bash
unzip copilot-stop-notify-linux-x86_64.zip -d ~/.copilot
chmod +x ~/.copilot/hooks/copilot-stop-notif/copilot-stop-notify
```

解压后目录结构如下：

```
~/.copilot/
└── hooks/
    ├── copilot-stop-notify.json          ← Claude Code CLI 用户可直接使用
    └── copilot-stop-notif/
        ├── copilot-stop-notify[.exe]     ← 可执行文件
        └── copilot-stop-notif.env.example
```

---

### 第二步：填写配置文件

将 `env.example` 模板复制一份并重命名为 `copilot-stop-notif.env`，然后填入真实 SMTP 参数：

**Windows（PowerShell）：**
```powershell
Copy-Item "$env:USERPROFILE\.copilot\hooks\copilot-stop-notif\copilot-stop-notif.env.example" `
          "$env:USERPROFILE\.copilot\hooks\copilot-stop-notif\copilot-stop-notif.env"
```

**Linux / macOS：**
```bash
cp ~/.copilot/hooks/copilot-stop-notif/copilot-stop-notif.env.example \
   ~/.copilot/hooks/copilot-stop-notif/copilot-stop-notif.env
```

用文本编辑器打开 `copilot-stop-notif.env`，填写 SMTP 参数（以 QQ 邮箱为例）：

```dotenv
# SMTP 服务器
SMTP_HOST=smtp.qq.com
SMTP_PORT=465
SMTP_USER=your_qq@qq.com
SMTP_PASSWORD=your_smtp_authorization_code
SMTP_USE_SSL=true

# 收件人（多个用英文逗号分隔）
EMAIL_TO=your_qq@qq.com
```

> **提示：** 配置文件中的密码是 **SMTP 授权码**，不是邮箱登录密码。各邮箱服务商获取授权码的入口见[下方速查表](#常用邮箱-smtp-配置速查)。

---

### 第三步：注册 VS Code Hook

打开 `~/.claude/settings.json`（不存在则新建），添加以下内容：

```json
{
  "hooks": {
    "Stop": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "& \"$env:USERPROFILE\\.copilot\\hooks\\copilot-stop-notif\\copilot-stop-notify.exe\" --env-file \"$env:USERPROFILE\\.copilot\\hooks\\copilot-stop-notif\\copilot-stop-notif.env\"",
            "timeout": 45
          }
        ]
      }
    ]
  }
}
```

**Linux 用户**将 `command` 替换为：

```json
"command": "\"$HOME/.copilot/hooks/copilot-stop-notif/copilot-stop-notify\" --env-file \"$HOME/.copilot/hooks/copilot-stop-notif/copilot-stop-notif.env\""
```

配置后**重启 VS Code**，下次 Copilot Chat 对话结束时即会自动发送邮件。

---

## 配置参考

完整配置项说明（均在 `.env` 文件中设置）：

### SMTP 配置

| 配置项 | 必填 | 默认值 | 说明 |
|--------|:----:|--------|------|
| `SMTP_HOST` | ✅ | — | SMTP 服务器地址，例如 `smtp.qq.com` |
| `SMTP_PORT` | ✅ | — | SMTP 端口，SSL 通常为 `465`，STARTTLS 为 `587` |
| `SMTP_USER` | ✅ | — | SMTP 登录用户名（通常为完整邮箱地址） |
| `SMTP_PASSWORD` | ✅ | — | SMTP 授权码或密码 |
| `SMTP_USE_SSL` | ❌ | `true` | 是否使用 SSL/TLS 加密连接 |
| `SMTP_ALLOW_INSECURE_PLAIN` | ❌ | `false` | 是否允许明文认证（**不推荐**，当前版本始终强制 TLS） |

### 邮件配置

| 配置项 | 必填 | 默认值 | 说明 |
|--------|:----:|--------|------|
| `EMAIL_FROM` | ❌ | `SMTP_USER` | 发件人地址 |
| `EMAIL_TO` | ✅ | — | 收件人，多个地址用英文逗号 `,` 分隔 |
| `EMAIL_INCLUDE_CONTEXT` | ❌ | `false` | 是否在邮件中附带工作目录和会话 ID 等上下文信息 |

### 高级配置

| 配置项 | 必填 | 默认值 | 说明 |
|--------|:----:|--------|------|
| `TRANSCRIPT_ALLOWED_ROOTS` | ❌ | 系统默认 | 额外允许读取 transcript 的目录，多个路径用 `;` 分隔，支持 `%APPDATA%` 等 Windows 环境变量 |

**完整配置文件示例：**

```dotenv
# ── SMTP 服务器 ──────────────────────────────────────
SMTP_HOST=smtp.example.com
SMTP_PORT=465
SMTP_USER=your_email@example.com
SMTP_PASSWORD=your_smtp_password
SMTP_USE_SSL=true

# ── 邮件设置 ─────────────────────────────────────────
# EMAIL_FROM=notify@example.com  # 可选，默认使用 SMTP_USER
EMAIL_TO=you@example.com,team@example.com

# 是否包含工作目录/会话ID等上下文（敏感信息会自动脱敏）
EMAIL_INCLUDE_CONTEXT=false

# ── 高级：transcript 白名单目录 ───────────────────────
# TRANSCRIPT_ALLOWED_ROOTS=%LOCALAPPDATA%\copilot-logs;D:\custom-path
```

---

## 常用邮箱 SMTP 配置速查

<details>
<summary><strong>QQ 邮箱</strong></summary>

```dotenv
SMTP_HOST=smtp.qq.com
SMTP_PORT=465
SMTP_USER=your_qq@qq.com
SMTP_PASSWORD=<授权码>   # 邮箱设置 → 账户 → POP3/SMTP → 开启并生成授权码
SMTP_USE_SSL=true
```

</details>

<details>
<summary><strong>163 邮箱 / 126 邮箱</strong></summary>

```dotenv
SMTP_HOST=smtp.163.com   # 或 smtp.126.com
SMTP_PORT=465
SMTP_USER=your_email@163.com
SMTP_PASSWORD=<授权码>   # 邮箱设置 → IMAP/SMTP → 开启并获取授权码
SMTP_USE_SSL=true
```

</details>

<details>
<summary><strong>Gmail</strong></summary>

```dotenv
SMTP_HOST=smtp.gmail.com
SMTP_PORT=465
SMTP_USER=your_email@gmail.com
SMTP_PASSWORD=<应用专用密码>   # Google 账户 → 安全性 → 两步验证 → 应用密码
SMTP_USE_SSL=true
```

> Gmail 必须开启两步验证后才能生成应用专用密码。

</details>

<details>
<summary><strong>Outlook / Hotmail</strong></summary>

```dotenv
SMTP_HOST=smtp.office365.com
SMTP_PORT=587
SMTP_USER=your_email@outlook.com
SMTP_PASSWORD=<邮箱密码>
SMTP_USE_SSL=false   # Outlook 使用 STARTTLS（端口 587），不是 SSL（端口 465）
```

</details>

<details>
<summary><strong>企业邮箱（腾讯企业邮箱）</strong></summary>

```dotenv
SMTP_HOST=smtp.exmail.qq.com
SMTP_PORT=465
SMTP_USER=your_email@your_company.com
SMTP_PASSWORD=<邮箱密码>
SMTP_USE_SSL=true
```

</details>

---

## 从源码构建

需要 [Rust 工具链](https://rustup.rs/)（stable，1.75+）。

```bash
# 克隆仓库
git clone https://github.com/your-username/copilot-stop-notify.git
cd copilot-stop-notify

# 构建发布版（体积最小）
cargo build --release

# 可执行文件位于：
# Windows: target\release\copilot-stop-notify.exe
# Linux:   target/release/copilot-stop-notify
```

运行测试：

```bash
cargo test
```

---

## 安全说明

- **密码安全**：配置文件 `copilot-stop-notif.env` 包含 SMTP 密码，请确保文件权限仅限当前用户读取。
  - Windows：右键文件 → 属性 → 安全，移除其他用户的权限
  - Linux：`chmod 600 ~/.copilot/hooks/copilot-stop-notif/copilot-stop-notif.env`

- **自动脱敏**：即使 `EMAIL_INCLUDE_CONTEXT=true`，邮件中也会自动隐藏：
  - 本机 Windows/Linux 用户名
  - 邮箱地址
  - 绝对路径中的用户目录部分

- **Transcript 路径白名单**：工具只读取 VS Code 默认 transcript 目录内的文件，拒绝读取任意路径，防止路径遍历攻击。

- **不上传二进制**：本地构建的可执行文件包含本机源码绝对路径，推荐使用 Release 页提供的 CI 构建版本。

---

## 故障排查

**问题：会话结束后没有收到邮件**

1. 确认 `~/.claude/settings.json` 中的 `command` 路径正确无误
2. 重启 VS Code 后再尝试
3. 查看 VS Code 输出面板（`Copilot Chat` → `Output`）是否有 Hook 错误日志
4. 手动测试命令是否可以执行（在终端运行 `echo {} | <可执行文件路径> --env-file <配置文件路径>`）

**问题：提示 `SMTP 认证失败`**

- 确认使用的是 **SMTP 授权码**而非邮箱登录密码
- 确认 SMTP 服务已在邮箱设置中开启（部分邮箱默认关闭）

**问题：Windows 下提示找不到可执行文件**

- 路径中不能有中文目录，建议使用 `%USERPROFILE%` 下的英文路径
- 检查 `settings.json` 中路径的引号是否完整（PowerShell 语法要求路径用双引号包裹）

**问题：同一会话发送了多封邮件**

- 属于正常情况：当前会话中有新的对话轮次产生时，去重逻辑会允许再次发送
- 如果是完全相同的内容重复发送，请检查系统临时目录（`%TEMP%\copilot-stop-notify-dedup`）是否可写

**问题：邮件内容被截断**

- 单段对话超过 20,000 字符时会被截断，总邮件超过 220,000 字符时会停止渲染后续内容，这是邮件客户端兼容性保护措施

---

## License

MIT
