## Tiktok Archiver
Minimal TikTok account watcher and downloader written in Rust.
It watches configured TikTok accounts, keeps a JSON state of seen videos, and downloads missing videos using `yt-dlp`, while logging activity to level-specific log files under `state/`.

**[Setup Guide Video](https://www.youtube.com/watch?v=3Ewcy7WfzaA)** — Watch this for a walkthrough of the installation and configuration process.

### Requirements
- Rust toolchain (edition 2021)

### First‑time setup
1. **Build**
```bash
cargo build
```
2. **Choose accounts and (optionally) download directory**
After the first run, a `config.yaml` file is created next to the executable. Edit it to set your accounts and download directory:
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
The app will:
- Periodically query TikTok for each tracked account's video count
- Discover new videos via a browser session when counts increase
- Append new videos into `state/seen_videos.json`
- Download any pending videos to `<download_dir>/<username>/<video_id>.mp4` (or `<download_dir>/favs/` when `download_fav` is enabled)
- Maintain derived state for each account in `state/accounts.json`

### State files
All persistent state lives under the `state` directory created in the project root:
- `saved_cookies.json`: TikTok cookies captured during `cargo run login`
- `accounts.json`: per‑account counts, diffs, and unavailable counts
- `seen_videos.json`: per‑account list of discovered videos and download status
- `info.log`: routine operational events
- `error.log`: recoverable failures (poll, download, discovery)
- `criticalfail.log`: unrecoverable failures that stop the process

### Troubleshooting
- If you see messages about missing cookies or config, follow the printed instructions in the terminal and rerun `cargo run login` or fix `config.yaml`.
- If `yt-dlp` fails, check `state/error.log` and verify:
  - Your cookies are still valid (repeat the login flow if needed)
