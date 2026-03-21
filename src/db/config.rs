use std::fs;
use std::env;
use std::path::{Path, PathBuf};
use crate::db::{atomic_write_text, ensure_file};
use anyhow::Result;
use serde::{Deserialize, Serialize};


#[derive(Serialize, Deserialize)]
pub struct Config{
    pub accounts:Vec<String>,
    pub download_dir:String,
}


pub fn load_config()->Result<Config>{
    let path = config_file()?;
    let file = fs::File::open(&path)?;
    Ok(serde_yaml::from_reader(&file)?)
}

pub fn save_config(config:&Config)->Result<()>{
    let path = config_file()?;
    let yaml = serde_yaml::to_string(&config)?;
    atomic_write_text(&path, &yaml)?;
    Ok(())
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

pub fn account_name(entry: &str) -> &str {
    let entry = entry.trim();
    if let Some((name, _)) = entry.split_once(':') {
        name
    } else {
        entry
    }
}

pub fn is_tracked(entry: &str) -> bool {
    let entry = entry.trim();
    !entry.ends_with(":false")
}