//v0
mod db;
mod discover;
mod download;
pub mod browser;
mod core;

use crate::db::check_state;
use crate::db::logger::{Log};
use crate::discover::login;
use std::{env, io::Write, process};
use std::sync::OnceLock;

use crate::core::default_loop;

//v1
#[derive(Debug)]
pub enum RunMode {
    Login,
    Default,
    Dev,
}
//v1
fn print_usage_and_exit() -> ! {
    eprintln!("  no args = default mode");
    eprintln!(
        "  login   = explicitly run login flow (for switching accounts or refreshing cookies)"
    );
    eprintln!("  dev     = default mode with visible browser and verbose console tracing");
    process::exit(1);
}
//v1
fn parse_args() -> RunMode {
    let args: Vec<String> = env::args().collect();
    if let Some(arg) = args.get(1) {
        match arg.as_str() {
            "login" => RunMode::Login,
            "dev" => RunMode::Dev,
            _ => print_usage_and_exit(),
        }
    } else {
        RunMode::Default
    }
}
//v1
fn print_how_to_use_and_exit(reason: &str) -> ! {
    Log::critical_fail(reason.to_string());
    eprintln!("\n[State Check] {}\n", reason);
    eprintln!("How to use, in order:");
    eprintln!("  1) run");
    eprintln!("     - On first run, this will prompt you to log in and save cookies into `state/saved_cookies.json`");
    eprintln!("  2) update config.yaml");
    eprintln!("     - Choose which accounts you want to track and optionally change download_dir.");
    eprintln!("  3) run");
    eprintln!(
        "     - Default mode: poll for new videos + download pending using your saved login."
    );
    eprintln!("  4) cargo run dev");
    eprintln!("     - Debug mode: run default loop but show browser windows.");
    eprintln!("  5) cargo run login");
    eprintln!("     - Explicitly run the login flow to switch accounts or refresh cookies.");
    process::exit(1);
}
pub static DEV_MODE : OnceLock<bool>=OnceLock::new();
//v1
#[tokio::main]
async fn main() {
    let mode = parse_args();
    Log::console(format!("Tiktok-Archiver 1.1.0 | Run Mode:{:?}", mode));
    DEV_MODE.set(matches!(mode, RunMode::Dev)).expect("TODO: panic message");
    check_state(&mode).await;
    match mode {
        RunMode::Login => login().await.unwrap_or_else(|e| {
            let msg = format!("Error logging in: {}", e);
            Log::critical_fail(msg.clone());
        }),
        RunMode::Default | RunMode::Dev => default_loop().await,
    }
}
