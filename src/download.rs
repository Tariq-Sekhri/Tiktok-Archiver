//v1
use anyhow::{anyhow, Result};
use std::{collections::HashMap, fs, process::Command};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::time::Instant;
use crate::browser::{load_cookie_params, write_ytdlp_cookie_jar};
use crate::db::{ logger::Log, resolve_executable_path, video::{Video}};
use crate::db::video::DownloadStatus::{Downloaded};
use crate::db::video::save_all;

//v1
fn download_videos(mut vids: Vec<&mut Video>) -> Result<()> {
    let len = vids.len();
    for (index, vid) in vids.iter_mut().enumerate() {
        let t0 = Instant::now();
        Log::dev(format!(
            "[download] {}/{}: video {} (@{}) starting",
            index + 1,
            len,
            vid.id,
            vid.username
        ));
        match download_video(vid) {
            Ok(()) => {
                vid.download_status = Downloaded;
                vid.download_date = Some(chrono::Local::now().naive_local());
                Log::dev(format!(
                    "[download] {}/{}: video {} ok ({}ms)",
                    index + 1,
                    len,
                    vid.id,
                    t0.elapsed().as_millis()
                ));
            }
            Err(e) => {
                vid.download_failed(&e);
                Log::error(format!("Download {} Failed:{}", vid.id, e));
                Log::dev(format!(
                    "[download] {}/{}: video {} failed ({}ms): {}",
                    index + 1,
                    len,
                    vid.id,
                    t0.elapsed().as_millis(),
                    e
                ));
            }
        }
    }
    Ok(())
}
//v1
pub fn download_pending(seen_vids: &mut HashMap<String, Vec<Video>>) -> Result<()> {
    let pending: Vec<&mut Video> = seen_vids
        .iter_mut()
        .flat_map(|(_, vids)| vids.iter_mut().filter(|vid| vid.is_pending()))
        .collect();
    if pending.is_empty() {
        Log::dev("[download] no pending videos".to_string());
        return Ok(());
    }
    Log::console(format!("{} video(s) to download", pending.len()));
    Log::dev(format!("[download] {} pending video(s)", pending.len()));
    download_videos(pending)?;
    Log::dev("[download] saving database after downloads".to_string());
    save_all(&seen_vids)?;
    Log::dev("[download] database saved".to_string());
    Ok(())
}

//v1
pub fn download_video(vid: &Video) -> Result<()> {
    let file_path = vid.file_path()?;
    Log::dev(format!("is_fav: {} Path: {:?}", vid.is_fav,file_path));

    if file_path.exists() {
        Log::info(format!("Video {} already on disk", vid.id));
        return Ok(());
    }
    if vid.other_file_path()?.exists() {
        Log::info(format!("Video {} linked from existing file", vid.id));
        return Ok(fs::hard_link(&vid.other_file_path()?, &vid.file_path()?)?);
    }
    Log::dev(format!("[download] video {}: running yt-dlp", vid.id));
    if let Some(parent) = vid.file_path()?.parent() {
        fs::create_dir_all(parent)?;
    }
    yt_dlp(vid)
}

fn yt_dlp(vid: &Video)->Result<()>{
    let cookie_params = load_cookie_params()?;
    let ytdlp_path = resolve_executable_path("yt-dlp.exe");
    let mut cmd = Command::new(&ytdlp_path);
    cmd.arg("-o")
        .arg(vid.file_path()?.to_str().unwrap_or(""))
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
        Log::info(format!("Video {} downloaded", vid.id));
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);

        Err(anyhow!(format!("yt-dlp: {}", stderr.trim())))
    }
}

#[cfg(test)]
mod test_download{
    use super::*;
    #[test]
    fn test_yt_download(){

        let username= "".to_string();
            let id=  0;
            let video= Video::new(format!("https://www.tiktok.com/@{}/video/{}", username, id), id,username );
            assert_eq!( yt_dlp(&video).is_ok(), true);
            assert_eq!(video.file_path().unwrap().exists(), true);
        }
}