use anyhow::{anyhow, Result};
use std::{collections::HashMap, fs, path::PathBuf, process::Command};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::time::Instant;
use crate::browser::{load_cookie_params, write_ytdlp_cookie_jar};
use crate::db::{
    logger::Log,
    resolve_executable_path,
    video::{Video, VIDEO_EXT},
};
use crate::db::video::DownloadStatus::{Downloaded};
use crate::db::video::save_all;

fn download_videos(seen_vids: &mut HashMap<String, Vec<Video>>) -> Result<()> {
    let queue: Vec<(String, usize)> = seen_vids
        .iter()
        .flat_map(|(user, vids)| {
            vids.iter()
                .enumerate()
                .filter(|(_, vid)| vid.is_pending())
                .map(|(index, _)| (user.clone(), index))
                .collect::<Vec<_>>()
        })
        .collect();
    let len = queue.len();
    for (index, (user, vid_index)) in queue.into_iter().enumerate() {
        let t0 = Instant::now();
        let snapshot = seen_vids.get(&user).unwrap()[vid_index].clone();
        Log::dev(format!(
            "[download] {}/{}: video {} (@{}) starting",
            index + 1,
            len,
            snapshot.id,
            snapshot.username
        ));
        match download_video(&snapshot) {
            Ok(()) => {
                let entry = &mut seen_vids.get_mut(&user).unwrap()[vid_index];
                entry.download_status = Downloaded;
                entry.download_date = Some(chrono::Local::now().naive_local());
                propagate_hardlinks_for_id(seen_vids, snapshot.id)?;
                Log::dev(format!(
                    "[download] {}/{}: video {} ok ({}ms)",
                    index + 1,
                    len,
                    snapshot.id,
                    t0.elapsed().as_millis()
                ));
            }
            Err(e) => {
                seen_vids
                    .get_mut(&user)
                    .unwrap()[vid_index]
                    .download_failed(&e);
                Log::error(format!("download {} failed: {:#}", snapshot.id, e));
                Log::dev(format!(
                    "[download] {}/{}: video {} failed ({}ms): {}",
                    index + 1,
                    len,
                    snapshot.id,
                    t0.elapsed().as_millis(),
                    e
                ));
            }
        }
    }
    Ok(())
}

pub fn download_pending(seen_vids: &mut HashMap<String, Vec<Video>>) -> Result<()> {
    propagate_hardlinks(seen_vids)?;
    let pending_count: usize = seen_vids
        .values()
        .flat_map(|vids| vids.iter())
        .filter(|vid| vid.is_pending())
        .count();
    if pending_count == 0 {
        Log::dev("[download] no pending videos".to_string());
        return Ok(());
    }
    Log::console(format!("{} video(s) to download", pending_count));
    Log::dev(format!("[download] {} pending video(s)", pending_count));
    download_videos(seen_vids)?;
    Log::dev("[download] saving database after downloads".to_string());
    save_all(seen_vids)?;
    Log::dev("[download] database saved".to_string());
    Ok(())
}

pub fn download_video(vid: &Video) -> Result<()> {
    let file_path = vid.file_path()?;
    Log::dev(format!("is_fav: {} Path: {:?}", vid.is_fav, file_path));

    if file_path.exists() {
        Log::info(format!("Video {} already on disk", vid.id));
        return Ok(());
    }
    if let Some(existing_path) = existing_video_path(vid)? {
        link_video_to(existing_path, file_path, vid.id)?;
        return Ok(());
    }
    Log::dev(format!("[download] video {}: running yt-dlp", vid.id));
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)?;
    }
    yt_dlp(vid)
}

fn link_video_to(source: PathBuf, target: PathBuf, id: i64) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    Log::info(format!(
        "Video {} linked from existing file: {}",
        id,
        source.display()
    ));
    fs::hard_link(source, target)?;
    Ok(())
}

fn find_existing_file_for_id(id: i64) -> Result<Option<PathBuf>> {
    let download_root = PathBuf::from(crate::db::config::load_config()?.download_dir);
    let file_name = format!("{id}.{VIDEO_EXT}");
    for entry in fs::read_dir(download_root)? {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        let candidate = path.join(&file_name);
        if candidate.is_file() {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

fn existing_video_path(vid: &Video) -> Result<Option<PathBuf>> {
    let target = vid.file_path()?;
    Ok(find_existing_file_for_id(vid.id)?.filter(|path| *path != target))
}

fn propagate_hardlinks(seen_vids: &mut HashMap<String, Vec<Video>>) -> Result<()> {
    let ids: Vec<i64> = seen_vids
        .values()
        .flat_map(|vids| vids.iter().filter(|vid| vid.is_pending()).map(|vid| vid.id))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    for id in ids {
        propagate_hardlinks_for_id(seen_vids, id)?;
    }
    Ok(())
}

fn propagate_hardlinks_for_id(seen_vids: &mut HashMap<String, Vec<Video>>, id: i64) -> Result<()> {
    let Some(source) = find_existing_file_for_id(id)? else {
        return Ok(());
    };
    for vids in seen_vids.values_mut() {
        for vid in vids.iter_mut() {
            if vid.id != id {
                continue;
            }
            let target = vid.file_path()?;
            if target.exists() {
                if vid.is_pending() {
                    vid.download_status = Downloaded;
                    vid.download_date = Some(chrono::Local::now().naive_local());
                }
                continue;
            }
            if !vid.is_pending() {
                continue;
            }
            link_video_to(source.clone(), target, id)?;
            vid.download_status = Downloaded;
            vid.download_date = Some(chrono::Local::now().naive_local());
        }
    }
    Ok(())
}

fn yt_dlp(vid: &Video) -> Result<()> {
    let cookie_params = load_cookie_params()?;
    let ytdlp_path = resolve_executable_path("yt-dlp.exe");
    let mut cmd = Command::new(&ytdlp_path);
    cmd.arg("-o")
        .arg(vid.file_path()?.to_str().unwrap_or(""))
        .arg("--merge-output-format")
        .arg("mp4")
        .arg("--extractor-retries")
        .arg("5")
        .arg("--retry-sleep")
        .arg("extractor:exp=1:8")
        .arg("--sleep-requests")
        .arg("1")
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
        Log::info(format!("Video {} downloaded", vid.id));
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);

        Err(anyhow!(format!("yt-dlp: {}", stderr.trim())))
    }
}

#[cfg(test)]
mod test_download {
    use super::*;
    #[test]
    fn test_yt_download() {
        let username = "".to_string();
        let id = 0;
        let video = Video::new(
            format!("https://www.tiktok.com/@{}/video/{}", username, id),
            id,
            username,
        );
        assert_eq!(yt_dlp(&video).is_ok(), true);
        assert_eq!(video.file_path().unwrap().exists(), true);
    }
}
