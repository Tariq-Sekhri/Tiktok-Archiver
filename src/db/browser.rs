use crate::db::{ensure_file, state_dir};
use anyhow::{anyhow, Context};
use anyhow::Result;
use headless_chrome::protocol::cdp::Network::CookieParam;
use headless_chrome::{browser, Browser};
use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;


pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const WAIT_AFTER_LOAD_S: u64 = 2;

pub enum CookiesMode {
    Persistent,
    None,
}

pub struct BrowserSession {
    _browser: Browser,
    pub tab: Arc<headless_chrome::Tab>,
}
pub fn cookies_path() -> Result<String> {
    let path = state_dir().join("saved_cookies.json");
    ensure_file(&path, "{\n  \"cookies\": []\n}\n")?;
    Ok(path.to_string_lossy().into_owned())
}

pub fn load_cookie_params() -> Result<Vec<CookieParam>> {
    let path = cookies_path()?;
    let content = fs::read_to_string(&path)?;
    let data: serde_json::Value =  serde_json::from_str(&content)?;
    let cookies = data.get("cookies").and_then(|c| c.as_array()).ok_or(anyhow!("error getting cookies"))?;
    let mut params = Vec::new();
    for c in cookies {
        let domain = c
            .get("domain")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim_start_matches('.');
        if !domain.contains("tiktok.com") && domain != "tiktok.com" {
            continue;
        }
        let name = match c.get("name").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let value = match c.get("value").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let path = c.get("path").and_then(|v| v.as_str()).map(String::from);
        let domain = if domain.is_empty() {
            None
        } else {
            Some(format!(".{}", domain))
        };
        let secure = c.get("secure").and_then(|v| v.as_bool());
        let http_only = c
            .get("httpOnly")
            .or(c.get("http_only"))
            .and_then(|v| v.as_bool());
        params.push(CookieParam {
            name,
            value,
            url: Some("https://www.tiktok.com".to_string()),
            domain,
            path: path.or(Some("/".to_string())),
            secure,
            http_only,
            same_site: None,
            expires: None,
            priority: None,
            same_party: None,
            source_scheme: None,
            source_port: None,
            partition_key: None,
        });
    }
    if params.is_empty() {
        eprintln!("  [Load Cookies] No tiktok.com cookies in {}", path);
        eprintln!("  [Load Cookies] run `cargo run` once to save your cookies (or `cargo run login` to swap accounts): {}", path);
    }
    Ok(params)
}

pub fn save_cookies(cookies: &[CookieParam])->Result<()> {
    let path = cookies_path()?;

    let cookies_json: Vec<serde_json::Value> = cookies
        .iter()
        .map(|c| {
            // Playwright uses -1 for session cookies; integer expires
            let expires = match c.expires {
                None => -1,
                Some(t) if t <= 0.0 => -1,
                Some(t) => t as i64,
            };
            let mut obj = serde_json::json!({
                "name": c.name,
                "value": c.value,
                "domain": c.domain,
                "path": c.path,
                "expires": expires,
                "httpOnly": c.http_only,
                "secure": c.secure,
            });
            // sameSite only if set (Playwright: "Strict" | "Lax" | "None"); omit when null
            if let Some(ref s) = c.same_site {
                obj["sameSite"] = serde_json::json!(s);
            }
            obj
        })
        .collect();

    let root = serde_json::json!({ "cookies": cookies_json });

    let json_str = serde_json::to_string_pretty(&root)?;
    fs::write(&path, json_str)?;
    Ok(())
}

pub fn cookie_to_param(
    cookies: Vec<headless_chrome::protocol::cdp::Network::Cookie>,
) -> Vec<CookieParam> {
    cookies
        .into_iter()
        .map(|cookie| CookieParam {
            name: cookie.name,
            value: cookie.value,
            url: Some("https://www.tiktok.com".to_string()),
            domain: if cookie.domain.starts_with('.') {
                Some(cookie.domain)
            } else {
                Some(format!(".{}", cookie.domain))
            },
            path: Some(cookie.path),
            secure: Some(cookie.secure),
            http_only: Some(cookie.http_only),
            same_site: cookie.same_site,
            expires: Some(cookie.expires),
            priority: Some(cookie.priority),
            same_party: Some(cookie.same_party),
            source_scheme: Some(cookie.source_scheme),
            source_port: Some(cookie.source_port),
            partition_key: cookie.partition_key,
        })
        .collect()
}


pub fn cookies_have_any(path: &PathBuf) -> bool {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let v: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };
    v.get("cookies")
        .and_then(|c| c.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false)
}

fn tiktok_profile_path() -> PathBuf {
    state_dir().join("tiktok_profile")
}

pub fn launch_browser(url: &str, mode: CookiesMode, headless:bool) -> Result<BrowserSession> {
    let profile_dir = match mode {
        CookiesMode::Persistent => {
            let p = tiktok_profile_path();
            fs::create_dir_all(&p)?;
            Some(p)
        }
        CookiesMode::None => None,
    };

    let mut builder = browser::LaunchOptionsBuilder::default();
    builder.headless(headless);
    builder.window_size(Some((1920, 1080)));
    builder.idle_browser_timeout(Duration::from_secs(3600));
    builder.user_data_dir(profile_dir);
    builder.args(vec![
        OsStr::new("--disable-blink-features=AutomationControlled"),
        OsStr::new("--disable-infobars"),
        OsStr::new("--no-sandbox"),
    ]);
    builder.ignore_default_args(vec![OsStr::new("--enable-automation")]);
    let launch_opts = builder.build().expect("LaunchOptions");

    let browser = Browser::new(launch_opts)
        .context("Failed to launch headless_chrome browser")?;
    let tab = browser
        .new_tab()
        .context("Failed to open new browser tab for TikTok session")?;
    tab.set_user_agent(USER_AGENT, Some("en-US,en;q=0.9"), None)
        .context("Failed to set TikTok user agent on tab")?;

    if matches!(mode, CookiesMode::Persistent) {
        let cookie_params = load_cookie_params()?;
        if !cookie_params.is_empty() {
            tab.navigate_to("https://www.tiktok.com")
                .with_context(|| "Failed to navigate to tiktok.com for cookie injection")?;
            std::thread::sleep(Duration::from_millis(500));
            if let Err(e) = tab.set_cookies(cookie_params) {
                eprintln!("  [Browser] set_cookies failed: {}", e);
            }
        }
    }

    tab.navigate_to(url)
        .with_context(|| format!("Failed to navigate TikTok tab to URL: {}", url))
        .map_err(|e| {
            eprintln!("[Browser] navigate_to error for {}: {:#}", url, e);
            e
        })?;

    std::thread::sleep(Duration::from_secs(WAIT_AFTER_LOAD_S));

    Ok(BrowserSession {
        _browser: browser,
        tab,
    })
}

pub fn scroll_to_bottom(session: &BrowserSession) -> Result<()> {
    loop {
        let reached_end: bool = session
            .tab
            .evaluate(
                r#"
                (function() {
                    const oldHeight = document.body.scrollHeight;
                    window.scrollTo(0, oldHeight);

                    return new Promise((resolve) => {
                        // Wait for potential network/DOM update
                        setTimeout(() => {
                            const newHeight = document.body.scrollHeight;
                            const isAtBottom = window.innerHeight + window.scrollY >= newHeight - 10;

                            // Done if height didn't change OR we are physically at the bottom
                            resolve(newHeight === oldHeight || isAtBottom);
                        }, 1500); // Increased to 1.5s for TikTok's slow loading
                    });
                })()
                "#,
                true,
            )
            .context("Failed to evaluate scroll script")?
            .value
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        if reached_end {
            std::thread::sleep(Duration::from_millis(500));
            break;
        }
    }
    Ok(())
}