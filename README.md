## Tiktok Archiver
Minimal TikTok account watcher and downloader written in Rust.
It watches configured TikTok accounts, keeps a JSON state of seen videos, and downloads missing videos using `yt-dlp`, while logging activity to a JSON log file.


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
```
- `accounts`: list of usernames to watch; append `:false` to temporarily disable one.
- `download_dir`: base directory where per‑user folders and videos are stored.

On the very first run, if no TikTok cookies are present, the app will open a browser window and walk you through logging in, then save cookies into `state/saved_cookies.json`. You can always run:
```bash
cargo run login
```
later to explicitly trigger the login flow (for switching accounts or refreshing cookies).

### Running the watcher
Run the default mode (poll + download):
```bash
cargo run
```
The app will:
- Periodically query TikTok for each tracked account's video count
- Discover new videos via a browser session when counts increase
- Append new videos into `state/seen_videos.json`
- Download any pending videos to `<download_dir>/<username>/<video_id>.mp4`
- Maintain derived state for each account in `state/accounts.json`

### State files
All persistent JSON state lives under the `state` directory created in the project root:
- `saved_cookies.json`: TikTok cookies captured during `cargo run login`
- `accounts.json`: per‑account counts, diffs, and unavailable counts
- `seen_videos.json`: per‑account list of discovered videos and download status
- `log.json`: JSON log entries with timestamps and log levels

### Troubleshooting
- If you see messages about missing cookies or config, follow the printed instructions in the terminal and rerun `cargo run login` or fix `config.yaml`.
- If `yt-dlp` fails, check the log messages in `state/log.json` and verify:
  - Your cookies are still valid (repeat the login flow if needed)
