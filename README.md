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
- iTerm2: 「iTerm2 で再開」「iTerm2 で表示」に使う。なくても一覧・詳細の閲覧と再開コマンドのコピーは使える。初回は macOS が iTerm2 の操作許可を尋ねる

## 構成

- `src-tauri/src/sessions.rs`: `~/.claude` のセッション記録（JSONL）の読み取りと要約
- `src-tauri/src/iterm.rs`: iTerm2 連携（AppleScript）
- `src-tauri/src/lib.rs`: 画面から呼ぶ Tauri コマンド
- `public/index.html`: 画面（バニラ JS/CSS の単一ファイル。ビルド工程なし）
- `src-tauri/app-icon.svg`: アイコンの元データ（`pnpm tauri icon src-tauri/app-icon.svg` で再生成）
