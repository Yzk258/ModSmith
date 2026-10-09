//! 二进制入口: 只做子命令分发, 其余逻辑在 lib 各模块。
//! - cargo run            → ui::serve (Web UI, 自动打开浏览器)
//! - cargo run -- selftest → selftest::run
//! - cargo run -- ui       → ui::serve
//! - cargo run -- repair   → tools::repair_pack

use rustagent::prelude::*;
use rustagent::{
    cli, config, desktop,
    providers::modrinth,
    selftest,
    tools::{self, ToolRegistry},
    ui,
};
use std::path::{Path, PathBuf};

fn report_startup_error(error: &anyhow::Error) -> ! {
    let message = format!("RustAgent 启动失败\n\n{error:#}");
    let log_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("rustagent-startup.log")))
        .unwrap_or_else(|| PathBuf::from("rustagent-startup.log"));
    let _ = std::fs::write(&log_path, format!("{message}\n"));
    eprintln!("{message}");

    #[cfg(windows)]
    show_windows_error(&message);

    std::process::exit(1);
}

#[cfg(windows)]
fn show_windows_error(message: &str) {
    use std::os::windows::ffi::OsStrExt;
    let title: Vec<u16> = std::ffi::OsStr::new("RustAgent").encode_wide().chain(Some(0)).collect();
    let body: Vec<u16> = std::ffi::OsStr::new(message).encode_wide().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(std::ptr::null_mut(), body.as_ptr(), title.as_ptr(), 0x10);
    }
}

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn MessageBoxW(hwnd: *mut std::ffi::c_void, text: *const u16, caption: *const u16, ty: u32) -> i32;
}

/// 找到配置文件并把进程工作目录切到配置目录。
///
/// 双击 `target\\release\\rustagent.exe` 时，Windows 通常把工作目录设为
/// `target\\release`，而项目的 config.toml 在项目根目录。配置中的
/// `userdata/...` 等相对路径也必须相对于项目根目录解析，因此这里统一切换工作目录。
fn prepare_working_directory() -> Result<String> {
    let mut candidates = Vec::new();
    let cwd_config = PathBuf::from("config.toml");
    candidates.push(cwd_config.clone());

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("config.toml"));
            // cargo 的 target\\debug/release 目录对应项目根目录的两级父目录。
            if let Some(project_root) = dir.parent().and_then(Path::parent) {
                candidates.push(project_root.join("config.toml"));
            }
        }
    }

    let config = candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            anyhow!(
                "找不到 config.toml。请先将 config.example.toml 复制为 config.toml，\n\
                 再填写接口地址、模型和 API Key，然后重新启动程序。\n\
                 配置文件应放在 exe 同目录或项目根目录。"
            )
        })?;
    let config = if config.is_absolute() {
        config
    } else {
        std::env::current_dir()
            .context("读取当前工作目录失败")?
            .join(config)
    };
    let dir = config
        .parent()
        .ok_or_else(|| anyhow!("config.toml 没有有效的父目录"))?;
    std::env::set_current_dir(dir)
        .with_context(|| format!("无法切换到配置目录: {}", dir.display()))?;
    Ok("config.toml".to_string())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        report_startup_error(&error);
    }
}

async fn run() -> Result<()> {
    cli::enable_vt();
    let args: Vec<String> = std::env::args().collect();
    let config_path = prepare_working_directory()?;
    let cfg = config::load(&config_path)?;

    if args.len() > 1 && args[1] == "selftest" {
        return selftest::run(&cfg).await;
    }
    if args.len() > 1 && args[1] == "ui" {
        // --no-browser: 跳过自动打开浏览器 (进程被降权 / 浏览器已提权时系统会拒绝跨 IL 转发)
        return ui::serve(cfg, &config_path, ui::auto_open_enabled(&args[2..])).await;
    }
    if args.len() > 1 && args[1] == "desktop" {
        return desktop::run(cfg);
    }
    if args.len() > 2 && args[1] == "repair" {
        let client = modrinth::ModrinthClient::new()?;
        let registry = ToolRegistry::new(client, None, &cfg.output.download_dir, cfg.db_path());
        let result = registry
            .execute("repair_pack", &args[2..].join(" "), &tools::TaskCtx::none())
            .await?;
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }
    if args.len() > 1 {
        bail!(
            "未知子命令 '{}' (可用: selftest / ui / desktop / repair; 不带参数启动 Web UI)",
            args[1]
        );
    }

    // 默认启动 Web UI；CLI REPL 仍保留在 cli 模块中，供代码复用和后续显式入口扩展。
    ui::serve(cfg, &config_path, true).await
}
