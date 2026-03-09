use std::collections::HashMap;
use std::fs;
use chrono::{Local, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use crate::db::{ensure_file, state_dir};
use anyhow::{Context, Result};

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy)]
pub enum DownloadStatus {
    Downloaded,
    NotDownloaded,
    DownloadFailed,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct SeenVideo {
    pub url: String,
    pub video_id: i64,
    pub username: String,
    pub download_status: DownloadStatus,
    pub source_available: bool,
    #[serde(
        serialize_with = "serialize_download_date",
        deserialize_with = "deserialize_download_date"
    )]
    pub download_date: Option<NaiveDateTime>,
}

impl SeenVideo {
    pub fn new(url: String, video_id: i64, username: String, download_status: DownloadStatus, source_available: bool) -> Self {
        SeenVideo {
            url,
            video_id,
            username,
            download_status,
            source_available,
            download_date: None,
        }
    }
}

pub fn seen_videos_file() -> Result<String> {
    let path = state_dir().join("seen_videos.json");
    ensure_file(&path, "{}\n")?;
    Ok(path.to_string_lossy().into_owned())
}
fn serialize_download_date<S>(opt: &Option<NaiveDateTime>, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match opt {
        Some(dt) => s.serialize_str(&dt.format("%Y-%m-%dT%H:%M:%S").to_string()),
        None => s.serialize_none(),
    }
}
fn deserialize_download_date<'de, D>(d: D) -> Result<Option<NaiveDateTime>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(d)?;
    match opt {
        None => Ok(None),
        Some(s) => {
            if s.len() == 10 {
                // "YYYY-MM-DD"
                NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                    .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
                    .map(Some)
                    .map_err(serde::de::Error::custom)
            } else {
                NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S")
                    .map(Some)
                    .map_err(serde::de::Error::custom)
            }
        }
    }
}






pub fn append_seen_videos(username: &str, vids: &Vec<SeenVideo>) ->Result<()>{
    let mut map = load_all_seen_videos()?;
    let user_vids = map.entry(username.to_string()).or_insert_with(Vec::new);

    let mut existing_ids: std::collections::HashSet<i64> =
        user_vids.iter().map(|v| v.video_id).collect();

    for vid in vids {
        if existing_ids.insert(vid.video_id) {
            user_vids.push(vid.clone());
        }
    }

    save_all_seen_videos(&map)
}


pub fn load_all_seen_videos() -> Result<HashMap<String, Vec<SeenVideo>>> {
    let path = seen_videos_file()?;
    let file = fs::File::open(&path)?;
    serde_json::from_reader(file).context("Error loading seen videos")
}

pub fn save_all_seen_videos(map: &HashMap<String, Vec<SeenVideo>>)->Result<()> {
    let path = seen_videos_file()?;
    let file = fs::File::create(&path)?;
    serde_json::to_writer_pretty(file, map)?;
    Ok(())
}

pub fn update_download_status(username: &str, video_id: i64, status: DownloadStatus) -> Result<()> {
    let mut map = load_all_seen_videos()?;
    if let Some(vids) = map.get_mut(username) {
        if let Some(v) = vids.iter_mut().find(|v| v.video_id == video_id) {
            v.download_status = status;
            match status {
                DownloadStatus::Downloaded => v.download_date = Some(Local::now().naive_local()),
                DownloadStatus::NotDownloaded => v.download_date = None,
                DownloadStatus::DownloadFailed => {}
            }
        }
    }
    save_all_seen_videos(&map)
}


pub fn total_seen_videos() -> Result<HashMap<String, usize>> {
    let vids: HashMap<String, Vec<SeenVideo>> = load_all_seen_videos()?;

    let totals = vids
        .into_iter()
        .map(|(username, videos)| (username, videos.len()))
        .collect();

    Ok(totals)
}