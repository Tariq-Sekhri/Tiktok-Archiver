use std::fs;
use std::path::Path;
use serde::{Deserialize, Serialize};
use crate::db::{atomic_write_text, ensure_file, state_dir};
use crate::db::config::{load_config, is_tracked};
use crate::db::logger::Log;
use anyhow::{anyhow, Context, Result};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Account {
    pub name: String,
    pub count: i64,
    #[serde(default)]
    pub diff: i64,
    #[serde(default)]
    pub unavailable: i64,
}
impl Account {
    pub fn new(name: String, video_count: i64, diff:i64) -> Self {
        Account {
            name,
            count: video_count,
            diff,
            unavailable: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub enum CountEvent {
    Same,
    Increased,
    Decreased,
}

impl CountEvent {
    pub fn observe(old_count: i64, new_count: i64) -> Self {
        if old_count == new_count {
            CountEvent::Same
        } else if old_count > new_count {
            CountEvent::Decreased
        } else {
            CountEvent::Increased
        }
    }
}

fn utf16_units_to_string(units: &[u16]) -> Result<String> {
    let slice = if units.first() == Some(&0xFEFF) {
        &units[1..]
    } else {
        units
    };
    String::from_utf16(slice).map_err(|e| anyhow!("utf-16 decode: {}", e))
}

fn decode_json_file_bytes(bytes: &[u8]) -> Result<String> {
    if bytes.is_empty() || bytes.iter().all(|b| matches!(b, b' ' | b'\n' | b'\r' | b'\t')) {
        return Ok("[]".to_string());
    }
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let payload = &bytes[2..];
        if payload.len() % 2 != 0 {
            return Err(anyhow!("utf-16le: odd byte length"));
        }
        let mut u16s = Vec::with_capacity(payload.len() / 2);
        for chunk in payload.chunks_exact(2) {
            u16s.push(u16::from_le_bytes([chunk[0], chunk[1]]));
        }
        return utf16_units_to_string(&u16s);
    }
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let payload = &bytes[2..];
        if payload.len() % 2 != 0 {
            return Err(anyhow!("utf-16be: odd byte length"));
        }
        let mut u16s = Vec::with_capacity(payload.len() / 2);
        for chunk in payload.chunks_exact(2) {
            u16s.push(u16::from_be_bytes([chunk[0], chunk[1]]));
        }
        return utf16_units_to_string(&u16s);
    }
    let start = if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
        3
    } else {
        0
    };
    String::from_utf8(bytes[start..].to_vec()).map_err(|e| anyhow!("utf-8: {}", e))
}

fn backup_corrupt_accounts(path_str: &str, original: &[u8]) -> Result<()> {
    let p = Path::new(path_str);
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let dest = p.with_file_name(format!("accounts.json.corrupt.{}", ts));
    fs::write(&dest, original).with_context(|| format!("write {}", dest.display()))?;
    Ok(())
}

pub fn load_accounts() -> Result<Vec<Account>> {
    let path = account_file()?;
    let bytes = fs::read(&path).context("failed to read accounts.json")?;
    let text = decode_json_file_bytes(&bytes).context("invalid text encoding in accounts.json")?;
    match serde_json::from_str::<Vec<Account>>(&text) {
        Ok(v) => Ok(v),
        Err(e) => {
            if let Err(be) = backup_corrupt_accounts(&path, &bytes) {
                return Err(anyhow!(be).context(format!("error deserializing accounts: {}", e)));
            }
            atomic_write_text(Path::new(&path), "[]\n")?;
            Log::error(format!(
                "accounts.json was invalid; backed up to state/accounts.json.corrupt.* and reset: {}",
                e
            ));
            Ok(Vec::new())
        }
    }
}

fn save_accounts(accounts:&Vec<Account>) ->Result<()>{
    let file = account_file()?;
    let json = serde_json::to_string_pretty(&accounts)?;
    atomic_write_text(std::path::Path::new(&file), &json)?;
    Ok(())
}

pub fn add_account(account:&Account)->Result<()>{
    let mut accounts = load_accounts()?;

    if accounts.iter().any(|acc| acc.name == account.name) {
        return Err(anyhow!("account already exists"));
    }

    accounts.push(account.clone());


    save_accounts(&accounts)
}

pub fn update_account_state(account: &Account, count: i64, diff: i64, unavailable: i64) -> Result<()> {
    let mut accounts = load_accounts()?;
    accounts.iter_mut().for_each(|acc| {
        if acc.name == account.name {
            acc.count = count;
            acc.diff = diff;
            acc.unavailable = unavailable;
        }
    });
    save_accounts(&accounts)?;
    Ok(())
}

pub fn load_tracked_accounts() -> Result<Vec<Account>> {
    let accounts = load_accounts()?;
    let tracked_names:Vec<String> = load_config()?
        .accounts.
        into_iter().filter(|acc| is_tracked(acc)).collect();
    
    let filtered: Vec<Account> = accounts
        .into_iter()
        .filter(|a| tracked_names.contains(&a.name))
        .collect();

    Ok(filtered)
}

pub fn account_file() -> Result<String> {
    let path = state_dir().join("accounts.json");
    ensure_file(&path, "[]\n")?;
    Ok(path.to_string_lossy().into_owned())
}
