use std::fs;
use std::path::PathBuf;
use std::process::Command;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use anyhow::anyhow;
use crate::db::browser::{load_cookie_params, write_ytdlp_cookie_jar};
use anyhow::Result;
use crate::db::config::load_config;
use crate::db::logger::{log, Event, LogLevel};
use crate::db::video::{load_all, update_download_status, update_source_available, DownloadStatus, Video};

pub const VIDEO_EXT: &str = "mp4";

fn video_file_path(username: &str, video_id: i64) -> Result<PathBuf> {
    let mut p = PathBuf::from(load_config()?.download_dir);
    p.push(username);
    p.push(format!("{}.{}", video_id, VIDEO_EXT));
    Ok(p)
}
fn have_video_on_disk(vid: &Video) -> Result<bool> {
    Ok(video_file_path(&vid.username, vid.video_id)?.exists())
}


fn download_videos(vids:&Vec<Video>)->Result<()>{
    for vid in vids {

        if have_video_on_disk(&vid).unwrap_or(false) {
            log(Event::new(format!("had {} on disk", vid.video_id), LogLevel::Info));
            update_download_status(&vid.username, vid.video_id, DownloadStatus::Downloaded)?;
            continue;
        }
        if !vid.source_available || vid.download_status == DownloadStatus::Downloaded {
            log(Event::new(format!("Video Unavailable:{}", vid.video_id), LogLevel::Info));
            continue;
        }

        println!("Downloading:{}", vid.video_id);
        if let Err(e) = download_video(&vid) {
            update_download_status(&vid.username, vid.video_id, DownloadStatus::DownloadFailed)?;
            if is_fatal_source_error(&e.to_string()) {
                update_source_available(&vid.username, vid.video_id, false)?;
            }
            log(Event::new(format!("Error Downloading vid:{:?}:({})", vid, e), LogLevel::Error));
            continue;
        };
        log(Event::new(format!("Downloaded vid:{:?}:", vid), LogLevel::Info));
        update_download_status(&vid.username, vid.video_id, DownloadStatus::Downloaded)?;
    }
    Ok(())
}

fn is_fatal_source_error(error: &str) -> bool {
    let msg = error.to_ascii_lowercase();
    msg.contains("your ip address is blocked from accessing this post")
        || msg.contains("video is unavailable")
        || msg.contains("this post is private")
        || msg.contains("this account is private")
        || msg.contains("not available")
        || msg.contains("video has been removed")
        || msg.contains("status code 404")
}

pub fn download_pending()->Result<()>{
    let vids:Vec<Video> = load_all()?.into_values().flatten().filter(|vid| vid.download_status == DownloadStatus::NotDownloaded || vid.download_status== DownloadStatus::DownloadFailed).collect();
    download_videos(&vids)?;
    for vid in vids{

    if vid.is_fav{
        let original = PathBuf::from(format!(
            "{}/{}/{}.{}",
            load_config()?.download_dir,
            vid.username,
            vid.video_id,
            VIDEO_EXT
        ));
        let link = PathBuf::from(format!(
            "{}/favs/{}.{}",
            load_config()?.download_dir,
            vid.video_id,
            VIDEO_EXT
        ));
        fs::hard_link(original, link)?;
    }
    }
    Ok(())
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
    let path = video_file_path(&vid.username, vid.video_id)?;
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
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(anyhow!(format!("yt-dlp: {}", err)))
    }
}
