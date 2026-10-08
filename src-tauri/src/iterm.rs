// iTerm2 連携（AppleScript 経由）
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::sessions::Summary;

// 初回は macOS の許可ダイアログに答える時間が要るので長めに待つ
const TIMEOUT: Duration = Duration::from_secs(60);

const RESUME_SCRIPT: &str = r#"on run argv
  set cmd to item 1 of argv
  tell application "iTerm"
    activate
    if (count of windows) = 0 then
      create window with default profile
    else
      tell current window to create tab with default profile
    end if
    tell current session of current window to write text cmd
  end tell
end run"#;

const FOCUS_SCRIPT: &str = r#"on run argv
  set target to item 1 of argv
  tell application "iTerm"
    repeat with w in windows
      repeat with t in tabs of w
        repeat with s in sessions of t
          if (tty of s) is target then
            select w
            select t
            select s
            activate
            return "ok"
          end if
        end repeat
      end repeat
    end repeat
  end tell
  return "notfound"
end run"#;

fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn osascript(script: &str, arg: &str) -> Result<String, String> {
    let mut child = Command::new("/usr/bin/osascript")
        .args(["-e", script, arg])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + TIMEOUT;
    while child.try_wait().map_err(|e| e.to_string())?.is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("iTerm2 から応答がありませんでした".into());
        }
        sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    // -1743: オートメーションの許可が拒否されている
    if stderr.contains("-1743") {
        return Err("iTerm2 を操作する許可がありません。システム設定 › プライバシーとセキュリティ › オートメーション で許可してください".into());
    }
    Err(if stderr.is_empty() { out.status.to_string() } else { stderr })
}

pub fn resume_command(summary: &Summary) -> Option<String> {
    let cwd = summary.cwd.as_deref()?;
    Some(format!("cd {} && claude --resume {}", shq(cwd), summary.id))
}

pub fn resume(summary: &Summary) -> Result<(), String> {
    let (Some(cwd), Some(command)) = (summary.cwd.as_deref(), resume_command(summary)) else {
        return Err("作業ディレクトリが分かりません".into());
    };
    // 権限不足で確認できない場合（デスクトップや書類フォルダ等）は止めずに iTerm2 側に任せる
    if let Ok(false) = Path::new(cwd).try_exists() {
        return Err(format!("ディレクトリがありません: {cwd}"));
    }
    osascript(RESUME_SCRIPT, &command).map(|_| ())
}

fn tty_of(pid: i32) -> Option<String> {
    let out = Command::new("/bin/ps").args(["-o", "tty=", "-p", &pid.to_string()]).output().ok()?;
    let tty = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !tty.is_empty() && tty != "??").then(|| format!("/dev/{tty}"))
}

pub fn focus(summary: &Summary) -> Result<(), String> {
    let live = summary.live.as_ref().ok_or("このセッションは稼働していません")?;
    let tty = tty_of(live.pid).ok_or("端末（tty）を特定できませんでした")?;
    if osascript(FOCUS_SCRIPT, &tty)? != "ok" {
        return Err("iTerm2 上に該当するタブが見つかりませんでした".into());
    }
    Ok(())
}
