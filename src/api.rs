use crate::db::browser::{discovery_headless, launch_browser, BrowserSession};
use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde_json::Value;
use std::time::{Duration, Instant};
use headless_chrome::protocol::cdp::Network::InitiatorType::Parser;
use crate::db::video;
use crate::db::video::Video;

fn parse_rehydration(html: &str) -> Option<Value> {
    let re = Regex::new(
        r#"(?s)<script[^>]*id=["']__UNIVERSAL_DATA_FOR_REHYDRATION__["'][^>]*>([^<]+)</script>"#,
    )
    .ok()?;

    let caps = re.captures(html)?;
    let json_str = caps.get(1)?.as_str().trim();

    serde_json::from_str(json_str).ok()
}

pub fn video_count_from_html(html: &str) -> Result<i64> {
    let data = parse_rehydration(html)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse rehydration: html dump({})", html))?;
    let video_count = data
        .pointer("/__DEFAULT_SCOPE__/webapp.user-detail/userInfo/stats/videoCount")
        .ok_or_else(|| anyhow::anyhow!("Error getting video count"))?;
    video_count
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("failed to parse video count as i64"))
}

fn wait_for_profile_page(session: &BrowserSession, username: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let html = session.tab.get_content().context("get_content")?;
        if video_count_from_html(&html).is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out waiting for @{} profile page to finish loading",
                username
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

pub async fn get_new_count(username: &str) -> Result<i64> {
    let url = format!("https://www.tiktok.com/@{}", username);
    let session = launch_browser(&url, discovery_headless())?;
    wait_for_profile_page(&session, username)?;
    let html = session.tab.get_content().context("get_content")?;
    video_count_from_html(&html)
}

pub fn videos_from_anchor_links(html: &str, is_fav:bool) -> Result<Vec<Video>> {
    let re = Regex::new(r#"/@([\w.]+)/video/(\d+)"#)?;
    let mut for_ret:Vec<Video>= Vec::new();
    for cap in re.captures_iter(html) {
        let username = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let id = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        if id.is_empty() {
            continue;
        }

        for_ret.push(Video::new(format!("https/tiktok.com/@{}/{}", username, id), id.parse()?, username.to_string(), is_fav));
    }
        Ok(for_ret)
}

