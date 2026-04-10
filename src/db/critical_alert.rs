use std::process::Command;

const MAX_ALERT_CHARS: usize = 3500;

pub fn alert_critical_failure(message: &str) {
    if std::env::var_os("TTA_SILENT_CRIT_ALERT")
        .map(|v| {
            let s = v.to_string_lossy().to_ascii_lowercase();
            matches!(s.as_str(), "1" | "true" | "yes")
        })
        .unwrap_or(false)
    {
        return;
    }
    let body: String = if message.chars().count() > MAX_ALERT_CHARS {
        message.chars().take(MAX_ALERT_CHARS).collect::<String>() + "…"
    } else {
        message.to_string()
    };
    #[cfg(windows)]
    windows_message_box(&body);
    #[cfg(not(windows))]
    unix_try_notify(&body);
}

#[cfg(windows)]
fn windows_message_box(body: &str) {
    let title = "Tiktok-Archiver — critical failure (PM2 / background)";
    let mut cmd = Command::new("powershell.exe");
    cmd.env("TTA_CRITICAL_MSG", body);
    cmd.env("TTA_CRITICAL_TITLE", title);
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-Sta",
        "-WindowStyle",
        "Normal",
        "-Command",
        "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.MessageBox]::Show($env:TTA_CRITICAL_MSG, $env:TTA_CRITICAL_TITLE, [System.Windows.Forms.MessageBoxButtons]::OK, [System.Windows.Forms.MessageBoxIcon]::Error, [System.Windows.Forms.MessageBoxDefaultButton]::Button1, [System.Windows.Forms.MessageBoxOptions]::ServiceNotification)",
    ]);
    let _ = cmd.status();
}

#[cfg(not(windows))]
fn unix_try_notify(body: &str) {
    let _ = Command::new("notify-send")
        .args([
            "-u",
            "critical",
            "-a",
            "Tiktok-Archiver",
            "Critical failure",
            body,
        ])
        .spawn();
}
