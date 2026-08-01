## Tiktok Archiver
Minimal TikTok account watcher and downloader written in Rust (v1.1.0).
It watches configured TikTok accounts, keeps a JSON state of seen videos, and downloads missing videos using `yt-dlp`, while logging activity to level-specific log files under `state/`.

**[Setup Guide Video](https://www.youtube.com/watch?v=3Ewcy7WfzaA)** — Watch this for a walkthrough of the installation and configuration process.

### Requirements
- Rust toolchain (edition 2021)
- Google Chrome (used via `headless_chrome` for profile scraping)
- Windows (the app downloads `yt-dlp.exe`, uses Windows process flags, and shows native alerts on critical failure)

`yt-dlp` is downloaded automatically into `state/` on first run if it is not already present.

### Install from release
Pre-built Windows executables are published on every push to `master` via GitHub Actions. Download the latest `.exe` from the [Releases](https://github.com/Tariq-Sekhri/Tiktok-Archiver/releases) page, place `config.yaml` next to it, and run it directly. State files are created in a `state/` folder beside the executable.

### First‑time setup
1. **Build**
```bash
cargo build
```
2. **Choose accounts and (optionally) download directory**

`config.yaml` is created on first run. When using `cargo run`, it lives in the project root; when running a release binary, it lives next to the executable.

```bash
cargo run
```
```yaml
accounts:
  - some_username
download_dir: downloads
download_fav: false
```
- `accounts`: list of usernames to watch; append `:false` to temporarily disable one.
- `download_dir`: base directory where per‑user folders and videos are stored.
- `download_fav`: when `true`, also watch and download videos from the signed‑in account’s Favorites tab (see below).

On the very first run, if no TikTok cookies are present, the app will open a browser window and walk you through logging in, then save cookies into `state/saved_cookies.json`. You can always run:
```bash
cargo run login
```
later to explicitly trigger the login flow (for switching accounts or refreshing cookies).

### Favorites (`download_fav`)
Set `download_fav: true` in `config.yaml` to archive videos you have favorited on TikTok.

Favorites use the same browser session and cookies as the rest of the app. Each poll cycle opens the **profile of whichever account is signed in** (the one in `state/saved_cookies.json`), goes to that account’s **Favorites** tab, discovers new items, and downloads them to `<download_dir>/favs/<video_id>.mp4`. They are tracked separately from the `accounts` list under the `favorite` key in `seen_videos.json`.

To archive a different user’s favorites, run `cargo run login` while signed into that TikTok account so the saved cookies match.

### Running the watcher
Run the default mode (poll + download):
```bash
cargo run
```

Debug mode — visible browser window and verbose tracing to stderr:
```bash
cargo run dev
```

Each poll cycle:
- Launches a headless Chrome session (visible in `dev` mode) using saved cookies
- Visits each tracked account’s profile, scrolls until no new video IDs appear, and parses links from the page HTML
- Optionally checks the signed‑in account’s Favorites tab when `download_fav` is enabled
- Appends newly discovered videos into `state/seen_videos.json`
- Downloads any pending videos to `<download_dir>/<username>/<video_id>.mp4` (or `<download_dir>/favs/` for favorites)
- Waits 5 minutes, then repeats

Downloads retry up to 5 times per video; after that the video is marked unavailable and skipped.

If every poll cycle fails 5 times in a row, the critical failure is first recorded durably and the Chrome processes using this archiver's `state/tiktok_profile` are terminated. The process then exits so PM2 can restart it. If another critical failure occurs before a successful poll, the app also shows a Windows critical-error message.

### Running in the background (PM2)
`ecosystem.config.cjs` is included for running the release binary under [PM2](https://pm2.keymetrics.io/):

```bash
cargo build --release
pm2 start ecosystem.config.cjs
```

PM2 stdout/stderr are written to `state/pm2-out.log` and `state/pm2-error.log`. Critical events are always recorded before a Windows message box is shown. Set `TTA_SILENT_CRIT_ALERT=1` to suppress the popup after the recovery retry.

### State files
All persistent state lives under `state/` (project root when using `cargo run`, or beside the executable for release builds):

- `saved_cookies.json`: TikTok cookies captured during login
- `seen_videos.json`: per‑account (and `favorite`) lists of discovered videos and download status
- `poll_health.json`: consecutive poll-failure tracking
- `ytdlp_cookies.txt`: cookie jar passed to `yt-dlp` at download time
- `yt-dlp.exe`: auto-downloaded downloader binary
- `tiktok_profile/`: persistent Chrome user profile used for scraping
- `info.log`: routine operational events
- `error.log`: recoverable failures (poll, download, discovery)
- `criticalfail.log`: unrecoverable failures that stop the process
- `critical_recovery.json`: whether a Chrome-recovery attempt is awaiting a successful poll
- `diagnostic.log`: detailed lifecycle, timing, browser, discovery, and download diagnostics; it is recorded in normal background operation too

Logs append continuously and are never rotated automatically. Critical records are synced before exit; logging failures are sent to stderr so PM2 captures them instead of being silently discarded.

### Troubleshooting
- If you see messages about missing cookies or config, follow the printed instructions in the terminal and rerun `cargo run login` or fix `config.yaml`.
- If `yt-dlp` fails, check `state/error.log` and verify your cookies are still valid (repeat the login flow if needed).
- If polls keep failing, check `state/poll_health.json`, `state/error.log`, and `state/criticalfail.log` for the last error, fix the underlying issue, and restart.
- For browser or login issues, try `cargo run dev` to watch what Chrome is doing, or `cargo run login` to refresh cookies.
