use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const MAX_ALERT_CHARS: usize = 3500;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub fn alert_critical_failure(message: &str) {
    if alerts_disabled() {
        return;
    }
    let body = truncate(message);
    spawn_background_alert("Tiktok-Archiver", &body);
}

pub fn alert_download_unavailable(message: &str) {
    alert_critical_failure(message);
}

fn truncate(message: &str) -> String {
    if message.chars().count() > MAX_ALERT_CHARS {
        message.chars().take(MAX_ALERT_CHARS).collect::<String>() + "…"
    } else {
        message.to_string()
    }
}

fn alerts_disabled() -> bool {
    std::env::var_os("TTA_SILENT_CRIT_ALERT")
        .map(|v| {
            let s = v.to_string_lossy().to_ascii_lowercase();
            matches!(s.as_str(), "1" | "true" | "yes")
        })
        .unwrap_or(false)
}

fn spawn_background_alert(title: &str, body: &str) {
    std::thread::spawn({
        let title = title.to_string();
        let body = body.to_string();
        move || {
            #[cfg(windows)]
            {
                play_alert_sound();
                show_windows_toast(&title, &body);
            }
            #[cfg(not(windows))]
            unix_try_notify(&body);
        }
    });
}

#[cfg(windows)]
fn play_alert_sound() {
    let script = r#"
Add-Type -AssemblyName System.Media
[System.Media.SystemSounds]::Hand.Play()
Start-Sleep -Milliseconds 180
[System.Media.SystemSounds]::Exclamation.Play()
Start-Sleep -Milliseconds 180
[System.Media.SystemSounds]::Hand.Play()
"#;
    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-WindowStyle",
        "Hidden",
        "-Command",
        script,
    ]);
    cmd.creation_flags(CREATE_NO_WINDOW);
    let _ = cmd.spawn();
}

#[cfg(windows)]
fn show_windows_toast(title: &str, body: &str) {
    let script = r#"
$AppId = 'TiktokArchiver.TTA'
$reg = "HKCU:\Software\Classes\AppUserModelId\$AppId"
if (-not (Test-Path $reg)) {
  New-Item -Path $reg -Force | Out-Null
  Set-ItemProperty -Path $reg -Name DisplayName -Value 'Tiktok Archiver'
}
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
$title = $env:TTA_TOAST_TITLE
$body = $env:TTA_TOAST_BODY
$escapedTitle = [System.Security.SecurityElement]::Escape($title)
$escapedBody = [System.Security.SecurityElement]::Escape($body)
$xml = @"
<toast activationType="background" scenario="reminder">
  <visual>
    <binding template="ToastGeneric">
      <text hint-maxLines="1">$escapedTitle</text>
      <text hint-maxLines="12">$escapedBody</text>
    </binding>
  </visual>
  <audio silent="true"/>
</toast>
"@
$doc = New-Object Windows.Data.Xml.Dom.XmlDocument
$doc.LoadXml($xml)
$toast = [Windows.UI.Notifications.ToastNotification]::new($doc)
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($AppId).Show($toast)
"#;
    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-WindowStyle",
        "Hidden",
        "-Sta",
        "-Command",
        script,
    ]);
    cmd.env("TTA_TOAST_TITLE", title);
    cmd.env("TTA_TOAST_BODY", body);
    cmd.creation_flags(CREATE_NO_WINDOW);
    let _ = cmd.spawn();
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
