// Claude Code セッションダッシュボード（読み取り専用）
mod iterm;
pub mod sessions;

use std::process::Command;
use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use sessions::{Sessions, Snapshot, Summary, Transcript};

type Shared<'a> = State<'a, Arc<Sessions>>;

const NOT_FOUND: &str = "セッションが見つかりません";

// JSONL の走査や osascript は時間がかかるので、IPC のスレッドを塞がないよう別スレッドで動かす
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct Detail {
    summary: Summary,
    command: Option<String>,
    #[serde(flatten)]
    transcript: Transcript,
}

#[tauri::command]
async fn get_sessions(state: Shared<'_>) -> Result<Snapshot, String> {
    let sessions = state.inner().clone();
    blocking(move || sessions.scan()).await
}

#[tauri::command]
async fn get_session(id: String, state: Shared<'_>) -> Result<Detail, String> {
    let sessions = state.inner().clone();
    blocking(move || {
        let (summary, file) = sessions.find(&id).ok_or(NOT_FOUND)?;
        let transcript = sessions::transcript(&file).map_err(|e| e.to_string())?;
        Ok(Detail { command: iterm::resume_command(&summary), summary, transcript })
    })
    .await?
}

#[tauri::command]
async fn resume_session(id: String, state: Shared<'_>) -> Result<(), String> {
    let sessions = state.inner().clone();
    blocking(move || iterm::resume(&sessions.find(&id).ok_or(NOT_FOUND)?.0)).await?
}

#[tauri::command]
async fn focus_session(id: String, state: Shared<'_>) -> Result<(), String> {
    let sessions = state.inner().clone();
    blocking(move || iterm::focus(&sessions.find(&id).ok_or(NOT_FOUND)?.0)).await?
}

// PR などの外部リンクは既定のブラウザで開く
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("https のリンクだけ開けます".into());
    }
    Command::new("/usr/bin/open").arg(&url).spawn().map(|_| ()).map_err(|e| e.to_string())
}

pub fn run() {
    tauri::Builder::default()
        .manage(Arc::new(Sessions::new()))
        .invoke_handler(tauri::generate_handler![get_sessions, get_session, resume_session, focus_session, open_url])
        .run(tauri::generate_context!())
        .expect("アプリの起動に失敗しました");
}
