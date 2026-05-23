//v1

use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde_json::Value;
use std::{collections::{HashMap, HashSet}, io, time::{Duration, Instant}};
use crate::{browser::{clear_tiktok_profile, cookie_params_have_session, cookie_to_param, launch_browser, load_cookie_params, save_cookies, scroll_to_bottom, scroll_x_times, BrowserSession}, db::{account::Account, logger::Log, video::Video}};
use crate::browser::navigate_to_fav;
use crate::core::timeout;
use crate::db::logger::LogLevel;

//v1
pub async fn first_discovery(username:String, session:&BrowserSession) -> Result<(Account, Vec<Video>)> {
    Log::dev(format!(
        "[discover] first_discovery @{}",
        username
    ));
    session.tab().navigate_to(&format!("https://www.tiktok.com/@{}", &username))?;
    timeout(2, LogLevel::Console).await;
    let result = (|| {
        scroll_to_bottom(&session)?;
        let html = session.tab().get_content().context("get_content")?;
        let new_vids = videos_from_html(&html)?;
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

//v1
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
        .navigate_to("https://www.tiktok.com")
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
//v1
pub fn fetch_newest_videos(account: &Account, session:&BrowserSession) -> Result<Vec<Video>> {
    let url = format!("https://www.tiktok.com/@{}", account.name);
    Log::dev(format!(
        "[discover] fetch_newest_videos @{} url={} wait_after_load_s={}",
        account.name,
        url,
        2
    ));
    session.tab().navigate_to(&url)?;
    std::thread::sleep(Duration::from_secs(2));
    let vids = videos_from_html(&session.tab().get_content().context("get_content")?)?;
    Log::dev(format!(
        "[discover] fetch_newest_videos @{} parsed {} anchor links",
        account.name,
        vids.len()
    ));
    Ok(vids)
}


//v1
pub fn scrolls_per_pass(pass: u32) -> u32 {
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
//v1
pub fn fav(session: &BrowserSession, seen: &mut HashMap<String, Vec<Video>> ) -> Result<bool> {
    navigate_to_fav(&session)?;
    let mut have_new_vids = false;
    let mut existing_fav_ids: HashSet<i64> = seen
        .get("favorite")
        .map(|v| v.iter().map(|x| x.id).collect())
        .unwrap_or_default();
    let mut pass = 0u32;
    loop {
        pass += 1;
        let html = session.tab().get_content().context("get_content")?;
        let new_fav_vids: Vec<Video> = videos_from_html(&html)?.into_iter()
            .filter(|vid| !existing_fav_ids.contains(&vid.id)).collect();
        Log::dev(format!("[fav] pass {}: found {} videos on page", pass, new_fav_vids.len()));
        let mut new_count = 0u32;
        for fav in &new_fav_vids {
            if existing_fav_ids.contains(&fav.id) {
                continue;
            }
            new_count += 1;
            let mut fav_video = fav.clone();
            fav_video.is_fav = true;

            existing_fav_ids.insert(fav.id);

            let favorite_videos = seen.entry("favorite".to_string()).or_default();
                favorite_videos.push(fav.to_owned());
            have_new_vids = true;
        }


        if new_count == 0 {
            Log::dev(format!("[fav] pass {}: no new items, done", pass));
            break;
        }

        let scroll_amount = scrolls_per_pass(pass);
        Log::dev(format!("[fav] pass {}: scroll: {}", pass, scroll_amount));
        let scroll_t0 = Instant::now();
        scroll_x_times(scroll_amount, session)?;
        Log::dev_timing("fav_pass_scroll", scroll_t0);
    }
    Log::dev(format!("[fav] finished after {} passes", pass));
    Ok(have_new_vids)
}


//v1
pub fn videos_from_html(html: &str) -> Result<Vec<Video>> {
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


//v1
fn parse_rehydration(html: &str) -> Option<Value> {
    let re = Regex::new(
        r#"(?s)<script[^>]*id=["']__UNIVERSAL_DATA_FOR_REHYDRATION__["'][^>]*>([^<]+)</script>"#,
    ).ok()?;

    let caps = re.captures(html)?;
    let json_str = caps.get(1)?.as_str().trim();

    serde_json::from_str(json_str).ok()
}
//v1
fn video_count_fallback(html: &str) -> Option<i64> {
    let re = Regex::new(r#""videoCount"\s*:\s*(\d+)"#).ok()?;
    let caps = re.captures(html)?;
    caps.get(1)?.as_str().parse().ok()
}

//v1
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


//v1
fn fetch_count_http(username: &str) -> Result<i64> {
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
    let client = reqwest::Client::builder()
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
    Log::dev(format!(
        "[api] get_new_count @{} videoCount={} (http)",
        username,
        count
    ));
    Ok(count)
}

//v1
async pub fn get_new_count(session: &BrowserSession, username: &str) -> Result<i64> {
    if let Ok(n) = fetch_count_http(username) {
        return Ok(n);
    }
    session.tab().navigate_to(&format!("https://www.tiktok.com/@{}", &username))?;
    let html = session.tab().get_content()?;
    video_count_from_html(&html)
}





