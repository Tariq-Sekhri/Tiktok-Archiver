use crate::db::browser::{
    discovery_headless, launch_browser, launch_browser_with_cleanup, load_cookie_params,
    BrowserSession, TIKTOK_ORIGIN, USER_AGENT,
};
use crate::db::logger::Log;
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
        .header("User-Agent", USER_AGENT)
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
    session.navigate_profile(username)?;
    let count = wait_for_video_count(session, username)?;
    Log::dev_timing("browser_count", t0);
    Log::dev(format!(
        "[api] get_new_count @{} parsed videoCount={}",
        username, count
    ));
    Ok(count)
}

pub fn open_poll_session() -> Result<BrowserSession> {
    let headless = discovery_headless();
    launch_browser_with_cleanup(TIKTOK_ORIGIN, headless, true)
}

pub fn fetch_counts_on_session(
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

pub fn finish_poll_session(session: BrowserSession) {
    session.finish_light();
}

pub fn fetch_poll_counts(usernames: &[String]) -> Result<Vec<(String, Result<i64>)>> {
    let t0 = Instant::now();
    let session = open_poll_session()?;
    let out = fetch_counts_on_session(&session, usernames)?;
    finish_poll_session(session);
    Log::dev_timing("poll_browser_session", t0);
    Ok(out)
}

fn get_new_count_sync(username: &str) -> Result<i64> {
    let url = format!("https://www.tiktok.com/@{}", username);
    Log::dev(format!(
        "[api] get_new_count @{} url={} headless={}",
        username,
        url,
        discovery_headless()
    ));
    let session = launch_browser(&url, discovery_headless())?;
    let result = get_new_count_with_session(&session, username);
    session.finish();
    result
}

pub async fn get_new_count(username: &str) -> Result<i64> {
    let user = username.to_string();
    match tokio::time::timeout(
        ACCOUNT_FETCH_TIMEOUT,
        tokio::task::spawn_blocking(move || get_new_count_sync(&user)),
    )
    .await
    {
        Ok(Ok(inner)) => inner,
        Ok(Err(e)) => Err(anyhow!("@{} fetch task failed: {}", username, e)),
        Err(_) => Err(anyhow!(
            "@{} timed out after {}s",
            username,
            ACCOUNT_FETCH_TIMEOUT.as_secs()
        )),
    }
}

pub async fn fetch_poll_counts_async(usernames: Vec<String>) -> Result<Vec<(String, Result<i64>)>> {
    match tokio::time::timeout(
        POLL_FETCH_TIMEOUT,
        tokio::task::spawn_blocking(move || fetch_poll_counts(&usernames)),
    )
    .await
    {
        Ok(Ok(inner)) => inner,
        Ok(Err(e)) => Err(anyhow!("poll count task failed: {}", e)),
        Err(_) => Err(anyhow!(
            "poll counts timed out after {}s",
            POLL_FETCH_TIMEOUT.as_secs()
        )),
    }
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
