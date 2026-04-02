<div align="center">

# 📬 copilot-stop-notif

**面向 Windows 的 VS Code Copilot Stop Hook 邮件通知工具**

在 Copilot 对话结束时，自动读取会话内容并通过 SMTP 发送邮件通知。

[![Rust](https://img.shields.io/badge/Rust-stable-orange?logo=rust)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/Platform-Windows-blue?logo=windows)](https://www.microsoft.com/windows)
[![Hook](https://img.shields.io/badge/VS%20Code-Copilot%20Stop%20Hook-007ACC?logo=visual-studio-code)](https://code.visualstudio.com/)

</div>

---

## 目录

- [特性](#特性)
- [适用范围](#适用范围)
- [安装](#安装)
- [快速开始](#快速开始)
- [配置说明](#配置说明)
- [邮件内容](#邮件内容)
- [Transcript 安全策略](#transcript-安全策略)
- [开发](#开发)
- [项目结构](#项目结构)
- [致谢](#致谢)

---

## ✨ 特性

| 特性 | 说明 |
|------|------|
| 🎯 **目标明确** | 仅支持 VS Code Copilot Stop Hook 场景，配置简单 |
| 📧 **SMTP 通知** | 仅支持 SMTP 邮件，避免引入多渠道配置复杂度 |
| 🖋️ **双格式邮件** | 同时生成纯文本和 HTML 邮件，兼顾可读性与兼容性 |
| 📋 **内容聚焦** | 默认提取最近 24 条会话消息，内容简洁不冗余 |
| 🔒 **隐私保护** | 默认不发送工作目录、会话 ID、线程 ID、轮次 ID |
| 🛡️ **安全认证** | 默认拒绝明文 SMTP 认证，只有显式允许时才启用 |
| 📁 **沙箱读取** | 默认只信任 workspaceStorage 和系统临时目录中的 transcript |
| ✅ **标准输出** | Hook 成功时输出合法 JSON，便于 VS Code 稳定解析 |

---

## 🖥️ 适用范围

> 本项目当前仅适用于以下条件：

- **操作系统**：Windows
- **编辑器**：VS Code
- **Hook 类型**：Copilot Stop Hook
- **通知渠道**：SMTP 邮件

> [!NOTE]
> 如需企业微信、飞书、钉钉等多渠道通知，可参考上游灵感项目 [ai-task-notify](https://github.com/koumoe/ai-task-notify)，本仓库当前不提供这些渠道。

---

## 📦 安装

### 方式一：下载发布包（推荐）

从 [GitHub Releases](../../releases) 下载发布压缩包，解压后即可获得：

```text
.github/hooks/copilot-stop-notif.json
.github/hooks/copilot-stop-notif.env.example
.github/hooks/copilot-stop-notif.exe
README.md
```

### 方式二：本地编译

要求本机已安装 [Rust 工具链](https://rustup.rs/)。

```powershell
cargo build --release
```

编译完成后，可执行文件位于：

```text
target\release\copilot-stop-notif.exe
```

---

## 🚀 快速开始

### 第 1 步 — 准备 Hook 目录

推荐在用户目录下创建以下路径，并将可执行文件与配置文件复制进去：

```text
%USERPROFILE%\.copilot\hooks\
├── copilot-stop-notif.json
└── copilot-stop-notif\
    ├── copilot-stop-notif.exe
    └── copilot-stop-notif.env    ← 由 .env.example 重命名而来
```

其中 `copilot-stop-notif.json` 放在 `%USERPROFILE%\.copilot\hooks\` 根目录，执行文件与环境变量文件放在同名子目录下。
GitHub Actions 生成的发布 zip 也使用这套结构；将压缩包中的 `.github/hooks/` 整体复制到 `%USERPROFILE%\.copilot\hooks\` 即可。

### 第 2 步 — 配置 SMTP 参数

编辑 `copilot-stop-notif.env`：

```env
SMTP_HOST=smtp.example.com
SMTP_PORT=465
SMTP_USER=notify@example.com
SMTP_PASSWORD=replace-with-app-password
SMTP_USE_SSL=true
SMTP_ALLOW_INSECURE_PLAIN=false
# EMAIL_FROM 可留空；如需显式填写，通常应与 SMTP_USER 一致
EMAIL_TO=recipient@example.com
EMAIL_INCLUDE_CONTEXT=false
```

### 第 3 步 — 启用 Hook

仓库已提供示例配置 `.github/hooks/copilot-stop-notif.json`，Windows 命令默认指向：

```powershell
& "$env:USERPROFILE\.copilot\hooks\copilot-stop-notif\copilot-stop-notif.exe" `
    --env-file "$env:USERPROFILE\.copilot\hooks\copilot-stop-notif\copilot-stop-notif.env"
```

### 第 4 步 — Dry-run 验证

在正式使用前，执行以下命令验证配置是否正常（不会真正发信）：

```powershell
@'
{"hookEventName":"Stop","cwd":"D:\\demo","sessionId":"manual-check"}
'@ | .\copilot-stop-notif\copilot-stop-notif.exe --env-file .\copilot-stop-notif\copilot-stop-notif.env --dry-run
```

命令会输出 JSON 结果，便于检查 Hook 是否能正常运行。

---

## ⚙️ 配置说明

| 变量 | 必填 | 默认值 | 说明 |
|------|:----:|:------:|------|
| `SMTP_HOST` | ✅ | — | SMTP 服务器地址 |
| `SMTP_PORT` | ❌ | `465` | SMTP 端口，常见为 `465` 或 `587` |
| `SMTP_USER` | ✅ | — | SMTP 登录用户名 |
| `SMTP_PASSWORD` | ✅ | — | SMTP 登录密码或应用专用密码 |
| `SMTP_USE_SSL` | ❌ | `true` | 是否直接使用 SMTPS |
| `SMTP_ALLOW_INSECURE_PLAIN` | ❌ | `false` | 是否允许明文 SMTP 认证 |
| `EMAIL_FROM` | ❌ | `SMTP_USER` | 发件人地址；不填时默认使用 `SMTP_USER`，部分 SMTP 服务要求与登录账号一致 |
| `EMAIL_TO` | ✅ | — | 收件人地址，多个用英文逗号分隔 |
| `EMAIL_INCLUDE_CONTEXT` | ❌ | `false` | 是否在邮件中附带工作目录、会话 ID 等上下文 |
| `TRANSCRIPT_ALLOWED_ROOTS` | ❌ | — | 额外允许读取 transcript 的专用根目录，多个用 `;` 分隔，避免填写项目目录 |

---

## 📨 邮件内容

默认邮件包含以下信息：

- 🕐 触发时间
- 📌 事件来源与事件类型
- 💬 最后一轮用户输入
- 🤖 最后一轮 AI 回复

> [!TIP]
> 出于隐私保护，工作目录、会话 ID、线程 ID、轮次 ID **默认不写入邮件**。  
> 若需要附带上下文信息，请显式设置 `EMAIL_INCLUDE_CONTEXT=true`。

---

## 🔐 Transcript 安全策略

为防止 `transcript_path` 被滥用为任意文件读取入口，程序默认只信任以下目录：

- ✅ VS Code 的 `workspaceStorage` 目录
- ✅ 系统临时目录

> [!WARNING]
> 程序**不会**因 Hook stdin 中的 `cwd` 自动放行项目目录。  
> 如果 transcript 位于自定义目录，需显式配置 `TRANSCRIPT_ALLOWED_ROOTS`，并只加入专用 transcript 目录，不要把项目目录或下载目录整体加入白名单：

```env
TRANSCRIPT_ALLOWED_ROOTS=%LOCALAPPDATA%\copilot-stop-notif\transcripts
```

此外，程序会拒绝以下类型的输入文件：

- ❌ 非 `.json` 或 `.jsonl` 扩展名
- ❌ 超过大小限制的文件
- ❌ 符号链接文件
- ❌ 非 UTF-8 文本文件

---

## 🛠️ 开发

### 运行测试

```powershell
cargo test
```

### 本地打包

```powershell
./package_release.ps1
```

该脚本会依次执行：

1. 运行 `cargo test`
2. 构建 release 版本可执行文件
3. 生成适合分发的目录与 `.zip` 包
4. 将 README 和 Hook 示例文件一并打入发布包

---

## 📁 项目结构

```text
.
├── .github/
│   ├── hooks/
│   │   ├── copilot-stop-notif.env.example
│   │   └── copilot-stop-notif.json
│   └── workflows/
├── src/
│   ├── lib.rs
│   └── main_copilot_stop_notif.rs
├── tests/
│   └── test_copilot_stop_notif.rs
├── Cargo.toml
├── README.md
└── package_release.ps1
```

---

## 🙏 致谢

本项目在设计方向和通知场景上参考了 [koumoe](https://github.com/koumoe) 的 [ai-task-notify](https://github.com/koumoe/ai-task-notify) 项目，感谢作者提供的开源思路。

本仓库并非对该项目的直接移植，而是基于当前需求，将范围收敛到 **Windows 下 VS Code Copilot Stop Hook 的 SMTP 邮件通知**，并补充了更严格的 transcript 读取限制与默认隐私保护策略。