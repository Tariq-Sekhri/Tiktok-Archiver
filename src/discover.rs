use crate::browser::{launch_browser_without_cookies, navigate_to_fav};
use crate::core::timeout;
use crate::db::logger::LogLevel;
use crate::db::video::{append_videos, save_all};
use crate::{
    browser::{
        clear_tiktok_profile, click_refresh_if_present, cookie_params_have_session,
        cookie_to_param, load_cookie_params, save_cookies, scroll_x_times, BrowserSession,
    },
    db::{logger::Log, video::Video},
};
use anyhow::{anyhow, Context, Result};
use regex::Regex;
use std::{
    collections::{HashMap, HashSet},
    io,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

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

    let session = launch_browser_without_cookies("https://www.tiktok.com/login/qrcode", false)?;
    println!("Log in in the browser window, then wait until you see your feed.");
    println!("Press Enter here to save your cookies.");
    let mut asd = String::new();
    io::stdin().read_line(&mut asd)?;

    session
        .tab()?
        .navigate_to("https://www.tiktok.com")
        .context("navigate to tiktok.com before saving cookies")?;
    session
        .tab()?
        .wait_until_navigated()
        .context("timed out waiting for tiktok.com after login")?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let cookies = cookie_to_param(session.tab()?.get_cookies().context("get_cookies")?);
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

pub fn fetch_new_fav(
    session: &BrowserSession,
    seen: &mut HashMap<String, Vec<Video>>,
) -> Result<()> {
    let t0 = Instant::now();
    Log::dev("[fav] navigating to favorites".to_string());
    navigate_to_fav(&session)?;
    Log::dev(format!(
        "[fav] navigation done ({}ms)",
        t0.elapsed().as_millis()
    ));
    let mut existing_fav_ids: HashSet<i64> = seen
        .get("favorite")
        .map(|v| v.iter().map(|x| x.id).collect())
        .unwrap_or_default();
    Log::dev(format!(
        "[fav] scrolling ({} known favorites)",
        existing_fav_ids.len()
    ));
    let scroll_start = Instant::now();
    let new_vids = scroll_while_new_video(session, &mut existing_fav_ids, true)?;
    Log::dev(format!(
        "[fav] scroll done ({}ms)",
        scroll_start.elapsed().as_millis()
    ));
    if !new_vids.is_empty() {
        Log::info(format!("{} new favorite(s)", new_vids.len()));
    }
    let should_save = !new_vids.is_empty();
    let fav = seen.entry("favorite".to_string()).or_default();
    for vid in new_vids {
        fav.push(vid.to_owned());
    }
    if should_save {
        Log::dev("[fav] saving database".to_string());
        save_all(seen)?;
        Log::dev("[fav] database saved".to_string());
    }
    Ok(())
}

pub async fn fetch_new_videos(
    username: &str,
    session: &BrowserSession,
    seen: &mut HashMap<String, Vec<Video>>,
) -> Result<()> {
    let profile_url = format!("https://www.tiktok.com/@{username}?lang=en");
    Log::dev(format!("[@{username}] navigating to {profile_url}"));
    let t0 = Instant::now();
    session.tab()?.navigate_to(profile_url.as_str())?;
    Log::dev(format!(
        "[@{username}] navigate sent ({}ms), waiting",
        t0.elapsed().as_millis()
    ));
    timeout(5, LogLevel::Console).await;
    Log::dev(format!("[@{username}] checking for refresh button"));
    click_refresh_if_present(&session)?;
    let known = seen.get(username).map(|vids| vids.len()).unwrap_or(0);
    Log::dev(format!(
        "[@{username}] scrolling profile ({} known videos)",
        known
    ));
    let mut existing_ids = seen
        .get(username)
        .map(|vids| vids.iter().map(|vid| vid.id).collect())
        .unwrap_or_default();
    let scroll_start = Instant::now();
    let new_vids = scroll_while_new_video(&session, &mut existing_ids, false)?;
    Log::dev(format!(
        "[@{username}] scroll done ({}ms)",
        scroll_start.elapsed().as_millis()
    ));
    if !new_vids.is_empty() {
        Log::info(format!("@{username}: {} new video(s)", new_vids.len()));
    }
    let should_save = !new_vids.is_empty();
    append_videos(seen, username, &new_vids);
    if should_save {
        Log::dev(format!(
            "[@{username}] saving {} new to database",
            new_vids.len()
        ));
        save_all(seen)?;
        Log::dev(format!("[@{username}] database saved"));
    }
    Ok(())
}

pub fn scroll_while_new_video(
    session: &BrowserSession,
    existing_ids: &mut HashSet<i64>,
    is_fav: bool,
) -> Result<Vec<Video>> {
    let label = if is_fav { "fav" } else { "profile" };
    let mut pass = 0u32;
    let mut new_vids: Vec<Video> = vec![];
    loop {
        pass += 1;
        let pass_start = Instant::now();
        Log::dev(format!("[scroll/{label}] pass {pass}: reading page HTML"));
        let html = get_content_with_timeout(session, Duration::from_secs(5))
            .context("get_content with timeout")?;
        let found_vids: Vec<Video> = videos_from_html(&html)?
            .into_iter()
            .filter(|vid| !existing_ids.contains(&vid.id))
            .collect();
        let mut new_count = 0u32;
        for vid in &found_vids {
            if existing_ids.contains(&vid.id) {
                continue;
            }
            new_count += 1;
            let mut vid = vid.clone();
            vid.is_fav = is_fav;
            existing_ids.insert(vid.id);
            new_vids.push(vid.to_owned());
        }
        if new_count == 0 {
            Log::dev(format!(
                "[scroll/{label}] pass {pass}: no new items, done ({}ms, {} total new)",
                pass_start.elapsed().as_millis(),
                new_vids.len()
            ));
            break;
        }
        let scroll_amount = scrolls_per_pass(pass);
        Log::dev(format!(
            "[scroll/{label}] pass {pass}: {new_count} new, scrolling {scroll_amount}x ({}ms parse)",
            pass_start.elapsed().as_millis()
        ));
        let scroll_start = Instant::now();
        scroll_x_times(scroll_amount, session)?;
        Log::dev(format!(
            "[scroll/{label}] pass {pass}: scroll finished ({}ms)",
            scroll_start.elapsed().as_millis()
        ));
    }
    Ok(new_vids)
}

fn get_content_with_timeout(session: &BrowserSession, timeout: Duration) -> Result<String> {
    let tab = session.tab()?.clone();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = tab.get_content().context("get_content");
        let _ = tx.send(result);
    });
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(anyhow!(
            "timed out after {}s waiting for page HTML",
            timeout.as_secs()
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(anyhow!("content worker thread disconnected"))
        }
    }
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
