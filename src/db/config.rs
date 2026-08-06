use std::fs;
use std::env;
use std::path::{Path, PathBuf};
use crate::db::{ensure_file};
use anyhow::Result;
use serde::{Deserialize, Serialize};


#[derive(Serialize, Deserialize)]
pub struct Config {
    pub accounts: Vec<String>,
    pub download_dir: String,
    #[serde(default)]
    pub download_fav: bool,
    #[serde(default = "default_poll_accounts")]
    pub poll_accounts: bool,
}

fn default_poll_accounts() -> bool {
    true
}

pub fn load_config()->Result<Config>{
    let path = config_file()?;
    let file = fs::File::open(&path)?;
    Ok(serde_yaml::from_reader(&file)?)
}



fn config_file() -> Result<PathBuf> {
    let dir = if cfg!(debug_assertions) {
        env::var("CARGO_MANIFEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    } else {
        let exe = env::current_exe()?;
        exe.parent().unwrap_or_else(|| Path::new(".")).to_path_buf()
    };
    let path = dir.join("config.yaml");
    ensure_file(&path, "accounts:\n# - username1\ndownload_dir: downloads\n")?;
    Ok(path)
}


pub fn is_tracked(entry: &str) -> bool {
    let entry = entry.trim();
    !entry.ends_with(":false")
}

pub fn load_tracked_accounts()->Result<Vec<String>>{
    Ok(load_config()?.accounts.into_iter().filter(|acc| is_tracked(acc)).collect())
}
