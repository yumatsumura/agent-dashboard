# agent-dashboard

Claude Code のセッションを一覧できる macOS 用デスクトップアプリ（Tauri 製・読み取り専用）。

## 使い方

```sh
pnpm install
pnpm build   # src-tauri/target/release/bundle/macos/AgentDashboard.app ができる
```

できた `AgentDashboard.app` を `/Applications` に置いて起動する。

開発中は `pnpm dev` でそのままウィンドウが開く。

## 必要なもの

- ビルド時のみ: Rust、Node 20 以上、pnpm、Xcode Command Line Tools（アプリの実行には不要）
- Claude Code（その PC の `~/.claude` を読む。場所を変える場合は `CLAUDE_CONFIG_DIR`）
- iTerm2 か Ghostty（1.3 以降）: セッションの再開・表示に使う。どちらを使うかは画面右上の「設定」で選ぶ（既定は iTerm2）。なくても一覧・詳細の閲覧と再開コマンドのコピーは使える。初回は macOS がターミナルアプリの操作許可を尋ねる
  - Ghostty は tty を公開していないため、「表示」は作業ディレクトリ（同じ場所に複数あればタブのタイトル）でタブを探す

## 構成

- `src-tauri/src/sessions.rs`: `~/.claude` のセッション記録（JSONL）の読み取りと要約
- `src-tauri/src/terminal.rs`: ターミナルアプリ連携（AppleScript。iTerm2・Ghostty）
- `src-tauri/src/lib.rs`: 画面から呼ぶ Tauri コマンド
- `public/index.html`: 画面（バニラ JS/CSS の単一ファイル。ビルド工程なし）
- `src-tauri/app-icon.svg`: アイコンの元データ（`pnpm tauri icon src-tauri/app-icon.svg` で再生成）
