//v1
use anyhow::{anyhow, Result};
use std::{collections::HashMap, fs, process::Command};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{path::PathBuf, time::Instant};
use crate::browser::{load_cookie_params, write_ytdlp_cookie_jar};
use crate::db::{ logger::Log, resolve_executable_path, video::{Video}};
use crate::db::video::DownloadStatus::{Downloaded};
use crate::db::video::save_all;

//v1
fn download_videos(mut vids: Vec<&mut Video>) -> Result<()> {
    let len = vids.len();
    for (index, vid) in vids.iter_mut().enumerate() {
        let t0 = Instant::now();
        Log::console(format!(
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
                Log::console(format!(
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
                Log::console(format!(
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
        Log::console("[download] no pending videos".to_string());
        return Ok(());
    }
    Log::console(format!("[download] {} pending video(s)", pending.len()));
    download_videos(pending)?;
    Log::console("[download] saving database after downloads".to_string());
    save_all(&seen_vids)?;
    Log::console("[download] database saved".to_string());
    Ok(())
}

//v1
pub fn download_video(vid: &Video) -> Result<()> {
    let file_path = vid.file_path()?;
    Log::dev(format!("is_fav: {} Path: {:?}", vid.is_fav,file_path));

    match download_pre_check(&file_path, &vid.other_file_path()?) {
        Idk::Download => {
            Log::console(format!("[download] video {}: running yt-dlp", vid.id));
            if let Some(parent) = vid.file_path()?.parent() {
                fs::create_dir_all(parent)?;
            }
            yt_dlp(vid)
        }
        Idk::AlreadyDownloaded => {
            Log::info(format!("Video:{} Already Downloaded",vid.id));
            Ok(())}
        Idk::HardLink => {
            Log::info(format!("hard link created for {}", vid.id));
            Ok(fs::hard_link( &vid.other_file_path()?, &vid.file_path()?)?)
        }
    }

}
#[derive(PartialEq,Debug)]
enum Idk{
    Download,
    AlreadyDownloaded,
    HardLink
}
fn download_pre_check(file_path:&PathBuf, other_file:&PathBuf)->Idk{
    if file_path.exists(){
        return Idk::AlreadyDownloaded
    }
    if other_file.exists(){
        return Idk::HardLink
    }
    Idk::Download
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
        Log::console(format!("Video: {} Downloaded", vid.id));
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);

        Err(anyhow!(format!("yt-dlp: {}", stderr.trim())))
    }
}

#[cfg(test)]
mod test_download{
    use super::*;
    // #[test]
    fn test_download_video(){
        // vid is normal
        //video is not there -> download
        //video is in other -> hard_link
        //video is already there -> do nothing
        // vid is fav
        //video is not there -> download
        //video is in other -> hard_link
        //video is already there -> do nothing

        // normal id:1 not there -> download
        //  normal id:1 already there -> return
        //  fav id:1 other -> hard_link
        //  fav id:1 already there -> return
        //  fav id:2 not there, ->
        //  fav id:2 already there ->return,
        //  normal id:2 other
    }
    #[test]
    fn test_yt_download(){

        let username= "".to_string();
            let id=  0;
            let video= Video::new(format!("https://www.tiktok.com/@{}/video/{}", username, id), id,username );
            assert_eq!( yt_dlp(&video).is_ok(), true);
            assert_eq!(video.file_path().unwrap().exists(), true);
        }
}