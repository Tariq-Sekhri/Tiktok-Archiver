//v1
use anyhow::{anyhow, Result};
use std::{collections::HashMap, fs, process::Command};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use crate::browser::{load_cookie_params, write_ytdlp_cookie_jar};
use crate::db::{ logger::Log, resolve_executable_path, video::{Video}};
use crate::db::video::DownloadStatus::{DownloadFailed, Downloaded};
//v1
fn download_videos(vids:Vec<&mut Video>, ) -> Result<()> {
    for vid in vids{
       match download_video(vid){
            Ok(()) => {
                vid.download_status = Downloaded;
                vid.download_date = Some(chrono::Utc::now().naive_utc());
            }
            Err(e) => {
                vid.download_status = DownloadFailed;
                Log::error(format!("Download {} Failed:{}",vid.id, e ))
            }
        }
    }
    Ok(())
}
//v1
pub fn download_pending(seen_vids: &mut HashMap<String, Vec<Video>>) -> Result<()> {

    let pending:Vec<&mut Video> = seen_vids.iter_mut().flat_map(|(_, vids)| vids.iter_mut().filter(|vid| vid.is_pending() )).collect();
    if pending.is_empty() {
        return Ok(());
    }
    download_videos(pending)?;

    Ok(())
}

//v1
pub fn download_video(vid: &Video) -> Result<()> {
    let file_path = vid.file_path()?;
    if file_path.exists(){
        Log::console(format!("Video:{} Already Downloaded",vid.id));
        return Ok(())

    }
    if !vid.file_path()?.exists() && vid.other_file_path()?.exists(){
        Log::info(format!("hard link created for {}", vid.id));
        fs::hard_link( &vid.other_file_path()?, &vid.file_path()?)?;
        return Ok(())

    }
    let path = vid.file_path()?;
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

        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);

        Err(anyhow!(format!("yt-dlp: {}", stderr.trim())))
    }
}
