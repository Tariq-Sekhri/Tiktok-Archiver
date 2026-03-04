use crate::db::browser::cookies_path;
use crate::db::seen_video::{DownloadStatus, SeenVideo};
use anyhow::{Context, Result};
use regex::Regex;
use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, COOKIE, USER_AGENT},
    Client,
};
use serde_json::Value;
use std::{fs, time::Duration};

fn parse_rehydration(html: &str) -> Option<Value> {
    let re = Regex::new(
        r#"(?s)<script[^>]*id=["']__UNIVERSAL_DATA_FOR_REHYDRATION__["'][^>]*>([^<]+)</script>"#,
    )
    .ok()?;

    let caps = re.captures(html)?;
    let json_str = caps.get(1)?.as_str().trim();

    serde_json::from_str(json_str).ok()
}

pub async fn get_new_count(username: &str) -> Result<i64> {
    let html = &fetch_html(username).await?;
    let data = parse_rehydration(html)
        // .ok_or_else(|| anyhow::anyhow!("Failed to parse rehydration: html dump({})", html))?;
        .ok_or_else(|| anyhow::anyhow!("Failed to parse rehydration" ))?;
    let video_count = data
        .pointer("/__DEFAULT_SCOPE__/webapp.user-detail/userInfo/stats/videoCount")
        .ok_or_else(|| anyhow::anyhow!("Error getting video count"))?;
    video_count
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("failed to parse video count as i64"))
}


async fn fetch_html(username: &str) -> Result<String> {
    let url = format!("https://www.tiktok.com/@{}", username);
    let headers = get_headers_reqwest(true);
    let client = Client::builder()
        .gzip(false)
        .default_headers(headers)
        .timeout(Duration::from_secs(30))
        .build()
        .context("build reqwest client")?;

    client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("Fetch error: {}", url))?
        .text()
        .await
        .with_context(|| format!("Fetch text error: {}", url))
}

pub fn videos_from_anchor_links(html: &str, username: &str) -> Result<Vec<SeenVideo>> {
    let re = Regex::new(r#"/@([\w.]+)/video/(\d+)"#)?;
    let mut seen = std::collections::HashSet::new();
    let mut ids = Vec::new();
    for cap in re.captures_iter(html) {
        let u = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let vid = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        if vid.is_empty() {
            continue;
        }
        if u.eq_ignore_ascii_case(username) && seen.insert(vid.to_string()) {
            ids.push(vid.to_string());
        }
    }
    ids.into_iter()
        .map(|id| {
            let parsed_id = id.parse::<i64>()?;
            Ok(SeenVideo::new(
                format!("https://www.tiktok.com/@{}/video/{}", username, parsed_id),
                parsed_id,
                username.to_string(),
                DownloadStatus::NotDownloaded,
                true,
            ))
        })
        .collect::<Result<Vec<_>>>()
}

fn get_saved_cookie_header() -> Result<String> {
    let content = fs::read_to_string(cookies_path()?)?;
    let data: Value = serde_json::from_str(&content)?;

    let cookies = data
        .get("cookies")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("missing or invalid cookies array"))?;
    let mut parts = Vec::new();

    for cookie in cookies {
        let domain = cookie
            .get("domain")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim_start_matches('.')
            .to_string();

        if !domain.contains("tiktok.com") && domain != "tiktok.com" {
            continue;
        }

        let name = cookie
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("cookie missing name"))?;
        let value = cookie
            .get("value")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("cookie missing value"))?;

        parts.push(format!("{}={}", name, value));
    }

    if parts.is_empty() {
        Err(anyhow::anyhow!("no tiktok cookies in saved_cookies.json"))
    } else {
        Ok(parts.join("; "))
    }
}

fn get_headers_reqwest(with_cookies: bool) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        ),
    );
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
    );
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
    if with_cookies {
        if let Ok(cookie) = get_saved_cookie_header() {
            if let Ok(value) = HeaderValue::from_str(&cookie) {
                headers.insert(COOKIE, value);
            }
        }
    }
    headers
}


