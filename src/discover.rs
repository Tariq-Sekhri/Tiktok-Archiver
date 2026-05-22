use std::collections::{HashMap, HashSet};
use std::{fs, io};
use crate::browser::{clear_tiktok_profile, cookie_params_have_session, cookie_to_param, is_headless, launch_browser, load_cookie_params, save_cookies, scroll_to_bottom, scroll_x_times, BrowserSession, TIKTOK_ORIGIN};
use crate::db::account::Account;
use crate::db::logger::Log;
use crate::db::video::{update_download_status, DownloadStatus};
use crate::download::{link_fav_video, video_on_disk, VIDEO_EXT};

use anyhow::{anyhow, Context, Result};
use headless_chrome::protocol::cdp::Runtime::RemoteObject;
use headless_chrome::Tab;
use regex::Regex;
use serde_json::Value;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use crate::db::video::Video;

const PROFILE_WAIT_TIMEOUT: Duration = Duration::from_secs(60);
const EVAL_TIMEOUT: Duration = Duration::from_secs(12);
const ACCOUNT_FETCH_TIMEOUT: Duration = Duration::from_secs(90);
const POLL_FETCH_TIMEOUT: Duration = Duration::from_secs(600);
const HTTP_COUNT_TIMEOUT: Duration = Duration::from_secs(25);

const VIDEO_COUNT_JS: &str = r#"
(function() {
  const el = document.getElementById('__UNIVERSAL_DATA_FOR_REHYDRATION__');
  if (el && el.textContent) {
    try {
      const data = JSON.parse(el.textContent);
      const detail = data && data['__DEFAULT_SCOPE__'] && data['__DEFAULT_SCOPE__']['webapp.user-detail'];
      if (detail && detail.userInfo && detail.userInfo.stats) {
        const n = detail.userInfo.stats.videoCount;
        if (typeof n === 'number') return n;
      }
    } catch (e) {}
  }
  const html = document.documentElement && document.documentElement.innerHTML;
  if (html) {
    const m = html.match(/"videoCount"\s*:\s*(\d+)/);
    if (m) return parseInt(m[1], 10);
  }
  return null;
})()
"#;

//v1
pub async fn first_discovery(username:String) -> Result<(Account, Vec<Video>)> {
    Log::dev(format!(
        "[discover] first_discovery @{}",
        username
    ));
    let session = launch_browser(&format!("https://www.tiktok.com/@{}", &username), is_headless())?;
    let result = (|| {
        scroll_to_bottom(&session)?;
        let html = session.tab().get_content().context("get_content")?;
        let new_vids = videos_from_anchor_links(&html)?;
        Log::dev(format!(
            "[discover] first_discovery @{} anchor links={}",
            username,
            new_vids.len()
        ));

        if new_vids.is_empty() {
            return Err(anyhow::anyhow!("No new video"));
        }

        let count = video_count_from_html(&html)?;
        let diff = count - new_vids.len() as i64;
        Log::dev(format!(
            "[discover] first_discovery @{} videoCount={} diff={}",
            username,
            count,
            diff
        ));

        Ok((Account::new(username.to_string(), count, diff), new_vids))
    })();
    result
}

//v0
pub async fn login() -> Result<()> {
    let cookies = load_cookie_params()?;
    if !cookies.is_empty() {
        println!("We Already have Cookies");
        println!("continuing will wipe current cookies");
        println!("Press Enter To Continue:");
        let mut asd = String::new();
        io::stdin().read_line(&mut asd)?;
        clear_tiktok_profile()?;
    }

    let session = launch_browser(
        "https://www.tiktok.com/login/qrcode",
        false,
    )?;
    println!("Log in in the browser window, then wait until you see your feed.");
    println!("Press Enter here to save your cookies.");
    let mut asd = String::new();
    io::stdin().read_line(&mut asd)?;

    session
        .tab()
        .navigate_to(TIKTOK_ORIGIN)
        .context("navigate to tiktok.com before saving cookies")?;
    session
        .tab()
        .wait_until_navigated()
        .context("timed out waiting for tiktok.com after login")?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let cookies = cookie_to_param(session.tab().get_cookies().context("get_cookies")?);
    if cookies.is_empty() {
        return Err(anyhow::anyhow!(
            "no tiktok cookies found in browser — finish logging in and try again"
        ));
    }
    if !cookie_params_have_session(&cookies) {
        return Err(anyhow::anyhow!(
            "session cookies missing — make sure you are fully logged in before pressing Enter"
        ));
    }
    save_cookies(&cookies)?;
    let path = crate::browser::cookies_path()?;
    println!("Saved {} TikTok cookies to {}", cookies.len(), path);
    println!("You can now run `cargo run` to start the default watcher.");
    Ok(())
}
pub fn fetch_newest_videos_sync(account: &Account) -> Result<Vec<Video>> {
    let url = format!("https://www.tiktok.com/@{}", account.name);
    Log::dev(format!(
        "[discover] fetch_newest_videos @{} url={} wait_after_load_s={}",
        account.name,
        url,
        2
    ));
    let session = launch_browser(&url, is_headless())?;
    std::thread::sleep(Duration::from_secs(2));
    let vids = videos_from_anchor_links(&session.tab().get_content().context("get_content")?)?;
    Log::dev(format!(
        "[discover] fetch_newest_videos @{} parsed {} anchor links",
        account.name,
        vids.len()
    ));
    Ok(vids)
}
pub async fn fetch_newest_videos(account: &Account) -> Result<Vec<Video>> {
    let account = account.clone();
    match tokio::task::spawn_blocking(move || fetch_newest_videos_sync(&account)).await {
        Ok(r) => r,
        Err(e) => Err(anyhow::anyhow!("fetch_newest_videos task failed: {}", e)),
    }
}

//v1
pub fn fav_scroll_budget(pass: u32) -> u32 {
    const CAP: u32 = 800;
    match pass {
        1 => 3,
        2 => 12,
        p => {
            let exp = (p - 3) as i32;
            let v = 30.0 * 3.0_f64.powi(exp);
            (v.round() as u32).clamp(30, CAP)
        }
    }
}

pub fn fav_with_seen(session: &BrowserSession, seen: &mut HashMap<String, Vec<Video>>, download_dir: &str, ) -> anyhow::Result<bool> {
    let fav_cycle_t0 = Instant::now();
    let mut seen_dirty = false;
    let mut done_ids: HashSet<i64> = HashSet::new();
    let mut existing_fav_ids: HashSet<i64> = seen
        .get("favorite")
        .map(|v| v.iter().map(|x| x.video_id).collect())
        .unwrap_or_default();
    let mut pass = 0u32;
    loop {
        pass += 1;
        Log::console(format!("fav pass {}", pass));
        Log::dev(format!("[fav] pass {}: reading page", pass));
        let read_t0 = Instant::now();
        let html = session.tab().get_content().context("get_content")?;
        let fav_vids: Vec<Video> = videos_from_anchor_links(&html)?
            .into_iter()
            .filter(|vid| !done_ids.contains(&vid.video_id))
            .collect();
        Log::dev_timing("fav_pass_read", read_t0);
        Log::console(format!("fav pass {} links {}", pass, fav_vids.len()));
        Log::dev(format!(
            "[fav] pass {}: found {} videos on page",
            pass,
            fav_vids.len()
        ));

        let process_t0 = Instant::now();
        let mut new_count = 0u32;
        let mut batch_new: Vec<Video> = Vec::new();
        let mut mark_downloaded: Vec<i64> = Vec::new();
        for fav in &fav_vids {
            done_ids.insert(fav.video_id);

            if existing_fav_ids.contains(&fav.video_id) {
                continue;
            }

            new_count += 1;
            let mut fav_video = fav.clone();
            fav_video.is_fav = true;

            if video_on_disk(&fav.username, fav.video_id)? {
                let fav_path = format!("{}/favs/{}.{}", download_dir, fav.video_id, VIDEO_EXT);
                if !fs::exists(&fav_path)? {
                    Log::dev(format!(
                        "[fav] hard_link @{} id={}",
                        fav.username, fav.video_id
                    ));
                    link_fav_video(fav)?;
                }
                mark_downloaded.push(fav.video_id);
            } else {
                Log::dev(format!(
                    "[fav] new favorite @{} id={}",
                    fav.username, fav.video_id
                ));
            }

            batch_new.push(fav_video);
            existing_fav_ids.insert(fav.video_id);
        }

        if !batch_new.is_empty() {
            let favorite_videos = seen.entry("favorite".to_string()).or_default();
            for v in batch_new {
                favorite_videos.push(v);
            }
            seen_dirty = true;
        }
        for video_id in mark_downloaded {
            if update_download_status(
                seen,
                "favorite",
                video_id,
                DownloadStatus::Downloaded,
            ) {
                seen_dirty = true;
            }
        }
        Log::dev_timing("fav_pass_process", process_t0);

        if new_count > 0 {
            Log::console(format!("fav pass {} new {}", pass, new_count));
        }
        Log::dev(format!(
            "[fav] pass {}: processed new={}",
            pass, new_count
        ));

        if new_count == 0 {
            Log::console(format!("fav done {}", pass));
            Log::dev(format!("[fav] pass {}: no new items, done", pass));
            break;
        }

        let budget = fav_scroll_budget(pass);
        Log::console(format!("fav pass {} scroll {}", pass, budget));
        Log::dev(format!("[fav] pass {}: scroll_x_times {}", pass, budget));
        let scroll_t0 = Instant::now();
        scroll_x_times(budget, session)?;
        Log::dev_timing("fav_pass_scroll", scroll_t0);
    }

    Log::dev(format!("[fav] finished after {} passes", pass));
    Log::dev_timing("fav_cycle", fav_cycle_t0);
    Ok(seen_dirty)
}



pub fn videos_from_anchor_links(html: &str) -> Result<Vec<Video>> {
    let re = Regex::new(r#"/@([\w.]+)/video/(\d+)"#)?;
    let mut for_ret: Vec<Video> = Vec::new();
    let junk_ids: [i64; 4] = [
        7511413375285447958,
        7074556216227155202,
        7035790010829769990,
        7008644040782613765,
    ];
    for cap in re.captures_iter(html) {
        let username = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let id: i64 = cap
            .get(2)
            .ok_or_else(|| anyhow!("missing video id"))?
            .as_str()
            .parse()
            .context("parse video id")?;
        if junk_ids.contains(&id) {
            continue;
        }
        for_ret.push(Video::new(
            format!("https://www.tiktok.com/@{}/video/{}", username, id),
            id,
            username.to_string(),
        ));
    }

    Ok(for_ret)
}





fn parse_rehydration(html: &str) -> Option<Value> {
    let re = Regex::new(
        r#"(?s)<script[^>]*id=["']__UNIVERSAL_DATA_FOR_REHYDRATION__["'][^>]*>([^<]+)</script>"#,
    )
        .ok()?;

    let caps = re.captures(html)?;
    let json_str = caps.get(1)?.as_str().trim();

    serde_json::from_str(json_str).ok()
}

fn video_count_fallback(html: &str) -> Option<i64> {
    let re = Regex::new(r#""videoCount"\s*:\s*(\d+)"#).ok()?;
    let caps = re.captures(html)?;
    caps.get(1)?.as_str().parse().ok()
}

pub fn video_count_from_html(html: &str) -> Result<i64> {
    if let Some(data) = parse_rehydration(html) {
        if let Some(v) = data
            .pointer("/__DEFAULT_SCOPE__/webapp.user-detail/userInfo/stats/videoCount")
            .and_then(|n| n.as_i64())
        {
            return Ok(v);
        }
    }
    if let Some(v) = video_count_fallback(html) {
        return Ok(v);
    }
    Err(anyhow!("videoCount not found in page"))
}

fn parse_eval_count(obj: RemoteObject) -> Result<i64> {
    let v = obj
        .value
        .ok_or_else(|| anyhow!("videoCount evaluate returned no value"))?;
    if let Some(n) = v.as_i64() {
        return Ok(n);
    }
    if let Some(n) = v.as_f64() {
        return Ok(n as i64);
    }
    if v.is_null() {
        return Err(anyhow!("videoCount not in page yet"));
    }
    Err(anyhow!("unexpected videoCount value: {}", v))
}

fn evaluate_with_timeout(tab: &Arc<Tab>, expression: &str, timeout: Duration) -> Result<RemoteObject> {
    let tab = Arc::clone(tab);
    let expr = expression.to_string();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(tab.evaluate(&expr, false));
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(obj)) => Ok(obj),
        Ok(Err(e)) => Err(e).context("evaluate"),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(anyhow!("evaluate timed out after {}s", timeout.as_secs())),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(anyhow!("evaluate worker disconnected")),
    }
}

pub fn video_count_from_tab(tab: &Arc<Tab>) -> Result<i64> {
    let obj = evaluate_with_timeout(tab, VIDEO_COUNT_JS, EVAL_TIMEOUT)?;
    parse_eval_count(obj)
}

fn wait_for_video_count(session: &BrowserSession, username: &str) -> Result<i64> {
    let deadline = Instant::now() + PROFILE_WAIT_TIMEOUT;
    let mut last_dev_log = Instant::now();
    loop {
        match video_count_from_tab(session.tab()) {
            Ok(n) => return Ok(n),
            Err(e) => {
                if Instant::now().duration_since(last_dev_log) >= Duration::from_secs(10) {
                    Log::dev(format!("@{} waiting for videoCount: {}", username, e));
                    last_dev_log = Instant::now();
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "@{} videoCount not ready after {}s",
                username,
                PROFILE_WAIT_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

pub fn try_get_count_http(username: &str) -> Result<i64> {
    let t0 = Instant::now();
    let params = load_cookie_params()?;
    if params.is_empty() {
        return Err(anyhow!("no cookies for http count"));
    }
    let url = format!("https://www.tiktok.com/@{}", username);
    let cookie_header: String = params
        .iter()
        .map(|p| format!("{}={}", p.name, p.value))
        .collect::<Vec<_>>()
        .join("; ");
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_COUNT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .context("http client")?;
    let resp = client
        .get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .header("Cookie", cookie_header)
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .context("http profile fetch")?;
    if !resp.status().is_success() {
        return Err(anyhow!("http status {}", resp.status()));
    }
    let html = resp.text().context("http body")?;
    let count = video_count_from_html(&html)?;
    Log::dev_timing("http_count", t0);
    Log::dev(format!(
        "[api] get_new_count @{} videoCount={} (http)",
        username,
        count
    ));
    Ok(count)
}

pub fn get_new_count_with_session(session: &BrowserSession, username: &str) -> Result<i64> {
    if let Ok(n) = try_get_count_http(username) {
        return Ok(n);
    }
    let t0 = Instant::now();
    let url = format!("https://www.tiktok.com/@{}", username);
    Log::dev(format!(
        "[api] get_new_count @{} url={} (browser tab)",
        username, url
    ));
    let count = wait_for_video_count(session, username)?;
    Log::dev_timing("browser_count", t0);
    Log::dev(format!(
        "[api] get_new_count @{} parsed videoCount={}",
        username, count
    ));
    Ok(count)
}


pub fn fetch_counts(
    session: &BrowserSession,
    usernames: &[String],
) -> Result<Vec<(String, Result<i64>)>> {
    let t0 = Instant::now();
    let mut out = Vec::with_capacity(usernames.len());
    for username in usernames {
        let result = get_new_count_with_session(session, username);
        out.push((username.clone(), result));
    }
    Log::dev_timing("poll_counts", t0);
    Ok(out)
}




