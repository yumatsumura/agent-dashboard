# agent-dashboard

Claude Code のセッションを一覧できるローカルダッシュボード（読み取り専用・依存パッケージなし）。

## 使い方

```sh
pnpm start            # または node server.mjs
node server.mjs --open  # 起動してブラウザも開く
```

http://localhost:4317 を開く。ポートは `PORT=xxxx` で変更できる。

## 必要なもの

- Node 20 以上
- Claude Code（その PC の `~/.claude` を読む。場所を変える場合は `CLAUDE_CONFIG_DIR`）
- iTerm2（macOS）: 「iTerm2 で再開」「iTerm2 で表示」に使う。なくても一覧・詳細の閲覧と再開コマンドのコピーは使える

## 構成

- `server.mjs`: Node 標準モジュールだけのサーバー。`127.0.0.1` でのみ待ち受ける
- `public/index.html`: 画面（バニラ JS/CSS の単一ファイル）
