//v0

use std::{

    collections::{HashMap, HashSet},

    fs,

    path::PathBuf,

};



use anyhow::{Context, Result};

use chrono::{Local, NaiveDate, NaiveDateTime};

use serde::{Deserialize, Deserializer, Serialize, Serializer};



use crate::db::logger::dev_mode_enabled;
use crate::db::{atomic_write_text, ensure_file, state_dir};
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy)]
pub enum DownloadStatus {
    Downloaded,
    NotDownloaded,
    DownloadFailed,
}
//v0
pub fn serialize_download_date<S>(opt: &Option<NaiveDateTime>, s: S, ) -> std::result::Result<S::Ok, S::Error> where S: Serializer,{
    match opt {
        Some(dt) => s.serialize_str(&dt.format("%Y-%m-%d %I:%M:%S %p").to_string()),
        None => s.serialize_none(),
    }
}
//v0
pub fn deserialize_download_date<'de, D>(    d: D,) -> std::result::Result<Option<NaiveDateTime>, D::Error>where    D: Deserializer<'de>, {
    let opt: Option<String> = Option::deserialize(d)?;
    match opt {
        None => Ok(None),
        Some(s) => {
            if s.len() == 10 {
                NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                    .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
                    .map(Some)
                    .map_err(serde::de::Error::custom)
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
//v0
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Video {
    pub url: String,
    pub video_id: i64,
    pub username: String,
    #[serde(default)]
    pub is_fav: bool,
    pub download_status: DownloadStatus,
    pub source_available: bool,
    #[serde(
        serialize_with = "serialize_download_date",
        deserialize_with = "deserialize_download_date"
    )]
    pub download_date: Option<NaiveDateTime>,
}
//v0
impl Video {
    pub fn new(url: String, video_id: i64, username: String) -> Self {
        Self {
            url,
            video_id,
            username,
            is_fav: false,
            download_status: DownloadStatus::NotDownloaded,
            source_available: true,
            download_date: None,

        }

    }

}


//v1
pub fn videos_file() -> Result<PathBuf> {
    let path = state_dir().join("seen_videos.json");
    ensure_file(&path, "{}\n")?;
    Ok(path)
}


//v1
pub fn load_all() -> Result<HashMap<String, Vec<Video>>> {
    let path = videos_file()?;
    let file = fs::File::open(path)?;
    serde_json::from_reader(file).context("Error loading videos")
}

//v1
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


//v0
pub fn bucket_count(map: &HashMap<String, Vec<Video>>, username: &str) -> usize {
    map.get(username).map(|v| v.len()).unwrap_or(0)
}

//v0
pub fn append_videos(map: &mut HashMap<String, Vec<Video>>,username: &str,vids:&[Video]) -> usize {
    let user_vids = map.entry(username.to_string()).or_default();
    let mut existing_ids: HashSet<i64> = user_vids.iter().map(|v| v.video_id).collect();
    let mut added = 0;
    for vid in vids {
        if existing_ids.insert(vid.video_id) {
            user_vids.push(vid.clone());
            added += 1;
        }
    }
    added
}

//v0
pub fn update_download_status( map: &mut HashMap<String, Vec<Video>>, username: &str, video_id: i64, status: DownloadStatus) -> bool {
    let Some(vids) = map.get_mut(username) else { return false;};
    let Some(v) = vids.iter_mut().find(|v| v.video_id == video_id) else {return false;};
    v.download_status = status;
    match status {
        DownloadStatus::Downloaded => {
            v.download_date = Some(Local::now().naive_local());
        }
        DownloadStatus::NotDownloaded => {
            v.download_date = None;
        }
        DownloadStatus::DownloadFailed => {}
    }
    true
}

//v0
pub fn update_source_available(seen_vids: &mut HashMap<String, Vec<Video>>, username: &str, video_id: i64, source_available: bool) -> bool {
    let Some(vids) = seen_vids.get_mut(username) else { return false; };
    let Some(v) = vids.iter_mut().find(|v| v.video_id == video_id) else { return false; };
    v.source_available = source_available;
    true
}
//v0
pub fn pending_videos(seen_vids: &HashMap<String, Vec<Video>>) -> Vec<Video> {
    seen_vids.iter()
        .flat_map(|(bucket, videos)| {
            videos.iter().filter_map(move |vid| {
                if vid.source_available && (vid.download_status == DownloadStatus::NotDownloaded
                        || vid.download_status == DownloadStatus::DownloadFailed) {
                    let mut out = vid.clone();
                    if bucket == "favorite" {
                        out.is_fav = true;
                    }
                    Some(out)
                } else {
                    None
                }
            })}).collect()
}


