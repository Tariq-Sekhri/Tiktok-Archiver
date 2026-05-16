use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
};

use anyhow::{Context, Result};
use chrono::{Local, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::db::{atomic_write_text, ensure_file, state_dir};

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy)]
pub enum DownloadStatus {
    Downloaded,
    NotDownloaded,
    DownloadFailed,
}

pub fn serialize_download_date<S>(
    opt: &Option<NaiveDateTime>,
    s: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match opt {
        Some(dt) => s.serialize_str(&dt.format("%Y-%m-%d %I:%M:%S %p").to_string()),
        None => s.serialize_none(),
    }
}

pub fn deserialize_download_date<'de, D>(
    d: D,
) -> std::result::Result<Option<NaiveDateTime>, D::Error>
where
    D: Deserializer<'de>,
{
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

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Video {
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

impl Video{
    pub fn new(url:String, video_id:i64, username:String)->Self{
        Self{
            url,
            video_id,
            username,
            download_status: DownloadStatus::NotDownloaded,
            source_available: true,
            download_date: None,
        }
    }
}

pub trait VideoStore {
    const FILE_NAME: &'static str;
}

pub struct SeenVideos;
pub struct FavVideos;

impl VideoStore for SeenVideos {
    const FILE_NAME: &'static str = "seen_videos.json";
}

impl VideoStore for FavVideos {
    const FILE_NAME: &'static str = "fav_videos.json";
}

pub fn videos_file<S: VideoStore>() -> Result<PathBuf> {
    let path = state_dir().join(S::FILE_NAME);
    ensure_file(&path, "{}\n")?;
    Ok(path)
}

pub fn load_all<S: VideoStore>() -> Result<HashMap<String, Vec<Video>>> {
    let path = videos_file::<S>()?;
    let file = fs::File::open(path)?;
    serde_json::from_reader(file).context("Error loading videos")
}

pub fn save_all<S: VideoStore>(map: &HashMap<String, Vec<Video>>) -> Result<()> {
    let path = videos_file::<S>()?;
    let json = serde_json::to_string_pretty(map)?;
    atomic_write_text(&path, &json)?;
    Ok(())
}

pub fn append_videos<S: VideoStore>(username: &str, vids: &[Video]) -> Result<()> {
    let mut map = load_all::<S>()?;
    let user_vids = map.entry(username.to_string()).or_default();

    let mut existing_ids: HashSet<i64> =
        user_vids.iter().map(|v| v.video_id).collect();

    for vid in vids {
        if existing_ids.insert(vid.video_id) {
            user_vids.push(vid.clone());
        }
    }

    save_all::<S>(&map)
}

pub fn update_download_status<S: VideoStore>(
    username: &str,
    video_id: i64,
    status: DownloadStatus,
) -> Result<()> {
    let mut map = load_all::<S>()?;

    if let Some(vids) = map.get_mut(username) {
        if let Some(v) = vids.iter_mut().find(|v| v.video_id == video_id) {
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
        }
    }

    save_all::<S>(&map)
}

pub fn update_source_available<S: VideoStore>(
    username: &str,
    video_id: i64,
    source_available: bool,
) -> Result<()> {
    let mut map = load_all::<S>()?;

    if let Some(vids) = map.get_mut(username) {
        if let Some(v) = vids.iter_mut().find(|v| v.video_id == video_id) {
            v.source_available = source_available;
        }
    }

    save_all::<S>(&map)
}

pub fn total_videos<S: VideoStore>() -> Result<HashMap<String, usize>> {
    let vids = load_all::<S>()?;

    Ok(vids
        .into_iter()
        .map(|(username, videos)| (username, videos.len()))
        .collect())
}