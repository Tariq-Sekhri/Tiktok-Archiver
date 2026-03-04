use std::path::PathBuf;
use std::process::Command;
use anyhow::anyhow;
use crate::db::browser::load_cookie_params;
use crate::db::seen_video::{load_all_seen_videos,  update_download_status, DownloadStatus, SeenVideo};
use anyhow::Result;
use crate::db::config::load_config;
use crate::db::logger::{log, Event, LogLevel};
use crate::db::seen_video::DownloadStatus::DownloadFailed;

pub const VIDEO_EXT: &str = "mp4";

fn video_file_path(username: &str, video_id: i64) -> Result<PathBuf> {
    let mut p = PathBuf::from(load_config()?.download_dir);
    p.push(username);
    p.push(format!("{}.{}", video_id, VIDEO_EXT));
    Ok(p)
}
fn have_video_on_disk(vid: &SeenVideo) -> Result<bool> {
    Ok(video_file_path(&vid.username, vid.video_id)?.exists())
}


fn download_videos(vids:Vec<SeenVideo>)->Result<()>{
    for vid in vids {
        if have_video_on_disk(&vid).unwrap_or(false) {
            println!("had {} on disk", vid.video_id);
            update_download_status(&vid.username, vid.video_id, DownloadStatus::Downloaded)?;
            continue;
        }
        if !vid.source_available || vid.download_status == DownloadStatus::Downloaded {
            println!("Video Unavailable:{}", vid.video_id);
            continue;
        }

        println!("Downloding:{}", vid.video_id);
        if let Err(e) = download_video(&vid) {
            let msg = format!("Error Downloading vid:{:?}:({})", vid, e);
            update_download_status(&vid.username, vid.video_id, DownloadStatus::DownloadFailed)?;
            eprintln!("{}", msg);
            log(Event::new(msg, LogLevel::Error));
            continue;
        };

        update_download_status(&vid.username, vid.video_id, DownloadStatus::Downloaded)?;
    }
    Ok(())
}

pub fn download_pending()->Result<()>{
    let vids:Vec<SeenVideo> = load_all_seen_videos()?.into_values().flatten().filter(|vid| vid.download_status == DownloadStatus::NotDownloaded || vid.download_status== DownloadFailed).collect();
    download_videos(vids)?;
    Ok(())
}

fn download_video(vid: &SeenVideo) -> Result<()> {
    let path = video_file_path(&vid.username, vid.video_id)?;
    let cookie_params = load_cookie_params()?;
    let cookie_header = cookie_params
        .iter()
        .map(|c| format!("{}={}", c.name, c.value))
        .collect::<Vec<_>>()
        .join("; ");
    let mut cmd = Command::new(load_config()?.python_path);
    cmd.arg("-m")
        .arg("yt_dlp")
        .arg("-o")
        .arg(path.to_str().unwrap_or(""))
        .arg("--merge-output-format")
        .arg("mp4")
        .arg("--no-warnings");
    cmd.arg("--add-header")
        .arg(format!("Cookie:{}", cookie_header));
    cmd.arg(&vid.url);

    let output = cmd
        .output()
        .map_err(|e| anyhow!(format!("Failed to execute python: {}", e)))?;

    if output.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(anyhow!(format!("yt-dlp: {}", err)))
    }
}

