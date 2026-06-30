#!/usr/bin/env python3
"""
corpus_server.py — web dashboard for typst-diff corpus tests.

Usage: python3 tests/corpus_server.py [--port 8787]
Then open http://localhost:8787
"""

import json
import re
import subprocess
import sys
import threading
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import List, Optional, Tuple
from urllib.parse import unquote, urlparse

SCRIPT_DIR = Path(__file__).parent.resolve()
CORPUS_DIR = SCRIPT_DIR / "corpus"
OUTPUT_DIR = SCRIPT_DIR / "corpus_output"
CARGO_TOML = SCRIPT_DIR.parent / "Cargo.toml"

# ── job registry ──────────────────────────────────────────────────────────────

_jobs: dict = {}
_lock = threading.Lock()


def start_job(cmd: list) -> str:
    jid = uuid.uuid4().hex[:8]
    e: dict = {"status": "running", "lines": []}
    with _lock:
        _jobs[jid] = e

    def _run() -> None:
        p = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
        assert p.stdout
        for line in iter(p.stdout.readline, ""):
            with _lock:
                e["lines"].append(line.rstrip())
        p.wait()
        with _lock:
            e["status"] = "done" if p.returncode == 0 else f"failed:{p.returncode}"

    threading.Thread(target=_run, daemon=True).start()
    return jid


def poll_job(jid: str) -> Optional[dict]:
    with _lock:
        j = _jobs.get(jid)
        return {"status": j["status"], "lines": list(j["lines"])} if j else None


# ── data ──────────────────────────────────────────────────────────────────────


def _parse_result(path: Path) -> dict:
    r: dict = {
        "status": "not_run",
        "elapsed_s": 0,
        "modifications": 0,
        "pdf_bytes": 0,
        "visual_status": "-",
        "failure_reason": "",
    }
    if not path.exists():
        return r
    for raw in path.read_text().splitlines():
        k, _, v = raw.partition(":")
        k, v = k.strip(), v.strip()
        if k == "status":
            r["status"] = v
        elif k == "elapsed_s":
            r["elapsed_s"] = int(v) if v.isdigit() else 0
        elif k == "modifications":
            r["modifications"] = int(v) if v.isdigit() else 0
        elif k == "pdf_bytes":
            r["pdf_bytes"] = int(v) if v.isdigit() else 0
        elif k == "visual_status":
            r["visual_status"] = v
        elif k == "failure_reason":
            r["failure_reason"] = v
    return r


def get_tests() -> List[dict]:
    tests = []

    def _sort_key(p: Path) -> int:
        m = re.match(r"^(\d+)", p.name)
        return int(m.group(1)) if m else 0

    for d in sorted(CORPUS_DIR.iterdir(), key=_sort_key):
        if not d.is_dir():
            continue
        name = d.name
        m = re.match(r"^(\d+)-", name)
        tests.append(
            {
                "id": int(m.group(1)) if m else 0,
                "name": name,
                **_parse_result(OUTPUT_DIR / name / "result.txt"),
            }
        )
    return tests


def do_compare(test_name: str) -> Tuple[Optional[bytes], Optional[str]]:
    m = re.match(r"^(\d+)", test_name)
    if not m:
        return None, "invalid test name"
    tid = m.group(1)
    out_png = Path(f"/tmp/compare-{test_name}.png")
    res = subprocess.run(
        ["bash", str(SCRIPT_DIR / "compare_corpus.sh"), tid, "--no-open"],
        capture_output=True,
        text=True,
        timeout=120,
    )
    if out_png.exists():
        return out_png.read_bytes(), None
    err = (res.stderr or res.stdout or "compare failed").strip()
    return None, err


# ── embedded HTML ─────────────────────────────────────────────────────────────

HTML = r"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>typst-diff corpus</title>
<style>
*,*::before,*::after{box-sizing:border-box;margin:0;padding:0}
body{font-family:system-ui,-apple-system,sans-serif;background:#11111b;color:#cdd6f4;min-height:100vh}

/* ── header ── */
header{background:#1e1e2e;padding:12px 20px;display:flex;align-items:center;gap:14px;position:sticky;top:0;z-index:50;box-shadow:0 2px 10px rgba(0,0,0,.5)}
header h1{font-size:15px;font-weight:700;letter-spacing:.4px;color:#cba6f7;flex:1}
.summary{display:flex;gap:10px}
.s{display:flex;align-items:center;gap:5px;padding:4px 11px;border-radius:20px;font-size:12px;font-weight:700}
.s.pass{background:rgba(166,227,161,.12);color:#a6e3a1}
.s.fail{background:rgba(243,139,168,.12);color:#f38ba8}
.s.new{background:rgba(137,180,250,.12);color:#89b4fa}
.s.nr{background:rgba(88,91,112,.18);color:#6c7086}
.hbtns{display:flex;gap:8px;align-items:center}

/* ── buttons ── */
.btn{border:none;cursor:pointer;font-size:13px;font-weight:600;border-radius:6px;padding:7px 15px;transition:.15s opacity}
.btn:hover{opacity:.82}.btn:disabled{opacity:.35;cursor:not-allowed}
.btn.primary{background:#cba6f7;color:#1e1e2e}
.btn.ghost{background:rgba(205,214,244,.08);color:#cdd6f4;border:1px solid rgba(205,214,244,.15)}
.btn.ghost:hover{background:rgba(205,214,244,.15)}
.btn.sm{padding:4px 10px;font-size:12px;border-radius:5px}
.btn.run{background:#89b4fa;color:#1e1e2e}
.btn.cmp{background:#cba6f7;color:#1e1e2e}
.btn.upd{background:#a6e3a1;color:#1e1e2e}

/* ── controls ── */
#controls{background:#181825;border-bottom:1px solid rgba(205,214,244,.08);padding:10px 20px;display:flex;gap:10px;align-items:center;flex-wrap:wrap}
#search{background:#313244;border:1px solid transparent;border-radius:6px;padding:7px 12px;font-size:13px;color:#cdd6f4;outline:none;width:260px}
#search:focus{border-color:#cba6f7}#search::placeholder{color:#45475a}
.fbtn{padding:5px 13px;border-radius:20px;border:1px solid rgba(205,214,244,.15);background:transparent;color:#7f849c;cursor:pointer;font-size:12px;font-weight:600;transition:.15s}
.fbtn:hover{color:#cdd6f4;border-color:#cba6f7}
.fbtn.active{border-color:transparent!important;color:#1e1e2e!important}
.fbtn.f-all.active{background:#cdd6f4}
.fbtn.f-pass.active{background:#a6e3a1}
.fbtn.f-fail.active{background:#f38ba8}
.fbtn.f-new.active{background:#89b4fa}
.fbtn.f-nr.active{background:#6c7086;color:#cdd6f4!important}
#vis-count{margin-left:auto;font-size:12px;color:#45475a}

/* ── table ── */
#wrap{padding:16px 20px 100px}
table{width:100%;border-collapse:collapse;background:#1e1e2e;border-radius:10px;overflow:hidden;box-shadow:0 2px 16px rgba(0,0,0,.4)}
th{padding:11px 14px;text-align:left;font-size:11px;text-transform:uppercase;letter-spacing:.07em;color:#45475a;border-bottom:1px solid #313244;user-select:none}
td{padding:9px 14px;font-size:13px;border-bottom:1px solid #181825;vertical-align:middle}
tr:last-child td{border-bottom:none}
tr:hover td{background:rgba(255,255,255,.03)}
.tc-id{color:#45475a;text-align:right;width:44px;font-variant-numeric:tabular-nums}
.tc-name{font-family:ui-monospace,monospace;font-size:12px;color:#cdd6f4}
.tc-num{text-align:right;width:60px;color:#585b70;font-variant-numeric:tabular-nums}
.tc-act{width:220px}

/* ── badges ── */
.badge{display:inline-block;padding:2px 9px;border-radius:12px;font-size:11px;font-weight:700;text-transform:uppercase;letter-spacing:.06em}
.badge.pass{background:rgba(166,227,161,.12);color:#a6e3a1}
.badge.fail{background:rgba(243,139,168,.12);color:#f38ba8}
.badge.new{background:rgba(137,180,250,.12);color:#89b4fa}
.badge.nr{background:rgba(88,91,112,.18);color:#585b70}
.badge.running{background:rgba(249,226,175,.12);color:#f9e2af}

/* ── log panel ── */
#log{position:fixed;bottom:0;left:0;right:0;height:270px;background:#11111b;color:#cdd6f4;font-family:ui-monospace,monospace;font-size:12px;display:flex;flex-direction:column;box-shadow:0 -4px 24px rgba(0,0,0,.6);transform:translateY(100%);transition:transform .25s ease;z-index:100}
#log.open{transform:translateY(0)}
#log-head{padding:8px 16px;background:#181825;display:flex;align-items:center;gap:10px;flex-shrink:0;border-bottom:1px solid rgba(205,214,244,.08)}
#log-title{flex:1;font-weight:700;font-size:13px;color:#cba6f7}
#log-badge{font-size:11px;padding:2px 9px;border-radius:10px;background:rgba(249,226,175,.12);color:#f9e2af}
#log-badge.done{background:rgba(166,227,161,.12);color:#a6e3a1}
#log-badge.fail{background:rgba(243,139,168,.12);color:#f38ba8}
#log-body{flex:1;overflow-y:auto;padding:10px 16px;line-height:1.7}
.ll{white-space:pre}
.ll.pass{color:#a6e3a1}.ll.fail{color:#f38ba8}.ll.new{color:#89b4fa}.ll.dim{color:#313244}

/* ── modal ── */
#overlay{display:none;position:fixed;inset:0;background:rgba(0,0,0,.8);z-index:200;align-items:flex-start;justify-content:center;padding:32px 16px;overflow-y:auto;cursor:pointer}
#overlay.open{display:flex}
#modal{background:#1e1e2e;border-radius:10px;overflow:hidden;max-width:100%;box-shadow:0 10px 48px rgba(0,0,0,.7);cursor:default}
#modal-head{padding:10px 16px;background:#181825;display:flex;align-items:center;gap:10px;border-bottom:1px solid #313244;position:sticky;top:0}
#modal-title{flex:1;font-size:13px;font-weight:600;color:#cba6f7;font-family:monospace}
#modal-body{padding:0}
#modal-img{display:block;max-width:100%}
#modal-info{padding:40px;text-align:center;color:#45475a;font-size:14px}
#modal-err{padding:32px;color:#f38ba8;font-size:13px;font-family:monospace;white-space:pre-wrap}
</style>
</head>
<body>
<header>
  <h1>typst-diff corpus</h1>
  <div class="summary">
    <span class="s pass" id="s-pass">—</span>
    <span class="s fail" id="s-fail">—</span>
    <span class="s new" id="s-new">—</span>
    <span class="s nr" id="s-nr">—</span>
  </div>
  <div class="hbtns">
    <button class="btn ghost sm" id="build-btn" onclick="rebuild()" title="cargo build --release">Build</button>
    <button class="btn primary" id="run-all-btn" onclick="runAll()">▶ Run All</button>
  </div>
</header>

<div id="controls">
  <input id="search" type="search" placeholder="Filter by name…" oninput="onSearch(this.value)" autocomplete="off">
  <button class="fbtn f-all active" onclick="setFilter('all')">All</button>
  <button class="fbtn f-pass" onclick="setFilter('pass')">Pass</button>
  <button class="fbtn f-fail" onclick="setFilter('fail')">Fail</button>
  <button class="fbtn f-new" onclick="setFilter('new')">New</button>
  <button class="fbtn f-nr" onclick="setFilter('nr')">Not run</button>
  <span id="vis-count"></span>
</div>

<div id="wrap">
  <table>
    <thead>
      <tr>
        <th class="tc-id">#</th>
        <th>Name</th>
        <th style="width:76px">Status</th>
        <th class="tc-num" title="Elapsed seconds">Time</th>
        <th class="tc-num" title="Modifications detected">Mods</th>
        <th class="tc-act">Actions</th>
      </tr>
    </thead>
    <tbody id="tbody"></tbody>
  </table>
</div>

<!-- log panel -->
<div id="log">
  <div id="log-head">
    <span id="log-title">Log</span>
    <span id="log-badge">running</span>
    <button class="btn ghost sm" onclick="closeLog()">✕</button>
  </div>
  <div id="log-body"></div>
</div>

<!-- compare modal -->
<div id="overlay" onclick="onOverlayClick(event)">
  <div id="modal">
    <div id="modal-head">
      <span id="modal-title"></span>
      <button class="btn ghost sm" onclick="closeModal()">✕ Close</button>
    </div>
    <div id="modal-body">
      <div id="modal-info">Loading comparison…</div>
      <div id="modal-err" style="display:none"></div>
      <img id="modal-img" style="display:none" alt="comparison">
    </div>
  </div>
</div>

<script>
'use strict';

let allTests = [], activeFilter = 'all', searchQ = '';
let pollTimer = null, seenLines = 0;

// ── data ──────────────────────────────────────────────────────────────────────

async function loadTests() {
  try {
    const r = await fetch('/api/tests');
    allTests = await r.json();
    renderSummary();
    renderTable();
  } catch(e) { /* ignore network hiccup */ }
}

function sc(status) {
  const s = (status || '').toLowerCase();
  if (s === 'pass') return 'pass';
  if (s === 'fail' || s.startsWith('fail')) return 'fail';
  if (s === 'new') return 'new';
  return 'nr';
}

function renderSummary() {
  const c = {pass:0, fail:0, new:0, nr:0};
  for (const t of allTests) { const k = sc(t.status); c[k] = (c[k]||0) + 1; }
  document.getElementById('s-pass').textContent = c.pass + ' pass';
  document.getElementById('s-fail').textContent = c.fail + ' fail';
  document.getElementById('s-new').textContent  = c.new  + ' new';
  document.getElementById('s-nr').textContent   = c.nr   + ' —';
}

function renderTable() {
  const q = searchQ.toLowerCase();
  const rows = allTests.filter(t => {
    if (q && !t.name.toLowerCase().includes(q)) return false;
    if (activeFilter === 'all') return true;
    return sc(t.status) === activeFilter;
  });
  document.getElementById('tbody').innerHTML = rows.map(t => {
    const cls = sc(t.status);
    const lbl = (t.status || 'not_run').toUpperCase().replace('NOT_RUN', '—');
    const time = t.elapsed_s ? t.elapsed_s + 's' : '—';
    const mods = t.elapsed_s ? String(t.modifications) : '—';
    return `<tr>
      <td class="tc-id">${t.id}</td>
      <td class="tc-name">${t.name}</td>
      <td><span class="badge ${cls}">${lbl}</span></td>
      <td class="tc-num">${time}</td>
      <td class="tc-num">${mods}</td>
      <td class="tc-act">
        <button class="btn sm run" onclick="runTest('${t.name}')">Rerun</button>
        <button class="btn sm cmp" onclick="showCompare('${t.name}')">Compare</button>
        <button class="btn sm upd" onclick="updateTest('${t.name}')">Update ref</button>
      </td>
    </tr>`;
  }).join('');
  document.getElementById('vis-count').textContent =
    rows.length === allTests.length ? allTests.length + ' tests'
                                    : rows.length + ' / ' + allTests.length;
}

// ── filters ───────────────────────────────────────────────────────────────────

function setFilter(f) {
  activeFilter = f;
  document.querySelectorAll('.fbtn').forEach(b => b.classList.remove('active'));
  document.querySelector('.fbtn.f-' + f).classList.add('active');
  renderTable();
}

function onSearch(v) { searchQ = v; renderTable(); }

// ── actions ───────────────────────────────────────────────────────────────────

async function rebuild() {
  const btn = document.getElementById('build-btn');
  btn.disabled = true;
  const r = await fetch('/api/build', {method: 'POST'});
  const {job_id} = await r.json();
  openLog('Building typst-diff (release)…', job_id, () => { btn.disabled = false; });
}

async function runAll() {
  const btn = document.getElementById('run-all-btn');
  btn.disabled = true;
  const r = await fetch('/api/run-all', {method: 'POST'});
  const {job_id} = await r.json();
  openLog('Running all corpus tests…', job_id, () => { btn.disabled = false; loadTests(); });
}

async function runTest(name) {
  const r = await fetch('/api/run/' + encodeURIComponent(name), {method: 'POST'});
  const {job_id} = await r.json();
  openLog('Running ' + name + '…', job_id, () => loadTests());
}

async function updateTest(name) {
  const r = await fetch('/api/update/' + encodeURIComponent(name), {method: 'POST'});
  if (!r.ok) {
    const msg = await r.text();
    alert('Update failed: ' + msg);
    return;
  }
  const {job_id} = await r.json();
  openLog('Updating ref for ' + name + '…', job_id, () => loadTests());
}


// ── log panel ─────────────────────────────────────────────────────────────────

function openLog(title, jid, onDone) {
  document.getElementById('log-title').textContent = title;
  document.getElementById('log-body').innerHTML = '';
  const badge = document.getElementById('log-badge');
  badge.textContent = 'running'; badge.className = '';
  document.getElementById('log').classList.add('open');
  seenLines = 0;
  if (pollTimer) { clearInterval(pollTimer); pollTimer = null; }
  pollTimer = setInterval(async () => {
    let job;
    try {
      const r = await fetch('/api/job/' + jid);
      if (!r.ok) return;
      job = await r.json();
    } catch(e) { return; }

    const body = document.getElementById('log-body');
    const newLines = job.lines.slice(seenLines);
    seenLines = job.lines.length;
    for (const line of newLines) {
      const d = document.createElement('div');
      d.className = 'll' +
        (line.match(/^PASS/) ? ' pass' :
         line.match(/^FAIL/) ? ' fail' :
         line.match(/^NEW /)  ? ' new'  :
         line.startsWith('──') ? ' dim'  : '');
      d.textContent = line;
      body.appendChild(d);
    }
    if (newLines.length) body.scrollTop = body.scrollHeight;

    if (job.status !== 'running') {
      clearInterval(pollTimer); pollTimer = null;
      badge.textContent = job.status;
      badge.className = job.status === 'done' ? 'done' : 'fail';
      if (onDone) onDone();
    }
  }, 300);
}

function closeLog() {
  document.getElementById('log').classList.remove('open');
  if (pollTimer) { clearInterval(pollTimer); pollTimer = null; }
}

// ── compare modal ─────────────────────────────────────────────────────────────

async function showCompare(name) {
  document.getElementById('modal-title').textContent = name;
  document.getElementById('modal-info').textContent = 'Loading comparison…';
  document.getElementById('modal-info').style.display = 'block';
  document.getElementById('modal-err').style.display = 'none';
  document.getElementById('modal-img').style.display = 'none';
  document.getElementById('overlay').classList.add('open');

  try {
    const r = await fetch('/api/compare/' + encodeURIComponent(name));
    if (!r.ok) {
      const msg = await r.text();
      document.getElementById('modal-info').style.display = 'none';
      const err = document.getElementById('modal-err');
      err.textContent = msg || 'compare failed';
      err.style.display = 'block';
      return;
    }
    const blob = await r.blob();
    const url = URL.createObjectURL(blob);
    const img = document.getElementById('modal-img');
    img.onload = () => {
      document.getElementById('modal-info').style.display = 'none';
      img.style.display = 'block';
    };
    img.src = url;
  } catch(e) {
    document.getElementById('modal-info').style.display = 'none';
    const err = document.getElementById('modal-err');
    err.textContent = String(e);
    err.style.display = 'block';
  }
}

function closeModal() {
  document.getElementById('overlay').classList.remove('open');
  const img = document.getElementById('modal-img');
  if (img.src && img.src.startsWith('blob:')) URL.revokeObjectURL(img.src);
  img.src = '';
  img.style.display = 'none';
}

function onOverlayClick(e) {
  if (e.target === document.getElementById('overlay')) closeModal();
}

// ── init ──────────────────────────────────────────────────────────────────────
loadTests();
</script>
</body>
</html>"""


# ── HTTP handler ──────────────────────────────────────────────────────────────


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_: object) -> None:
        pass

    def do_GET(self) -> None:
        p = unquote(urlparse(self.path).path)
        if p == "/":
            self._ok("text/html; charset=utf-8", HTML.encode())
        elif p == "/api/tests":
            self._json(get_tests())
        elif p.startswith("/api/job/"):
            j = poll_job(p[9:])
            if j:
                self._json(j)
            else:
                self._err(404)
        elif p.startswith("/api/compare/"):
            name = p[13:]
            data, err = do_compare(name)
            if data:
                self._ok("image/png", data)
            else:
                self._err(500, (err or "compare failed").encode())
        else:
            self._err(404)

    def do_POST(self) -> None:
        p = unquote(urlparse(self.path).path)
        if p == "/api/build":
            jid = start_job(
                [
                    "cargo",
                    "build",
                    "--release",
                    "--manifest-path",
                    str(CARGO_TOML),
                ]
            )
            self._json({"job_id": jid})
        elif p == "/api/run-all":
            jid = start_job(
                [
                    "bash",
                    str(SCRIPT_DIR / "run_corpus.sh"),
                    "--release",
                    "--no-build",
                ]
            )
            self._json({"job_id": jid})
        elif p.startswith("/api/run/"):
            name = p[9:]
            jid = start_job(
                [
                    "bash",
                    str(SCRIPT_DIR / "run_corpus.sh"),
                    "--release",
                    "--no-build",
                    "--filter",
                    name,
                ]
            )
            self._json({"job_id": jid})
        elif p.startswith("/api/update/"):
            name = p[12:]
            actual_dir = OUTPUT_DIR / name / "actual"
            ref_dir = CORPUS_DIR / name / "ref"
            actual_pngs = (
                sorted(actual_dir.glob("page-*.png")) if actual_dir.exists() else []
            )
            if not actual_pngs:
                self._err(400, b"no actual PNGs found; run the test first")
                return
            ref_dir.mkdir(parents=True, exist_ok=True)
            for f in actual_pngs:
                (ref_dir / f.name).write_bytes(f.read_bytes())
            jid = start_job(
                [
                    "bash",
                    str(SCRIPT_DIR / "run_corpus.sh"),
                    "--release",
                    "--no-build",
                    "--filter",
                    name,
                ]
            )
            self._json({"job_id": jid})
        else:
            self._err(404)

    def _ok(self, ct: str, body: bytes) -> None:
        self.send_response(200)
        self.send_header("Content-Type", ct)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def _json(self, data: object) -> None:
        self._ok("application/json", json.dumps(data).encode())

    def _err(self, code: int, body: bytes = b"") -> None:
        self.send_response(code)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


# ── main ──────────────────────────────────────────────────────────────────────


def main() -> None:
    port = 8787
    args = sys.argv[1:]
    for i, a in enumerate(args):
        if a in ("--port", "-p") and i + 1 < len(args):
            port = int(args[i + 1])

    srv = ThreadingHTTPServer(("", port), Handler)
    print(f"typst-diff corpus server → http://localhost:{port}")
    print("Ctrl+C to stop.")
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        print("\nStopped.")


if __name__ == "__main__":
    main()
