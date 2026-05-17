use crate::db::{atomic_write_text, ensure_file, state_dir};
use anyhow::{anyhow, Context};
use anyhow::Result;
use headless_chrome::protocol::cdp::Network::{Cookie, CookieParam, CookieSameSite, SetCookies};
use headless_chrome::{browser, Browser};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub const TIKTOK_ORIGIN: &str = "https://www.tiktok.com";
const COOKIE_INJECT_CHUNK: usize = 40;
const SESSION_COOKIE_NAMES: &[&str] = &["sid_tt", "sessionid", "sid_guard", "uid_tt", "tt_session_tlb_tag"];


pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
pub const SHOW_BROWSER_ENV: &str = "TTA_SHOW_BROWSER";
const WAIT_AFTER_LOAD_S: u64 = 2;

pub fn discovery_headless() -> bool {
    if matches!(std::env::var(SHOW_BROWSER_ENV).as_deref(), Ok("1")) {
        return false;
    }
    if matches!(std::env::var(SHOW_BROWSER_ENV).as_deref(), Ok("0")) {
        return true;
    }
    !cfg!(debug_assertions)
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

fn normalize_cookie_domain(raw: &str) -> Option<String> {
    let d = raw.trim();
    if d.is_empty() {
        return None;
    }
    if d.starts_with('.') {
        Some(d.to_string())
    } else {
        Some(format!(".{}", d))
    }
}

fn is_tiktok_cookie_entry(c: &serde_json::Value) -> bool {
    if c.get("domain")
        .and_then(|v| v.as_str())
        .is_some_and(|d| d.contains("tiktok.com"))
    {
        return true;
    }
    c.get("url")
        .and_then(|v| v.as_str())
        .is_some_and(|u| u.contains("tiktok.com"))
}

fn parse_same_site(c: &serde_json::Value) -> Option<CookieSameSite> {
    let s = c
        .get("sameSite")
        .or_else(|| c.get("same_site"))
        .and_then(|v| v.as_str())?;
    match s {
        "Strict" => Some(CookieSameSite::Strict),
        "Lax" => Some(CookieSameSite::Lax),
        "None" => Some(CookieSameSite::None),
        _ => None,
    }
}

fn parse_expires(c: &serde_json::Value) -> Option<f64> {
    let v = c.get("expires")?;
    let t = v.as_i64().map(|i| i as f64).or_else(|| v.as_f64())?;
    if t > 0.0 {
        Some(t)
    } else {
        None
    }
}

fn build_cookie_param(
    name: String,
    value: String,
    domain: Option<String>,
    path: Option<String>,
    secure: Option<bool>,
    http_only: Option<bool>,
    same_site: Option<CookieSameSite>,
    expires: Option<f64>,
) -> CookieParam {
    CookieParam {
        name,
        value,
        url: None,
        domain,
        path: path.or(Some("/".to_string())),
        secure,
        http_only,
        same_site,
        expires,
        priority: None,
        same_party: None,
        source_scheme: None,
        source_port: None,
        partition_key: None,
    }
}

fn to_injection_param(p: CookieParam, cookie_url: &str) -> CookieParam {
    let same_site = p.same_site.clone();
    let mut secure = p.secure;
    if same_site == Some(CookieSameSite::None) {
        secure = Some(true);
    }
    CookieParam {
        name: p.name,
        value: p.value,
        url: Some(cookie_url.to_string()),
        domain: None,
        path: p.path.or(Some("/".to_string())),
        secure,
        http_only: p.http_only,
        same_site,
        expires: p.expires,
        priority: None,
        same_party: None,
        source_scheme: None,
        source_port: None,
        partition_key: None,
    }
}

fn has_session_cookie(cookies: &[Cookie]) -> bool {
    cookies
        .iter()
        .any(|c| SESSION_COOKIE_NAMES.contains(&c.name.as_str()))
}

pub fn cookie_params_have_session(params: &[CookieParam]) -> bool {
    params
        .iter()
        .any(|p| SESSION_COOKIE_NAMES.contains(&p.name.as_str()))
}

pub fn clear_tiktok_profile() -> Result<()> {
    let p = tiktok_profile_path();
    if p.exists() {
        fs::remove_dir_all(&p).context("failed to clear tiktok_profile")?;
    }
    Ok(())
}

fn inject_cookies(tab: &headless_chrome::Tab, params: Vec<CookieParam>, cookie_url: &str) -> Result<()> {
    if params.is_empty() {
        return Ok(());
    }
    let cookies: Vec<CookieParam> = params
        .into_iter()
        .map(|p| to_injection_param(p, cookie_url))
        .collect();
    let expected: HashSet<String> = cookies.iter().map(|c| c.name.clone()).collect();
    for chunk in cookies.chunks(COOKIE_INJECT_CHUNK) {
        tab.call_method(SetCookies {
            cookies: chunk.to_vec(),
        })
        .context("Network.setCookies failed")?;
    }
    std::thread::sleep(Duration::from_millis(300));
    let applied = tab.get_cookies().context("get_cookies after inject")?;
    let matched = applied
        .iter()
        .filter(|c| expected.contains(&c.name))
        .count();
    if matched == 0 {
        return Err(anyhow!(
            "cookie injection applied 0 of {} saved cookies",
            expected.len()
        ));
    }
    if !has_session_cookie(&applied) {
        return Err(anyhow!(
            "cookie injection missing session cookies (matched {}/{} names)",
            matched,
            expected.len()
        ));
    }
    eprintln!(
        "[Browser] injected cookies: {}/{} names present in browser",
        matched,
        expected.len()
    );
    Ok(())
}

pub fn load_cookie_params() -> Result<Vec<CookieParam>> {
    let path = cookies_path()?;
    let content = fs::read_to_string(&path)?;
    let data: serde_json::Value =  serde_json::from_str(&content)?;
    let cookies = data.get("cookies").and_then(|c| c.as_array()).ok_or(anyhow!("error getting cookies"))?;
    let mut params = Vec::new();
    for c in cookies {
        if !is_tiktok_cookie_entry(c) {
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
        let domain = c
            .get("domain")
            .and_then(|v| v.as_str())
            .and_then(normalize_cookie_domain);
        let secure = c.get("secure").and_then(|v| v.as_bool());
        let http_only = c
            .get("httpOnly")
            .or(c.get("http_only"))
            .and_then(|v| v.as_bool());
        let same_site = parse_same_site(c);
        let expires = parse_expires(c);
        params.push(build_cookie_param(
            name,
            value,
            domain,
            path,
            secure,
            http_only,
            same_site,
            expires,
        ));
    }
    if params.is_empty() {
        eprintln!("  [Load Cookies] No tiktok.com cookies in {}", path);
        eprintln!("  [Load Cookies] run `cargo run` once to save your cookies (or `cargo run login` to swap accounts): {}", path);
    }
    Ok(params)
}

pub fn cookie_params_to_netscape_cookies_txt(params: &[CookieParam]) -> String {
    let mut lines: Vec<String> = vec![
        "# Netscape HTTP Cookie File".to_string(),
        "# https://curl.se/docs/http-cookies.html".to_string(),
    ];
    for p in params {
        let domain = p.domain.as_deref().unwrap_or(".tiktok.com");
        let include_subdomains = if domain.starts_with('.') {
            "TRUE"
        } else {
            "FALSE"
        };
        let path = p.path.as_deref().unwrap_or("/");
        let secure = if p.secure.unwrap_or(false) {
            "TRUE"
        } else {
            "FALSE"
        };
        let expiration = match p.expires {
            Some(t) if t > 0.0 => t as i64,
            _ => 0,
        };
        lines.push(format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            domain, include_subdomains, path, secure, expiration, p.name, p.value
        ));
    }
    format!("{}\n", lines.join("\n"))
}

pub fn write_ytdlp_cookie_jar(params: &[CookieParam]) -> Result<PathBuf> {
    let path = state_dir().join("ytdlp_cookies.txt");
    let content = cookie_params_to_netscape_cookies_txt(params);
    atomic_write_text(&path, &content)?;
    Ok(path)
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
                let label = match s {
                    CookieSameSite::Strict => "Strict",
                    CookieSameSite::Lax => "Lax",
                    CookieSameSite::None => "None",
                };
                obj["sameSite"] = serde_json::json!(label);
            }
            obj
        })
        .collect();

    let root = serde_json::json!({ "cookies": cookies_json });

    let json_str = serde_json::to_string_pretty(&root)?;
    atomic_write_text(std::path::Path::new(&path), &json_str)?;
    write_ytdlp_cookie_jar(cookies)?;
    Ok(())
}

pub fn log_auth_storage_status() {
    let dir = state_dir();
    let cookies_path = dir.join("saved_cookies.json");
    let profile_path = tiktok_profile_path();

    eprintln!("[auth] state directory: {}", dir.display());
    eprintln!("[auth] cookies file: {}", cookies_path.display());

    if let Ok(meta) = fs::metadata(&cookies_path) {
        if let Ok(modified) = meta.modified() {
            if let Ok(duration) = modified.elapsed() {
                eprintln!(
                    "[auth] cookies file age: {}h {}m ago",
                    duration.as_secs() / 3600,
                    (duration.as_secs() % 3600) / 60
                );
            }
        }
    }

    match load_cookie_params() {
        Ok(params) => {
            eprintln!("[auth] cookies in file: {}", params.len());
            eprintln!(
                "[auth] session cookies in file: {}",
                if cookie_params_have_session(&params) {
                    "yes"
                } else {
                    "no — run `cargo run login`"
                }
            );
        }
        Err(e) => eprintln!("[auth] could not read cookies file: {}", e),
    }

    eprintln!(
        "[auth] chrome profile: {} ({})",
        profile_path.display(),
        if profile_path.exists() {
            "present"
        } else {
            "missing — run `cargo run login`"
        }
    );

    let alt = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("release").join("state");
    if alt.exists() && alt != dir {
        eprintln!(
            "[auth] WARNING: ignored duplicate state at {} (release build leftover)",
            alt.display()
        );
    }
}

pub fn cookie_to_param(
    cookies: Vec<headless_chrome::protocol::cdp::Network::Cookie>,
) -> Vec<CookieParam> {
    cookies
        .into_iter()
        .filter(|cookie| cookie.domain.contains("tiktok.com"))
        .map(|cookie| {
            let domain = if cookie.domain.starts_with('.') {
                cookie.domain
            } else {
                format!(".{}", cookie.domain)
            };
            let expires = if cookie.expires > 0.0 {
                Some(cookie.expires)
            } else {
                None
            };
            build_cookie_param(
                cookie.name,
                cookie.value,
                Some(domain),
                Some(cookie.path),
                Some(cookie.secure),
                Some(cookie.http_only),
                cookie.same_site,
                expires,
            )
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

pub fn launch_browser(url: &str, headless:bool) -> Result<BrowserSession> {
    let cookie_params =   load_cookie_params()?;


    let profile_dir = {
            let p = tiktok_profile_path();
            fs::create_dir_all(&p)?;
            Some(p)
    };
        eprintln!(
            "[Browser] state={} profile={} cookies_to_inject={}",
            state_dir().display(),
            tiktok_profile_path().display(),
            cookie_params.len()
        );


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

    let inject_from_file = !cookie_params.is_empty();
    if inject_from_file {
        tab.navigate_to(url)
            .with_context(|| format!("Failed to navigate to {} before cookie injection", url))?;
        tab.wait_until_navigated()
            .with_context(|| format!("Timed out waiting for {} before cookie injection", url))?;
        std::thread::sleep(Duration::from_millis(500));
        inject_cookies(&tab, cookie_params, url)?;
        tab.navigate_to(url)
            .with_context(|| format!("Failed to reload {} after cookie injection", url))?;
        tab.wait_until_navigated()
            .with_context(|| format!("Timed out waiting for reload of {}", url))?;
    } else {
        tab.navigate_to(url)
            .with_context(|| format!("Failed to navigate TikTok tab to URL: {}", url))
            .map_err(|e| {
                eprintln!("[Browser] navigate_to error for {}: {:#}", url, e);
                e
            })?;
        tab.wait_until_navigated()
            .with_context(|| format!("Timed out waiting for navigation to {}", url))?;
    }

    std::thread::sleep(Duration::from_secs(WAIT_AFTER_LOAD_S));

    if  inject_from_file {
        let applied = tab.get_cookies().context("get_cookies after launch")?;
        if !has_session_cookie(&applied) {
            return Err(anyhow!(
                "session cookies not present after launch — run `cargo run login` to refresh login (state: {})",
                state_dir().display()
            ));
        }
    }

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