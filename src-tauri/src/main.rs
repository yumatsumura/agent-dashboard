// リリースビルドではコンソールウィンドウを出さない（Windows 向け）
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    agent_dashboard_lib::run()
}
