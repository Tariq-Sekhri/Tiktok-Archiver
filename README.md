## Tiktok Archiver

Minimal TikTok account watcher and downloader written in Rust.

It watches configured TikTok accounts, keeps a JSON state of seen videos, and downloads missing videos using `yt-dlp`, while logging activity to a JSON log file.

### Requirements

- Rust toolchain (edition 2021)
- Python 3 installed and on disk
- `yt-dlp` installed in the Python environment (`python -m pip install yt-dlp`)

### First‑time setup

1. **Build**

```bash
cargo build
```

2. **Login to TikTok (save cookies)**

```bash
cargo run login
```

This opens a headless‑chrome session. Log in to TikTok in the browser window, then press Enter in the terminal to save cookies into `state/saved_cookies.json`.

3. **Edit config**

After the first run, a `config.yaml` file is created next to the executable. Edit it to point to your Python and download directory:

```yaml
accounts:
  - some_username
python_path: /absolute/path/to/python
download_dir: downloads
```

- `accounts`: list of usernames to watch; append `:false` to temporarily disable one.
- `python_path`: the Python interpreter to run `yt-dlp`.
- `download_dir`: base directory where per‑user folders and videos are stored.

### Running the watcher

Run the default mode (poll + download):

```bash
cargo run
```

The app will:

- Periodically query TikTok for each tracked account’s video count
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
  - `python_path` is correct and executable
  - `yt-dlp` is installed in that interpreter
  - Your cookies are still valid (repeat the login flow if needed)

