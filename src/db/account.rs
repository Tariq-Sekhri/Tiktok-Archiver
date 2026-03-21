use std::fs;
use serde::{Deserialize, Serialize};
use crate::db::{atomic_write_text, ensure_file, state_dir};
use crate::db::config::{load_config, is_tracked};
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

pub fn load_accounts() -> Result<Vec<Account>> {
    let path = account_file()?;
    let content = fs::read_to_string(&path).context("failed to read accounts.json")?;
    serde_json::from_str(&content).context("error deserializing accounts")
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
