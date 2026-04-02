use anyhow::{Result, bail};
use copilot_stop_notif::{DEFAULT_ENV_FILE, HookRunSummary, run_notify};
use std::env;
use std::io;
use std::path::PathBuf;
use std::process;

fn main() {
    let (summary, exit_code) = match real_main() {
        Ok(summary) => {
            let exit_code = if summary.ok { 0 } else { 1 };
            (summary, exit_code)
        }
        Err(error) => {
            eprintln!("{error}");
            (HookRunSummary::error(error.to_string()), 1)
        }
    };

    match serde_json::to_string(&summary) {
        Ok(text) => println!("{text}"),
        Err(_) => println!("{{}}"),
    }

    process::exit(exit_code);
}

fn real_main() -> Result<HookRunSummary> {
    let args = parse_args()?;
    let mut stdin = io::stdin();
    run_notify(
        &mut stdin,
        &args.env_file,
        args.dry_run,
        args.payload_json.as_deref(),
    )
}

struct CliArgs {
    env_file: PathBuf,
    dry_run: bool,
    payload_json: Option<String>,
}

fn parse_args() -> Result<CliArgs> {
    let mut parsed = CliArgs {
        env_file: PathBuf::from(DEFAULT_ENV_FILE),
        dry_run: false,
        payload_json: None,
    };
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--env-file" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--env-file 缺少路径参数"))?;
                parsed.env_file = PathBuf::from(value);
            }
            "--dry-run" => {
                parsed.dry_run = true;
            }
            "--help" | "-h" => {
                print_help();
                process::exit(0);
            }
            other if other.starts_with('-') => bail!("不支持的参数: {other}"),
            other => {
                if parsed.payload_json.is_some() {
                    bail!("只支持一个位置参数 JSON 载荷");
                }
                parsed.payload_json = Some(other.to_string());
            }
        }
    }

    Ok(parsed)
}

fn print_help() {
    println!("用法: copilot-stop-notif [--env-file PATH] [--dry-run] [PAYLOAD_JSON]");
}
