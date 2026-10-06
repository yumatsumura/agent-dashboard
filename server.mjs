#!/usr/bin/env node
// Claude Code セッションダッシュボード（読み取り専用・依存ゼロ）
import http from 'node:http';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import readline from 'node:readline';
import crypto from 'node:crypto';
import { execFile } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const PORT = Number(process.env.PORT) || 4317;
const HOST = '127.0.0.1';
const CLAUDE_DIR = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude');
const PROJECTS_DIR = path.join(CLAUDE_DIR, 'projects');
const LIVE_DIR = path.join(CLAUDE_DIR, 'sessions');
const INDEX_HTML = path.join(path.dirname(fileURLToPath(import.meta.url)), 'public', 'index.html');

// ページに埋め込み、副作用のある POST で照合する（他サイトからの CSRF 対策）
const TOKEN = crypto.randomBytes(24).toString('hex');
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const SNIPPET = 300;

// ---------- JSONL の解析 ----------

function textOf(content) {
  if (typeof content === 'string') return content;
  if (Array.isArray(content)) {
    return content.filter((c) => c?.type === 'text').map((c) => c.text).join('\n');
  }
  return '';
}

// ツール結果・メタ・スラッシュコマンド等を除いた「人が打ったプロンプト」だけを返す
function humanPrompt(r) {
  if (r.type !== 'user' || r.isMeta || r.isSidechain || r.isCompactSummary) return null;
  const content = r.message?.content;
  if (Array.isArray(content) && content.some((c) => c?.type === 'tool_result')) return null;
  const text = textOf(content).trim();
  if (!text || text.startsWith('<') || text.startsWith('[Request interrupted')) return null;
  return text;
}

function clip(text, n = SNIPPET) {
  const t = text.replace(/\s+/g, ' ').trim();
  return t.length > n ? t.slice(0, n) + '…' : t;
}

async function eachRecord(file, fn) {
  const rl = readline.createInterface({ input: fs.createReadStream(file), crlfDelay: Infinity });
  for await (const line of rl) {
    if (!line) continue;
    let r;
    try {
      r = JSON.parse(line);
    } catch {
      continue; // 書き込み途中の行など
    }
    fn(r);
  }
}

async function summarize(file, stat) {
  const s = {
    id: path.basename(file, '.jsonl'),
    projectDir: path.basename(path.dirname(file)),
    title: null,
    firstPrompt: null,
    lastPrompt: null,
    cwd: null,
    branch: null,
    startedAt: null,
    updatedAt: null,
    prompts: 0,
    turns: 0,
    model: null,
    contextTokens: null,
    costUSD: null,
    linesAdded: null,
    linesRemoved: null,
    version: null,
    entrypoint: null,
    prUrl: null,
    size: stat.size,
  };
  let aiTitle = null;
  let customTitle = null;
  let lastHuman = null;
  let lastPromptRecord = null;

  await eachRecord(file, (r) => {
    switch (r.type) {
      case 'ai-title':
        aiTitle = r.aiTitle || aiTitle;
        return;
      case 'custom-title':
        customTitle = r.customTitle || r.title || customTitle;
        return;
      case 'last-prompt':
        lastPromptRecord = r.lastPrompt || lastPromptRecord;
        return;
      case 'cost-state':
        s.costUSD = r.totalCostUSD ?? s.costUSD;
        s.linesAdded = r.totalLinesAdded ?? s.linesAdded;
        s.linesRemoved = r.totalLinesRemoved ?? s.linesRemoved;
        return;
      case 'pr-link':
        s.prUrl = r.prUrl || r.url || s.prUrl;
        return;
    }
    if (r.isSidechain) return;
    if (r.timestamp) {
      const t = Date.parse(r.timestamp);
      if (!Number.isNaN(t)) {
        if (s.startedAt === null) s.startedAt = t;
        s.updatedAt = t;
      }
    }
    if (r.cwd && !s.cwd) s.cwd = r.cwd; // 再開には開始時の cwd が必要
    if (r.gitBranch) s.branch = r.gitBranch === 'HEAD' ? null : r.gitBranch; // git 管理外は "HEAD" になる
    if (r.version) s.version = r.version;
    if (r.entrypoint) s.entrypoint = r.entrypoint;

    if (r.type === 'user') {
      const text = humanPrompt(r);
      if (text) {
        s.prompts++;
        if (!s.firstPrompt) s.firstPrompt = clip(text);
        lastHuman = text;
      }
    } else if (r.type === 'assistant') {
      s.turns++;
      const m = r.message;
      if (m?.model && m.model !== '<synthetic>') s.model = m.model;
      const u = m?.usage;
      if (u) {
        const ctx = (u.input_tokens || 0) + (u.cache_read_input_tokens || 0) + (u.cache_creation_input_tokens || 0);
        if (ctx) s.contextTokens = ctx;
      }
    }
  });

  s.lastPrompt = clip(lastPromptRecord || lastHuman || '') || null;
  s.title = customTitle || aiTitle || null;
  if (s.updatedAt === null) s.updatedAt = stat.mtimeMs;
  if (s.startedAt === null) s.startedAt = stat.mtimeMs;
  return s;
}

// ---------- セッション一覧（mtime+size でキャッシュ） ----------

const cache = new Map(); // file -> { mtimeMs, size, summary }

async function listSessionFiles() {
  const files = [];
  let dirs = [];
  try {
    dirs = await fsp.readdir(PROJECTS_DIR, { withFileTypes: true });
  } catch {
    return files;
  }
  for (const d of dirs) {
    if (!d.isDirectory()) continue;
    const dir = path.join(PROJECTS_DIR, d.name);
    for (const name of await fsp.readdir(dir).catch(() => [])) {
      if (name.endsWith('.jsonl') && UUID_RE.test(name.slice(0, -6))) files.push(path.join(dir, name));
    }
  }
  return files;
}

async function pool(items, limit, fn) {
  const out = new Array(items.length);
  let i = 0;
  await Promise.all(
    Array.from({ length: Math.min(limit, items.length) }, async () => {
      while (i < items.length) {
        const idx = i++;
        out[idx] = await fn(items[idx]);
      }
    }),
  );
  return out;
}

function pidAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return e.code === 'EPERM';
  }
}

async function liveSessions() {
  const live = new Map(); // sessionId -> info
  for (const name of await fsp.readdir(LIVE_DIR).catch(() => [])) {
    if (!name.endsWith('.json')) continue;
    try {
      const j = JSON.parse(await fsp.readFile(path.join(LIVE_DIR, name), 'utf8'));
      if (!j.sessionId || !j.pid || !pidAlive(j.pid)) continue;
      live.set(j.sessionId, {
        pid: j.pid,
        status: j.status || 'idle',
        name: j.name || null,
        kind: j.kind || null,
        startedAt: j.startedAt || null,
        statusUpdatedAt: j.statusUpdatedAt || j.updatedAt || null,
      });
    } catch {
      // 壊れた/書き込み途中のファイルは無視
    }
  }
  return live;
}

let scanning = null;
async function scan() {
  const files = await listSessionFiles();
  const seen = new Set(files);
  for (const k of cache.keys()) if (!seen.has(k)) cache.delete(k);

  const summaries = await pool(files, 6, async (file) => {
    try {
      const stat = await fsp.stat(file);
      const hit = cache.get(file);
      if (hit && hit.mtimeMs === stat.mtimeMs && hit.size === stat.size) return hit.summary;
      const summary = await summarize(file, stat);
      cache.set(file, { mtimeMs: stat.mtimeMs, size: stat.size, summary });
      return summary;
    } catch {
      return null;
    }
  });

  const live = await liveSessions();
  const sessions = summaries
    .filter((s) => s && (s.prompts > 0 || s.turns > 0 || live.has(s.id)))
    .map((s) => ({ ...s, live: live.get(s.id) || null }))
    .sort((a, b) => b.updatedAt - a.updatedAt);
  return { now: Date.now(), home: os.homedir(), sessions };
}

function getSessions() {
  // ポーリングが重なっても走査は 1 本にまとめる
  scanning ??= scan().finally(() => (scanning = null));
  return scanning;
}

async function findSession(id) {
  if (!UUID_RE.test(id)) return null;
  const { sessions } = await getSessions();
  const summary = sessions.find((s) => s.id === id);
  if (!summary) return null;
  return { summary, file: path.join(PROJECTS_DIR, summary.projectDir, id + '.jsonl') };
}

// ---------- 詳細（会話の抜粋） ----------

const MAX_ITEMS = 400;
const MAX_TEXT = 6000;

function toolBrief(input) {
  if (!input || typeof input !== 'object') return '';
  const v = input.command ?? input.file_path ?? input.pattern ?? input.description ?? input.query ?? input.url ?? input.skill ?? input.prompt ?? '';
  return typeof v === 'string' ? clip(v, 160) : '';
}

async function transcript(file) {
  const items = [];
  await eachRecord(file, (r) => {
    if (r.isSidechain) return;
    if (r.type === 'user') {
      const text = humanPrompt(r);
      if (text) items.push({ role: 'user', ts: r.timestamp, text: text.slice(0, MAX_TEXT) });
    } else if (r.type === 'assistant') {
      for (const c of r.message?.content || []) {
        if (c.type === 'text' && c.text?.trim()) {
          items.push({ role: 'assistant', ts: r.timestamp, text: c.text.slice(0, MAX_TEXT) });
        } else if (c.type === 'tool_use') {
          items.push({ role: 'tool', ts: r.timestamp, name: c.name, text: toolBrief(c.input) });
        }
      }
    }
  });
  const total = items.length;
  return { total, items: items.slice(-MAX_ITEMS) };
}

// ---------- iTerm2 連携 ----------

const shq = (s) => `'${String(s).replace(/'/g, `'\\''`)}'`;

function osascript(script, args) {
  return new Promise((resolve, reject) => {
    execFile('osascript', ['-e', script, ...args], { timeout: 15000 }, (err, stdout, stderr) => {
      if (err) reject(new Error((stderr || err.message).trim()));
      else resolve(stdout.trim());
    });
  });
}

const RESUME_SCRIPT = `on run argv
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
end run`;

const FOCUS_SCRIPT = `on run argv
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
end run`;

function resumeCommand(summary) {
  return `cd ${shq(summary.cwd)} && claude --resume ${summary.id}`;
}

async function resumeInITerm(summary) {
  if (!summary.cwd) throw new Error('作業ディレクトリが分かりません');
  if (!fs.existsSync(summary.cwd)) throw new Error(`ディレクトリがありません: ${summary.cwd}`);
  await osascript(RESUME_SCRIPT, [resumeCommand(summary)]);
}

function ttyOf(pid) {
  return new Promise((resolve) => {
    execFile('ps', ['-o', 'tty=', '-p', String(pid)], (err, stdout) => {
      const tty = (stdout || '').trim();
      resolve(err || !tty || tty === '??' ? null : '/dev/' + tty);
    });
  });
}

async function focusInITerm(summary) {
  if (!summary.live) throw new Error('このセッションは稼働していません');
  const tty = await ttyOf(summary.live.pid);
  if (!tty) throw new Error('端末（tty）を特定できませんでした');
  const result = await osascript(FOCUS_SCRIPT, [tty]);
  if (result !== 'ok') throw new Error('iTerm2 上に該当するタブが見つかりませんでした');
}

// ---------- HTTP ----------

function send(res, status, body, type = 'application/json; charset=utf-8') {
  const data = typeof body === 'string' || Buffer.isBuffer(body) ? body : JSON.stringify(body);
  res.writeHead(status, { 'Content-Type': type, 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff' });
  res.end(data);
}

function readJson(req) {
  return new Promise((resolve, reject) => {
    let buf = '';
    req.on('data', (c) => {
      buf += c;
      if (buf.length > 1e5) reject(new Error('too large'));
    });
    req.on('end', () => {
      try {
        resolve(JSON.parse(buf || '{}'));
      } catch (e) {
        reject(e);
      }
    });
    req.on('error', reject);
  });
}

const ALLOWED_HOSTS = new Set([`localhost:${PORT}`, `127.0.0.1:${PORT}`]);

const server = http.createServer(async (req, res) => {
  try {
    // DNS リバインディング対策: ローカルのホスト名以外は拒否
    if (!ALLOWED_HOSTS.has(req.headers.host)) return send(res, 403, { error: 'forbidden host' });
    const url = new URL(req.url, `http://${req.headers.host}`);

    if (req.method === 'GET' && url.pathname === '/') {
      const html = await fsp.readFile(INDEX_HTML, 'utf8');
      return send(res, 200, html.replace('__TOKEN__', TOKEN), 'text/html; charset=utf-8');
    }

    if (req.method === 'GET' && url.pathname === '/api/sessions') {
      return send(res, 200, await getSessions());
    }

    const m = url.pathname.match(/^\/api\/sessions\/([0-9a-f-]+)$/);
    if (req.method === 'GET' && m) {
      const found = await findSession(m[1]);
      if (!found) return send(res, 404, { error: 'セッションが見つかりません' });
      const t = await transcript(found.file);
      return send(res, 200, { summary: found.summary, command: found.summary.cwd ? resumeCommand(found.summary) : null, ...t });
    }

    if (req.method === 'POST' && (url.pathname === '/api/resume' || url.pathname === '/api/focus')) {
      if (req.headers['x-token'] !== TOKEN) return send(res, 403, { error: 'invalid token' });
      const body = await readJson(req);
      const found = await findSession(String(body.id || ''));
      if (!found) return send(res, 404, { error: 'セッションが見つかりません' });
      try {
        if (url.pathname === '/api/resume') await resumeInITerm(found.summary);
        else await focusInITerm(found.summary);
        return send(res, 200, { ok: true });
      } catch (e) {
        return send(res, 500, { error: e.message });
      }
    }

    send(res, 404, { error: 'not found' });
  } catch (e) {
    send(res, 500, { error: e.message });
  }
});

server.on('error', (e) => {
  if (e.code === 'EADDRINUSE') {
    console.error(`ポート ${PORT} は使用中です。PORT=xxxx を指定して起動してください。`);
    process.exit(1);
  }
  throw e;
});

server.listen(PORT, HOST, () => {
  const url = `http://localhost:${PORT}`;
  console.log(`Claude Code ダッシュボード: ${url}`);
  console.log(`読み取り元: ${PROJECTS_DIR}`);
  if (process.argv.includes('--open')) execFile('open', [url]);
});
