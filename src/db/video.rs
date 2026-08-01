
use std::{collections::{HashMap, HashSet}, fs, path::PathBuf, };
use anyhow::{Context, Result, Error};
use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use crate::db::{atomic_write_text, ensure_file, state_dir};
use crate::db::config::load_config;
use crate::db::logger::Log;
use crate::db::critical_alert::alert_download_unavailable;

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy)]
pub enum DownloadStatus {
    Downloaded,
    NotDownloaded,
    DownloadFailed(u8),
}
pub fn serialize_download_date<S>(opt: &Option<NaiveDateTime>, s: S, ) -> std::result::Result<S::Ok, S::Error> where S: Serializer,{
    match opt {
        Some(dt) => s.serialize_str(&dt.format("%Y-%m-%d %I:%M:%S %p").to_string()),
        None => s.serialize_none(),
    }
}
pub fn deserialize_download_date<'de, D>(    d: D,) -> std::result::Result<Option<NaiveDateTime>, D::Error>where    D: Deserializer<'de>, {
    let opt: Option<String> = Option::deserialize(d)?;
    match opt {
        None => Ok(None),
        Some(s) => {
            if s.len() == 10 {
                NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                    .map_err(serde::de::Error::custom)
                    .and_then(|d| {
                        d.and_hms_opt(0, 0, 0)
                            .map(Some)
                            .ok_or_else(|| serde::de::Error::custom("invalid date"))
                    })
            } else if s.contains('T') {
                NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S")
                    .map(Some)
                    .map_err(serde::de::Error::custom)
            } else {
                NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %I:%M:%S %p")
                    .map(Some)
                    .map_err(serde::de::Error::custom)
            }
        }
    }
}
pub const VIDEO_EXT: &str = "mp4";


#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Video {
    pub url: String,
    pub id: i64,
    pub username: String,
    #[serde(default)]
    pub is_fav: bool,
    pub download_status: DownloadStatus,
    #[serde(default = "default_source_available")]
    pub source_available: bool,
    #[serde(
        serialize_with = "serialize_download_date",
        deserialize_with = "deserialize_download_date"
    )]
    pub download_date: Option<NaiveDateTime>,
}
fn default_source_available() -> bool {
    true
}

impl Video {
    pub fn new(url: String, video_id: i64, username: String) -> Self {
        Self {
            url,
            id: video_id,
            username,
            is_fav: false,
            download_status: DownloadStatus::NotDownloaded,
            source_available: true,
            download_date: None,

        }

    }
    pub fn is_pending(&self) -> bool {
        if !self.source_available {
            return false;
        }
        match self.download_status {
            DownloadStatus::Downloaded => false,
            DownloadStatus::NotDownloaded => true,
            DownloadStatus::DownloadFailed(failed_count) => failed_count < 5,
        }
    }

    pub fn file_path(&self)->Result<PathBuf>{
        let folder = if self.is_fav {
            "favs"
        } else {
            &self.username
        };

        Ok(PathBuf::from(load_config()?.download_dir).join(folder).join(format!("{}.{}", self.id, VIDEO_EXT )))
    }
    pub fn download_failed(&mut self, e: &Error) {
        self.download_status = match self.download_status {
            DownloadStatus::Downloaded => {
                Log::critical_fail("download failed on a video already download??".to_string())
            }
            DownloadStatus::NotDownloaded => DownloadStatus::DownloadFailed(1),
            DownloadStatus::DownloadFailed(n) => DownloadStatus::DownloadFailed(n + 1),
        };
        if let DownloadStatus::DownloadFailed(n) = self.download_status {
            if n >= 5 {
                self.source_available = false;
                let message = format!(
                    "Video {} from @{} failed to download {} times and has been paused.\n\nLast yt-dlp error:\n{:#}\n\nSee state/error.log. Re-enable it after fixing the source/session issue.",
                    self.id, self.username, n, e
                );
                Log::error(message.clone());
                alert_download_unavailable(&message);
            }
        }
    }
}


pub fn videos_file() -> Result<PathBuf> {
    let path = state_dir().join("seen_videos.json");
    ensure_file(&path, "{}\n")?;
    Ok(path)
}


pub fn load_all() -> Result<HashMap<String, Vec<Video>>> {
    let path = videos_file()?;
    let file = fs::File::open(path)?;
    serde_json::from_reader(file).context("Error loading videos")
}

pub fn save_all(map: &HashMap<String, Vec<Video>>) -> Result<()> {
    let path = videos_file()?;
    let json = if cfg!(debug_assertions) {
        serde_json::to_string_pretty(map)?
    } else {
        serde_json::to_string(map)?
    };
    atomic_write_text(&path, &json)?;
    Ok(())
}

pub fn append_videos(map: &mut HashMap<String, Vec<Video>>,username: &str,vids:&[Video]) -> usize {
    let user_vids = map.entry(username.to_string()).or_default();
    let mut existing_ids: HashSet<i64> = user_vids.iter().map(|v| v.id).collect();
    let mut added = 0;
    for vid in vids {
        if existing_ids.insert(vid.id) {
            user_vids.push(vid.clone());
            added += 1;
        }
    }
    added
}
