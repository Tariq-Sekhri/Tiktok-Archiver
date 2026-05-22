use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use anyhow::anyhow;
use crate::db::browser::{load_cookie_params, write_ytdlp_cookie_jar};
use anyhow::Result;
use crate::db::config::load_config;
use crate::db::logger::Log;
use crate::db::video::{
    load_all, pending_from_map, save_all, update_download_status_in_memory,
    update_source_available_in_memory, DownloadStatus, Video,
};

pub const VIDEO_EXT: &str = "mp4";

fn video_file_path(username: &str, video_id: i64) -> Result<PathBuf> {
    let mut p = PathBuf::from(load_config()?.download_dir);
    p.push(username);
    p.push(format!("{}.{}", video_id, VIDEO_EXT));
    Ok(p)
}

fn fav_file_path(video_id: i64) -> Result<PathBuf> {
    let mut p = PathBuf::from(load_config()?.download_dir);
    p.push("favs");
    p.push(format!("{}.{}", video_id, VIDEO_EXT));
    Ok(p)
}

pub fn link_fav_video(vid: &Video) -> Result<()> {
    let original = video_file_path(&vid.username, vid.video_id)?;
    let link = fav_file_path(vid.video_id)?;
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::hard_link(&original, &link)?;
    Log::info(format!("hard link created for {}", vid.video_id));
    Ok(())
}

pub fn video_on_disk(username: &str, video_id: i64) -> Result<bool> {
    Ok(video_file_path(username, video_id)?.exists())
}

fn have_video_on_disk(vid: &Video) -> Result<bool> {
    if video_file_path(&vid.username, vid.video_id)?.exists() {
        return Ok(true);
    }
    if vid.is_fav {
        return Ok(fav_file_path(vid.video_id)?.exists());
    }
    Ok(false)
}

fn download_videos_in_memory(
    map: &mut HashMap<String, Vec<Video>>,
    vids: &[Video],
) -> Result<bool> {
    let mut dirty = false;
    for vid in vids {
        let bucket = if vid.is_fav {
            "favorite".to_string()
        } else {
            vid.username.clone()
        };

        if have_video_on_disk(vid).unwrap_or(false) {
            Log::info(format!("had {} on disk", vid.video_id));
            if update_download_status_in_memory(map, &bucket, vid.video_id, DownloadStatus::Downloaded) {
                dirty = true;
            }
            continue;
        }
        if !vid.source_available {
            continue;
        }
        if vid.download_status == DownloadStatus::Downloaded {
            continue;
        }

        println!("Downloading:{}", vid.video_id);
        if let Err(e) = download_video(vid) {
            let raw = e.to_string();
            let (user_msg, mark_unavailable) = classify_download_failure(&raw, vid);
            if update_download_status_in_memory(map, &bucket, vid.video_id, DownloadStatus::DownloadFailed) {
                dirty = true;
            }
            if mark_unavailable
                && update_source_available_in_memory(map, &bucket, vid.video_id, false)
            {
                dirty = true;
            }
            Log::error(user_msg.clone());
            continue;
        }
        Log::info(format!("Downloaded vid:{:?}:", vid));
        if update_download_status_in_memory(map, &bucket, vid.video_id, DownloadStatus::Downloaded) {
            dirty = true;
        }
    }
    Ok(dirty)
}

fn is_age_restricted_error(error: &str) -> bool {
    let msg = error.to_ascii_lowercase();
    msg.contains("age-restricted")
        || msg.contains("age restricted")
        || msg.contains("this post is age-restricted")
}

fn age_restricted_user_message(vid: &Video) -> String {
    format!(
        "Video {} (@{}): Post unavailable — this post is age-restricted. TikTok blocks it on web without a logged-in account that can view mature content. URL: {}. Log in on tiktok.com on this PC (same account as your phone), confirm the video plays in the browser, then run `cargo run login` to refresh archiver cookies.",
        vid.video_id, vid.username, vid.url
    )
}

fn classify_download_failure(raw: &str, vid: &Video) -> (String, bool) {
    if is_age_restricted_error(raw) {
        return (age_restricted_user_message(vid), true);
    }
    let mark_unavailable = is_fatal_source_error(raw);
    let user_msg = if mark_unavailable {
        format!(
            "Video {} (@{}): source unavailable — {}",
            vid.video_id, vid.username, raw
        )
    } else {
        format!("Error downloading {:?}: {}", vid, raw)
    };
    (user_msg, mark_unavailable)
}

fn is_fatal_source_error(error: &str) -> bool {
    if is_age_restricted_error(error) {
        return true;
    }
    let msg = error.to_ascii_lowercase();
    msg.contains("your ip address is blocked from accessing this post")
        || msg.contains("video is unavailable")
        || msg.contains("this post is private")
        || msg.contains("this account is private")
        || msg.contains("not available")
        || msg.contains("video has been removed")
        || msg.contains("status code 404")
}

pub fn download_pending_with_map(map: &mut HashMap<String, Vec<Video>>) -> Result<()> {
    let vids = pending_from_map(map);
    if vids.is_empty() {
        return Ok(());
    }
    let dirty = download_videos_in_memory(map, &vids)?;
    if dirty {
        save_all(map)?;
    }
    Ok(())
}

pub fn download_pending_favorites_with_map(map: &mut HashMap<String, Vec<Video>>) -> Result<()> {
    let vids: Vec<Video> = map
        .get("favorite")
        .map(|videos| {
            videos
                .iter()
                .filter_map(|vid| {
                    if vid.source_available
                        && (vid.download_status == DownloadStatus::NotDownloaded
                            || vid.download_status == DownloadStatus::DownloadFailed)
                    {
                        let mut out = vid.clone();
                        out.is_fav = true;
                        Some(out)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    if vids.is_empty() {
        return Ok(());
    }
    let dirty = download_videos_in_memory(map, &vids)?;
    if dirty {
        save_all(map)?;
    }
    Ok(())
}

pub fn download_pending() -> Result<()> {
    let mut map = load_all()?;
    download_pending_with_map(&mut map)
}

fn resolve_executable_path(default_name: &str) -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("state").join(default_name);
            if candidate.exists() {
                return candidate;
            }
            let candidate = dir.join(default_name);
            if candidate.exists() {
                return candidate;
            }
        }
    }
    if cfg!(debug_assertions) {
        if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
            let candidate = PathBuf::from(manifest).join("state").join(default_name);
            if candidate.exists() {
                return candidate;
            }
        }
    }

    PathBuf::from(default_name)
}

pub fn download_video(vid: &Video) -> Result<()> {
    if !vid.source_available {
        return Err(anyhow!("source unavailable"));
    }
    let path = video_file_path(&vid.username, vid.video_id)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let cookie_params = load_cookie_params()?;
    let ytdlp_path = resolve_executable_path("yt-dlp.exe");
    let mut cmd = Command::new(&ytdlp_path);
    cmd.arg("-o")
        .arg(path.to_str().unwrap_or(""))
        .arg("--merge-output-format")
        .arg("mp4")
        .arg("--no-warnings");
    if !cookie_params.is_empty() {
        let jar = write_ytdlp_cookie_jar(&cookie_params)?;
        cmd.arg("--cookies").arg(jar);
    }
    cmd.arg(&vid.url);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);

    let output = cmd
        .output()
        .map_err(|e| anyhow!(format!("Failed to execute yt-dlp: {}", e)))?;

    if output.status.success() {
        if vid.is_fav {
            link_fav_video(vid)?;
        }
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{}\n{}", stderr, stdout);
        if is_age_restricted_error(&combined) {
            return Err(anyhow!(age_restricted_user_message(vid)));
        }
        Err(anyhow!(format!("yt-dlp: {}", stderr.trim())))
    }
}
