// ターミナルアプリ連携（AppleScript 経由）。iTerm2 と Ghostty に対応
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::sessions::Summary;

// 初回は macOS の許可ダイアログに答える時間が要るので長めに待つ
const TIMEOUT: Duration = Duration::from_secs(60);

// 画面の設定で選んだターミナルアプリ
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Terminal {
    Iterm,
    Ghostty,
}

impl Terminal {
    fn name(self) -> &'static str {
        match self {
            Terminal::Iterm => "iTerm2",
            Terminal::Ghostty => "Ghostty",
        }
    }
}

const ITERM_RESUME: &str = r#"on run argv
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

const ITERM_FOCUS: &str = r#"on run argv
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

const GHOSTTY_RESUME: &str = r#"on run argv
  set cmd to item 1 of argv
  tell application "Ghostty"
    activate
    set cfg to new surface configuration
    set initial input of cfg to cmd & linefeed
    if (count of windows) = 0 then
      new window with configuration cfg
    else
      new tab in front window with configuration cfg
    end if
  end tell
end run"#;

// Ghostty は tty を公開していないので作業ディレクトリで探す。
// 同じ場所のタブが複数あるときは、タイトルにセッション名を含むものを優先する
const GHOSTTY_FOCUS: &str = r#"on run argv
  set dir to item 1 of argv
  set title to item 2 of argv
  tell application "Ghostty"
    set found to ""
    repeat with t in terminals
      if (working directory of t) is dir then
        if found is "" then set found to id of t
        if title is not "" and (name of t) contains title then
          set found to id of t
          exit repeat
        end if
      end if
    end repeat
    if found is "" then return "notfound"
    focus (first terminal whose id is found)
    activate
  end tell
  return "ok"
end run"#;

fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn osascript(app: Terminal, script: &str, args: &[&str]) -> Result<String, String> {
    let mut child = Command::new("/usr/bin/osascript")
        .args(["-e", script])
        .args(args)
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
            return Err(format!("{} から応答がありませんでした", app.name()));
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
        return Err(format!(
            "{} を操作する許可がありません。システム設定 › プライバシーとセキュリティ › オートメーション で許可してください",
            app.name()
        ));
    }
    Err(if stderr.is_empty() { out.status.to_string() } else { stderr })
}

pub fn resume_command(summary: &Summary) -> Option<String> {
    let cwd = summary.cwd.as_deref()?;
    Some(format!("cd {} && claude --resume {}", shq(cwd), summary.id))
}

pub fn resume(app: Terminal, summary: &Summary) -> Result<(), String> {
    let (Some(cwd), Some(command)) = (summary.cwd.as_deref(), resume_command(summary)) else {
        return Err("作業ディレクトリが分かりません".into());
    };
    // 権限不足で確認できない場合（デスクトップや書類フォルダ等）は止めずにターミナル側に任せる
    if let Ok(false) = Path::new(cwd).try_exists() {
        return Err(format!("ディレクトリがありません: {cwd}"));
    }
    let script = match app {
        Terminal::Iterm => ITERM_RESUME,
        Terminal::Ghostty => GHOSTTY_RESUME,
    };
    osascript(app, script, &[&command]).map(|_| ())
}

fn tty_of(pid: i32) -> Option<String> {
    let out = Command::new("/bin/ps").args(["-o", "tty=", "-p", &pid.to_string()]).output().ok()?;
    let tty = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !tty.is_empty() && tty != "??").then(|| format!("/dev/{tty}"))
}

pub fn focus(app: Terminal, summary: &Summary) -> Result<(), String> {
    let live = summary.live.as_ref().ok_or("このセッションは稼働していません")?;
    let result = match app {
        Terminal::Iterm => {
            let tty = tty_of(live.pid).ok_or("端末（tty）を特定できませんでした")?;
            osascript(app, ITERM_FOCUS, &[&tty])?
        }
        Terminal::Ghostty => {
            let cwd = summary.cwd.as_deref().ok_or("作業ディレクトリが分かりません")?;
            osascript(app, GHOSTTY_FOCUS, &[cwd, summary.title.as_deref().unwrap_or("")])?
        }
    };
    if result != "ok" {
        return Err(format!("{} 上に該当するタブが見つかりませんでした", app.name()));
    }
    Ok(())
}
