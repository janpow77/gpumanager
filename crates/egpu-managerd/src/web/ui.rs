/// The embedded HTML UI served at `/`.
/// Self-contained: no external dependencies, no CDN, no frameworks.
/// Visual style: Pipeline flow visualization with animated data flow,
/// GPU PCB cards with fans, Thunderbolt indicators, NVIDIA green scheme.
pub const INDEX_HTML: &str = r##"<!DOCTYPE html>
<html lang="de">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>eGPU Manager · Pipeline</title>
<style>
*{box-sizing:border-box;margin:0;padding:0}
:root{
  --nv:#76b900;--nv-glow:rgba(118,185,0,.35);
  --tb:#00b0f0;--tb-glow:rgba(0,176,240,.3);
  --amber:#f59e0b;--amber-glow:rgba(245,158,11,.3);
  --red:#ef4444;--warm:#f97316;--purple:#a855f7;
  --bg:#1a1a18;--bg-card:#2a2a27;--bg-deep:#1e1e1c;--border:#444440;
  --text:#e8e7e0;--muted:#9c9a92;
}
@media(prefers-color-scheme:light){
  :root{--bg:#fff;--bg-card:#f5f4f0;--bg-deep:#e8e7e2;--border:#c8c7c0;--text:#1a1a18;--muted:#6b6b66}
}
body{background:var(--bg);display:flex;align-items:flex-start;justify-content:center;
  min-height:100vh;font-family:'Segoe UI',system-ui,-apple-system,sans-serif;padding:24px 16px}
.root{width:100%;max-width:1100px;color:var(--text)}
.mono{font-family:'JetBrains Mono','Cascadia Code','Fira Code',monospace}

/* ── HEADER ─────────────────────────────────────────────── */
.pipeline-title{text-align:center;font-size:13px;font-weight:600;letter-spacing:.12em;
  text-transform:uppercase;color:var(--muted);margin-bottom:4px}
.live-bar{display:flex;align-items:center;justify-content:center;gap:8px;margin-bottom:18px;
  font-size:10px;color:var(--muted)}
.live-dot{width:6px;height:6px;border-radius:50%;background:var(--nv);
  animation:blink-nv .8s ease-in-out infinite}
.off{background:var(--muted)!important;animation:none!important}
@keyframes blink-nv{0%,100%{opacity:1;box-shadow:0 0 5px var(--nv-glow)}50%{opacity:.2;box-shadow:none}}

.warn-banner{text-align:center;padding:6px 12px;border-radius:6px;font-size:11px;font-weight:600;
  margin-bottom:12px;display:none}
.warn-banner.yellow{display:block;background:rgba(245,158,11,.15);color:var(--amber);border:1px solid rgba(245,158,11,.3)}
.warn-banner.orange{display:block;background:rgba(249,115,22,.15);color:var(--warm);border:1px solid rgba(249,115,22,.3)}
.warn-banner.red{display:block;background:rgba(239,68,68,.15);color:var(--red);border:1px solid rgba(239,68,68,.3)}

/* ── PIPELINE FLOW ──────────────────────────────────────── */
.pipeline{display:flex;align-items:center;justify-content:center;flex-wrap:nowrap;
  overflow-x:auto;padding:8px;gap:0}
.stage{display:flex;flex-direction:column;align-items:center;gap:6px;flex-shrink:0}
.stage-box{background:var(--bg-card);border:.5px solid var(--border);border-radius:10px;
  padding:10px 12px;display:flex;flex-direction:column;align-items:center;gap:5px;
  min-width:76px;transition:border-color .3s,opacity .3s;cursor:default}
.stage-box:hover{border-color:#888}
.stage-box.active{border-color:rgba(118,185,0,.5)}
.stage-box.idle{opacity:.5}
.stage-icon{font-size:22px;line-height:1}
.stage-icon svg{width:22px;height:22px}
.stage-lbl{font-size:10px;font-weight:600;text-align:center;color:var(--muted);
  letter-spacing:.04em;max-width:80px;line-height:1.3}
.stage-status{font-size:8px;color:var(--nv);letter-spacing:.04em}
.stage-status.off{color:var(--muted)}
.sub{font-size:9px;color:var(--muted)}

/* Connection lines with animated dots */
.conn{display:flex;align-items:center;flex-shrink:0;width:28px}
.conn-line{width:100%;height:1.5px;background:var(--border);position:relative;overflow:visible}
.dot{width:5px;height:5px;border-radius:50%;background:var(--tb);position:absolute;
  top:50%;transform:translateY(-50%);animation:flow 2s linear infinite;opacity:0}
.dot:nth-child(1){animation-delay:0s}.dot:nth-child(2){animation-delay:.7s}.dot:nth-child(3){animation-delay:1.4s}
.conn.paused .dot{animation:none;opacity:0}
@keyframes flow{0%{left:-6px;opacity:0}10%{opacity:1}90%{opacity:1}100%{left:calc(100% + 6px);opacity:0}}

/* NUC box */
.nuc-box{background:var(--bg-card);border:.5px solid rgba(245,158,11,.4);border-radius:12px;
  padding:12px 14px;display:flex;flex-direction:column;align-items:center;gap:6px}
.nuc-chip{width:44px;height:44px;border-radius:8px;border:1.5px solid var(--amber);
  display:flex;align-items:center;justify-content:center;position:relative;background:var(--bg-deep)}
.nuc-chip::before{content:'';position:absolute;inset:-4px;border-radius:11px;border:.5px dashed rgba(245,158,11,.3)}
.nuc-badge{font-size:8px;font-weight:600;color:var(--amber);letter-spacing:.06em}
.nuc-led{width:5px;height:5px;border-radius:50%;background:var(--amber);
  animation:b-amb 2s ease-in-out infinite;box-shadow:0 0 4px var(--amber-glow)}
@keyframes b-amb{0%,100%{opacity:1}50%{opacity:.3}}

/* GPU cards */
.gpu-section{display:flex;flex-direction:column;gap:10px;flex-shrink:0}
.gpu-card{background:var(--bg-card);border-radius:10px;padding:10px 12px;display:flex;
  flex-direction:column;gap:8px;min-width:240px;border:.5px solid rgba(118,185,0,.5);transition:border-color .3s}
.gpu-card.egpu{border-color:rgba(0,176,240,.3)}
.gpu-card.remote{border-color:rgba(168,85,247,.3)}
.gpu-card.offline{opacity:.4;border-color:var(--border)}
.gpu-top{display:flex;align-items:center;gap:10px}
.gpu-pcb{width:52px;height:36px;border-radius:4px;background:var(--bg-deep);
  border:1px solid rgba(118,185,0,.3);position:relative;flex-shrink:0;overflow:hidden}
.gpu-pcb::after{content:'';position:absolute;bottom:0;left:0;right:0;height:6px;
  background:rgba(118,185,0,.15);border-top:.5px solid rgba(118,185,0,.3)}
.gpu-card.egpu .gpu-pcb{border-color:rgba(0,176,240,.35)}
.fan{width:14px;height:14px;border-radius:50%;border:1.5px solid rgba(118,185,0,.5);position:absolute;top:5px}
.fan:nth-child(1){left:5px;animation:spin 2s linear infinite}
.fan:nth-child(2){right:5px;animation:spin 2s linear infinite reverse}
.fan::before{content:'';position:absolute;inset:2px;border-radius:50%;
  border-top:1.5px solid var(--nv);border-bottom:1.5px solid transparent;
  border-left:1.5px solid transparent;border-right:1.5px solid transparent}
.gpu-card.offline .fan{animation:none}
@keyframes spin{to{transform:rotate(360deg)}}
@media(prefers-reduced-motion:reduce){.fan{animation:none!important}.dot{animation:none!important;opacity:0!important}}

.gpu-inf{flex:1;min-width:0}
.gpu-brand{font-size:8px;font-weight:600;color:var(--nv);letter-spacing:.1em;text-transform:uppercase}
.gpu-model{font-size:11px;font-weight:700;color:var(--text);line-height:1.2;margin:2px 0}
.gpu-spec{font-size:9px;color:var(--muted)}
.vram-bar{width:100%;height:3px;background:var(--bg-deep);border-radius:2px;overflow:hidden;margin-top:3px}
.vram-fill{height:100%;border-radius:2px;background:linear-gradient(90deg,var(--nv),rgba(118,185,0,.5));transition:width .6s ease}
.vram-lbl{font-size:8px;color:var(--muted);margin-top:1px}
.g-led{width:6px;height:6px;border-radius:50%;background:var(--nv);flex-shrink:0}
.g-led.fast{animation:b-nv .6s ease-in-out infinite;box-shadow:0 0 6px var(--nv-glow)}
.g-led.slow{animation:b-nv 1.4s ease-in-out infinite}
.g-led.off{background:var(--muted);animation:none;box-shadow:none}
@keyframes b-nv{0%,100%{opacity:1;box-shadow:0 0 8px var(--nv-glow)}50%{opacity:.2;box-shadow:none}}
.g-status{display:flex;align-items:center;gap:4px;margin-top:4px}
.g-stxt{font-size:9px;color:var(--nv);letter-spacing:.04em}
.g-stxt.off{color:var(--muted)}
.egpu-badge{font-size:8px;font-weight:600;background:rgba(0,176,240,.12);color:var(--tb);
  border:.5px solid rgba(0,176,240,.35);border-radius:4px;padding:1px 5px;letter-spacing:.06em}
.remote-badge{font-size:8px;font-weight:600;background:rgba(168,85,247,.12);color:var(--purple);
  border:.5px solid rgba(168,85,247,.35);border-radius:4px;padding:1px 5px;letter-spacing:.06em}
.tb-conn{display:flex;align-items:center;gap:3px}
.tb-line{width:10px;height:1.5px;background:linear-gradient(90deg,var(--border),var(--tb));position:relative;overflow:visible}
.tb-dot{width:4px;height:4px;border-radius:50%;background:var(--tb);position:absolute;
  top:50%;transform:translateY(-50%);animation:tb 1.2s linear infinite;opacity:0}
.tb-dot:nth-child(1){animation-delay:0s}.tb-dot:nth-child(2){animation-delay:.6s}
@keyframes tb{0%{left:-4px;opacity:0}15%{opacity:.9}85%{opacity:.9}100%{left:calc(100% + 4px);opacity:0}}

/* GPU stats row */
.s-row{display:grid;grid-template-columns:1fr 1fr 1fr;gap:6px;border-top:.5px solid var(--border);padding-top:7px}
.s-cell{display:flex;flex-direction:column;gap:2px}
.s-lbl{font-size:8px;color:var(--muted);letter-spacing:.04em;text-transform:uppercase}
.s-val{font-size:12px;font-weight:600;color:var(--text);transition:color .4s}
.s-bar{height:2px;border-radius:1px;background:var(--bg-deep);overflow:hidden;margin-top:2px}
.s-fill{height:100%;border-radius:1px;background:var(--nv);transition:width .6s ease;width:0}
.s-fill.pwr{background:var(--tb)}.s-fill.tmp{background:var(--amber)}
.c-ok{color:var(--nv)}.c-warm{color:var(--warm)}.c-hot{color:var(--red)}

/* Split / merge wires */
.split{display:flex;align-items:center;position:relative;width:40px;flex-shrink:0}
.merge{display:flex;align-items:center;position:relative;width:32px;flex-shrink:0}

/* ── PIPELINE DETAIL CARDS (below) ──────────────────────── */
.detail-section{margin-top:20px}
.detail-title{font-size:10px;font-weight:600;letter-spacing:.1em;text-transform:uppercase;
  color:var(--muted);margin-bottom:10px;display:flex;align-items:center;gap:8px}
.detail-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(280px,1fr));gap:10px}
.detail-card{background:var(--bg-card);border:.5px solid var(--border);border-radius:10px;
  padding:12px 14px;transition:border-color .3s}
.detail-card:hover{border-color:#888}
.detail-card.egpu{border-left:3px solid var(--tb)}
.detail-card.internal{border-left:3px solid var(--nv)}
.detail-card.remote{border-left:3px solid var(--purple)}
.dc-header{display:flex;align-items:center;gap:8px;margin-bottom:8px}
.dc-name{font-size:11px;font-weight:700;flex:1}
.dc-project{font-size:8px;color:var(--muted);background:var(--bg-deep);padding:2px 6px;border-radius:3px}
.dc-prio{font-size:8px;font-weight:700;padding:2px 6px;border-radius:10px;min-width:20px;text-align:center}
.dc-prio.p1{background:rgba(239,68,68,.2);color:var(--red)}
.dc-prio.p2{background:rgba(249,115,22,.2);color:var(--warm)}
.dc-prio.p3{background:rgba(245,158,11,.2);color:var(--amber)}
.dc-prio.p4{background:rgba(118,185,0,.2);color:var(--nv)}
.dc-prio.p5{background:rgba(100,100,100,.2);color:var(--muted)}
.dc-gpu{font-size:8px;font-weight:600;padding:2px 6px;border-radius:10px}
.dc-gpu.egpu{background:rgba(0,176,240,.12);color:var(--tb);border:.5px solid rgba(0,176,240,.25)}
.dc-gpu.internal{background:rgba(118,185,0,.12);color:var(--nv);border:.5px solid rgba(118,185,0,.25)}
.dc-gpu.remote{background:rgba(168,85,247,.12);color:var(--purple);border:.5px solid rgba(168,85,247,.25)}
.dc-gpu.none{background:var(--bg-deep);color:var(--muted);border:.5px solid var(--border)}
.dc-workloads{display:flex;gap:4px;flex-wrap:wrap;margin-bottom:6px}
.dc-wl{font-size:8px;padding:2px 8px;border-radius:10px;background:var(--bg-deep);color:var(--muted);
  border:.5px solid var(--border);display:flex;align-items:center;gap:4px}
.dc-wl svg{width:10px;height:10px;fill:currentColor}
.dc-stats{display:grid;grid-template-columns:1fr 1fr;gap:3px 10px;font-size:9px;color:var(--muted)}
.dc-stat-val{color:var(--text);font-weight:600}
.dc-vram{margin-top:6px}
.dc-vbar{height:3px;background:var(--bg-deep);border-radius:2px;overflow:hidden}
.dc-vfill{height:100%;border-radius:2px;transition:width .6s ease}
.dc-reason{font-size:8px;color:var(--muted);margin-top:6px;padding:4px 8px;
  background:var(--bg-deep);border-radius:4px;border-left:2px solid var(--border)}
.dc-actions{display:flex;gap:6px;margin-top:8px;padding-top:8px;border-top:.5px solid var(--border)}
.dc-actions select,.dc-actions button{font-size:9px;padding:4px 10px;border-radius:6px;
  background:var(--bg-deep);color:var(--text);border:.5px solid var(--border);cursor:pointer;min-height:28px}
.dc-actions button:hover{border-color:var(--tb);color:var(--tb)}
.dc-actions label{font-size:8px;color:var(--muted);display:flex;align-items:center;gap:4px}

/* ── SYSTEM STATS PANEL ─────────────────────────────────── */
.sys-panel{margin-top:20px;background:var(--bg-card);border:.5px solid var(--border);border-radius:12px;overflow:hidden}
.sys-header{display:flex;align-items:center;gap:8px;padding:10px 16px;border-bottom:.5px solid var(--border);background:var(--bg-deep)}
.sys-title{font-size:10px;font-weight:600;letter-spacing:.1em;text-transform:uppercase;color:var(--muted)}
.sys-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(130px,1fr));gap:0}
.sys-cell{padding:12px 14px;border-right:.5px solid var(--border);border-bottom:.5px solid var(--border);
  display:flex;flex-direction:column;gap:4px}
.sys-cell:last-child{border-right:none}
.sys-cell-lbl{font-size:8px;color:var(--muted);letter-spacing:.06em;text-transform:uppercase}
.sys-cell-val{font-size:13px;font-weight:600;color:var(--text);transition:color .4s;white-space:nowrap}
.sys-cell-sub{font-size:9px;color:var(--muted);white-space:nowrap}
.sys-bar{height:2px;border-radius:1px;background:var(--bg-deep);overflow:hidden;margin-top:3px}
.sys-fill{height:100%;border-radius:1px;transition:width .6s ease;width:0}
.sys-fill.nv{background:var(--nv)}.sys-fill.tb{background:var(--tb)}
.sys-fill.amb{background:var(--amber)}.sys-fill.pur{background:var(--purple)}

/* Recovery actions in sys panel */
.recovery-row{display:flex;gap:8px;padding:10px 16px;border-top:.5px solid var(--border);background:var(--bg-deep)}
.recovery-row button{font-size:9px;padding:6px 14px;border-radius:6px;border:.5px solid rgba(239,68,68,.3);
  background:rgba(239,68,68,.1);color:var(--red);cursor:pointer;transition:all .2s}
.recovery-row button:hover{background:rgba(239,68,68,.2);border-color:var(--red)}

/* ── EVENT LOG ──────────────────────────────────────────── */
.log-section{margin-top:16px}
.log-header{display:flex;align-items:center;gap:8px;margin-bottom:8px}
.log-tabs{display:flex;gap:4px}
.log-tabs button{font-size:9px;padding:4px 12px;border-radius:10px;border:.5px solid var(--border);
  background:var(--bg-card);color:var(--muted);cursor:pointer}
.log-tabs button.active{background:rgba(0,176,240,.12);color:var(--tb);border-color:rgba(0,176,240,.3)}
.log-filter{margin-left:auto}
.log-filter input{font-size:9px;padding:4px 10px;border-radius:6px;background:var(--bg-card);
  color:var(--text);border:.5px solid var(--border);width:160px}
.log-filter input:focus{border-color:var(--tb);outline:none}
.log-list{max-height:200px;overflow-y:auto;font-size:9px;scrollbar-width:thin;scrollbar-color:var(--border) transparent}
.log-entry{padding:4px 8px;border-bottom:.5px solid var(--bg-deep);display:flex;gap:8px}
.log-entry:hover{background:var(--bg-deep)}
.log-ts{color:var(--muted);font-size:8px;flex-shrink:0;min-width:60px}
.log-type{color:var(--tb);font-weight:600;font-size:8px;flex-shrink:0;min-width:80px}
.log-msg{color:var(--text);flex:1}
.log-entry.warn{border-left:2px solid var(--amber)}
.log-entry.error{border-left:2px solid var(--red)}

/* ── LEGEND ─────────────────────────────────────────────── */
.legend{display:flex;align-items:center;gap:16px;justify-content:center;margin-top:16px;flex-wrap:wrap}
.li{display:flex;align-items:center;gap:5px;font-size:10px;color:var(--muted)}
.ld{width:6px;height:6px;border-radius:50%;flex-shrink:0}

/* ── MODAL ──────────────────────────────────────────────── */
.modal-overlay{position:fixed;inset:0;background:rgba(0,0,0,.7);display:flex;
  align-items:center;justify-content:center;z-index:200;backdrop-filter:blur(4px)}
.modal-overlay.hidden{display:none}
.modal{background:var(--bg-card);border-radius:12px;padding:20px;max-width:400px;width:90%;
  border:1px solid var(--border)}
.modal h3{font-size:12px;font-weight:700;margin-bottom:8px}
.modal p{font-size:11px;color:var(--muted);margin-bottom:16px;line-height:1.5}
.modal-actions{display:flex;gap:8px;justify-content:flex-end}
.modal-actions button{font-size:10px;padding:6px 14px;border-radius:6px;border:.5px solid var(--border);
  background:var(--bg-deep);color:var(--text);cursor:pointer}
.modal-actions .danger{border-color:rgba(239,68,68,.3);color:var(--red)}
</style>
</head>
<body>
<div class="root">

<div class="pipeline-title">eGPU Manager · Pipeline Monitor</div>
<div class="live-bar">
  <div id="live-dot" class="live-dot off"></div>
  <span id="live-status" class="mono" style="font-size:10px">Verbinde…</span>
  <span id="live-uptime" class="mono" style="font-size:10px;opacity:.5"></span>
</div>
<div id="warn-banner" class="warn-banner"></div>

<!-- ── PIPELINE FLOW ───────────────────────────────────── -->
<div class="pipeline" id="pipeline-flow"></div>

<!-- ── PIPELINE DETAIL CARDS ───────────────────────────── -->
<div class="detail-section">
  <div class="detail-title">
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none"><rect x="2" y="3" width="8" height="5" rx="1" stroke="currentColor" stroke-width="1.5"/><rect x="14" y="3" width="8" height="5" rx="1" stroke="currentColor" stroke-width="1.5"/><rect x="2" y="16" width="8" height="5" rx="1" stroke="currentColor" stroke-width="1.5"/><rect x="14" y="16" width="8" height="5" rx="1" stroke="currentColor" stroke-width="1.5"/><path d="M6 8v3h12V8M12 11v5" stroke="currentColor" stroke-width="1.5"/></svg>
    Pipeline-Details
  </div>
  <div class="detail-grid" id="detail-grid"></div>
</div>

<!-- ── SYSTEM STATS ────────────────────────────────────── -->
<div class="sys-panel">
  <div class="sys-header">
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none"><rect x="2" y="3" width="20" height="14" rx="2" stroke="currentColor" stroke-width="1.5"/><path d="M8 21h8M12 17v4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>
    <div class="sys-title">System · GPU-Laufzeit</div>
    <div id="sys-ts" class="mono" style="font-size:9px;color:var(--muted);margin-left:auto;opacity:.6"></div>
  </div>
  <div class="sys-grid" id="sys-grid"></div>
  <div class="recovery-row">
    <button onclick="window._recoveryReset()">PCIe Reset</button>
    <button onclick="window._tbReconnect()">TB Reconnect</button>
    <span style="flex:1"></span>
    <button onclick="window._downloadSetup()" style="border-color:rgba(168,85,247,.3);color:var(--purple);background:rgba(168,85,247,.1)">
      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" style="vertical-align:middle;margin-right:4px"><path d="M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4M7 10l5 5 5-5M12 15V3" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>
      Windows-Setup (LanGPU)
    </button>
    <span class="mono" style="font-size:8px;color:var(--muted)" id="recovery-status"></span>
  </div>
</div>

<!-- ── EVENT LOG ────────────────────────────────────────── -->
<div class="log-section">
  <div class="log-header">
    <div class="log-tabs" id="log-tabs">
      <button class="active" data-log="events">Events</button>
      <button data-log="audit">Audit</button>
    </div>
    <div class="log-filter"><input type="text" id="log-filter" placeholder="Filter..." aria-label="Log-Filter"></div>
  </div>
  <div class="log-list" id="log-list"></div>
</div>

<!-- ── LEGEND ──────────────────────────────────────────── -->
<div class="legend">
  <div class="li"><div class="ld" style="background:var(--tb)"></div>Datenfluss</div>
  <div class="li"><div class="ld" style="background:var(--nv)"></div>GPU aktiv (NVIDIA)</div>
  <div class="li"><div class="ld" style="background:var(--amber)"></div>Host (NUC)</div>
  <div class="li"><div class="ld" style="background:var(--tb)"></div>Thunderbolt 4</div>
  <div class="li"><div class="ld" style="background:var(--purple)"></div>Remote</div>
</div>

</div>

<!-- Modal -->
<div class="modal-overlay hidden" id="confirm-modal">
  <div class="modal">
    <h3 id="modal-title">Bestaetigung</h3>
    <p id="modal-body"></p>
    <div class="modal-actions">
      <button id="modal-cancel">Abbrechen</button>
      <button class="danger" id="modal-confirm">Ausfuehren</button>
    </div>
  </div>
</div>

<script>
(function(){
"use strict";
var BASE="";
var state={daemon:null,gpus:[],remote_gpus:[],pipelines:[],events:[],audit:[],sysinfo:null,activeLogTab:"events"};
var sse=null,reconnAttempt=0;

// Workload SVG icons (inline, no deps)
var WL_ICONS={
  ocr:'<svg viewBox="0 0 24 24" width="10" height="10"><rect x="3" y="3" width="7" height="7" rx="1" stroke="currentColor" stroke-width="1.5" fill="none"/><rect x="14" y="3" width="7" height="7" rx="1" stroke="currentColor" stroke-width="1.5" fill="none"/><rect x="3" y="14" width="7" height="7" rx="1" stroke="currentColor" stroke-width="1.5" fill="none"/><rect x="14" y="14" width="7" height="7" rx="1" stroke="currentColor" stroke-width="1.5" fill="none" stroke-dasharray="2 1"/></svg>',
  llm:'<svg viewBox="0 0 24 24" width="10" height="10"><path d="M21 11.5a8.4 8.4 0 01-.9 3.8 8.5 8.5 0 01-7.6 4.7 8.4 8.4 0 01-3.8-.9L3 21l1.9-5.7a8.4 8.4 0 01-.9-3.8 8.5 8.5 0 014.7-7.6 8.4 8.4 0 013.8-.9h.5a8.5 8.5 0 018 8v.5z" fill="none" stroke="currentColor" stroke-width="1.5"/></svg>',
  embeddings:'<svg viewBox="0 0 24 24" width="10" height="10"><circle cx="12" cy="5" r="2.5" fill="none" stroke="currentColor" stroke-width="1.5"/><circle cx="5" cy="19" r="2.5" fill="none" stroke="currentColor" stroke-width="1.5"/><circle cx="19" cy="19" r="2.5" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M12 8v4M8.5 17L11 13M15.5 17L13 13" fill="none" stroke="currentColor" stroke-width="1.2"/></svg>',
  interactive:'<svg viewBox="0 0 24 24" width="10" height="10"><rect x="2" y="3" width="20" height="14" rx="2" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M8 21h8M12 17v4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>',
  development:'<svg viewBox="0 0 24 24" width="10" height="10"><path d="M7 8l-4 4 4 4M17 8l4 4-4 4M14 4l-4 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>'
};

function esc(s){if(s==null)return"";var d=document.createElement("div");d.appendChild(document.createTextNode(String(s)));return d.innerHTML}
function tc(t){return t>=80?"c-hot":t>=65?"c-warm":"c-ok"}
function ts(){var n=new Date();return String(n.getHours()).padStart(2,"0")+":"+String(n.getMinutes()).padStart(2,"0")+":"+String(n.getSeconds()).padStart(2,"0")}

// ─── SSE ────────────────────────────────────────────────
function connectSSE(){
  if(sse)sse.close();
  sse=new EventSource(BASE+"/api/events/stream");
  sse.onopen=function(){reconnAttempt=0;setLive(true);fetchAll()};
  sse.onerror=function(){sse.close();sse=null;scheduleReconnect()};
  ["gpu_status","warning_level","recovery_stage","pipeline_change","config_reload","audit_action"].forEach(function(t){
    sse.addEventListener(t,function(e){try{handleEvt(t,JSON.parse(e.data))}catch(_){}});
  });
}
function scheduleReconnect(){
  reconnAttempt++;var d=[1000,2000,4000,8000,16000];var delay=reconnAttempt<=d.length?d[reconnAttempt-1]:0;
  if(!delay){setLive(false);return}
  setTimeout(connectSSE,delay);
}
function handleEvt(t){
  if(t==="gpu_status"||t==="warning_level"||t==="config_reload")fetchAll();
  else if(t==="pipeline_change")fetchPipelines();
  else if(t==="recovery_stage")fetchStatus();
}
function setLive(on){
  document.getElementById("live-dot").classList.toggle("off",!on);
  document.getElementById("live-status").textContent=on?"Live · GPU-Pipeline aktiv":"Offline · Daemon nicht erreichbar";
}

// ─── API ────────────────────────────────────────────────
function fetchAll(){fetchStatus();fetchPipelines();fetchEvents();fetchAudit();fetchSystem()}
function fetchStatus(){
  fetch(BASE+"/api/status").then(function(r){return r.json()}).then(function(d){
    state.daemon=d.daemon;state.gpus=d.gpus||[];state.remote_gpus=d.remote_gpus||[];
    renderPipelineFlow();renderSystemPanel();renderWarnBanner();
  }).catch(function(){setLive(false)});
}
function fetchPipelines(){
  fetch(BASE+"/api/pipelines").then(function(r){return r.json()}).then(function(d){
    state.pipelines=d||[];renderPipelineFlow();renderDetailCards();
  }).catch(function(){});
}
function fetchEvents(){
  fetch(BASE+"/api/events?limit=100").then(function(r){return r.json()}).then(function(d){
    state.events=d.events||[];if(state.activeLogTab==="events")renderLogs();
  }).catch(function(){});
}
function fetchAudit(){
  fetch(BASE+"/api/audit-log?limit=100").then(function(r){return r.json()}).then(function(d){
    state.audit=d.entries||[];if(state.activeLogTab==="audit")renderLogs();
  }).catch(function(){});
}
function fetchSystem(){
  fetch(BASE+"/api/system").then(function(r){return r.json()}).then(function(d){
    state.sysinfo=d;renderSystemPanel();
  }).catch(function(){});
}

// ─── Warning banner ─────────────────────────────────────
function renderWarnBanner(){
  var el=document.getElementById("warn-banner");
  if(!state.daemon){el.className="warn-banner";return}
  var lvl=(state.daemon.warning_level||"").toLowerCase();
  if(lvl.indexOf("gelb")>=0||lvl.indexOf("yellow")>=0){
    el.className="warn-banner yellow";el.textContent="\u26A0 Warnstufe Gelb: Erhoehte Fehlerrate auf eGPU";
  }else if(lvl.indexOf("orange")>=0){
    el.className="warn-banner orange";el.textContent="\u26A0 Warnstufe Orange: GPU-Migration aktiv";
  }else if(lvl.indexOf("rot")>=0||lvl.indexOf("red")>=0){
    el.className="warn-banner red";el.textContent="\u26D4 Warnstufe Rot: Recovery fehlgeschlagen — manuelle Intervention";
  }else{el.className="warn-banner"}

  // Uptime
  if(state.daemon.uptime_seconds){
    var u=state.daemon.uptime_seconds;var m=Math.floor(u/60);var h=Math.floor(m/60);
    document.getElementById("live-uptime").textContent="Uptime: "+(h>0?h+"h ":"")+(m%60)+"m";
  }

  // Recovery status
  var rs=document.getElementById("recovery-status");
  if(state.daemon.recovery_active)rs.textContent="Recovery: "+(state.daemon.recovery_stage||"aktiv");
  else rs.textContent="";
}

// ─── Pipeline flow visualization ────────────────────────
function renderPipelineFlow(){
  var el=document.getElementById("pipeline-flow");
  var gpus=state.gpus;
  var pipelines=state.pipelines;

  // Count active pipelines per workload type
  var activeWorkloads={};
  pipelines.forEach(function(p){
    if(p.workload_types)p.workload_types.forEach(function(w){activeWorkloads[w]=true});
  });

  // Pipeline stages (left side): workload sources
  var h='';

  // Stage: Workloads input
  h+='<div class="stage"><div class="stage-box'+(pipelines.length>0?" active":"")+'"><div class="stage-icon">';
  h+='<svg width="22" height="22" viewBox="0 0 24 24" fill="none"><rect x="4" y="4" width="16" height="16" rx="2" stroke="currentColor" stroke-width="1.5"/><path d="M9 9h6M9 12h6M9 15h4" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/></svg>';
  h+='</div><div class="stage-lbl">Workloads</div>';
  h+='<div class="stage-status mono'+(pipelines.length>0?"":" off")+'">'+pipelines.length+' Pipeline'+(pipelines.length!==1?'s':'')+'</div>';
  h+='</div></div>';

  h+=connLine(pipelines.length>0);

  // Stage: OCR
  var hasOcr=activeWorkloads.ocr;
  h+='<div class="stage"><div class="stage-box'+(hasOcr?" active":" idle")+'"><div class="stage-icon">';
  h+='<svg width="22" height="22" viewBox="0 0 24 24" fill="none"><rect x="3" y="3" width="7" height="7" rx="1" stroke="'+(hasOcr?"var(--tb)":"currentColor")+'" stroke-width="1.5"/><rect x="14" y="3" width="7" height="7" rx="1" stroke="'+(hasOcr?"var(--tb)":"currentColor")+'" stroke-width="1.5"/><rect x="3" y="14" width="7" height="7" rx="1" stroke="'+(hasOcr?"var(--tb)":"currentColor")+'" stroke-width="1.5"/><rect x="14" y="14" width="7" height="7" rx="1" stroke="currentColor" stroke-width="1.5" stroke-dasharray="2 1"/></svg>';
  h+='</div><div class="stage-lbl">OCR</div>';
  h+='<div class="sub">Texterkennung</div></div></div>';

  h+=connLine(hasOcr);

  // Stage: Embeddings
  var hasEmb=activeWorkloads.embeddings;
  h+='<div class="stage"><div class="stage-box'+(hasEmb?" active":" idle")+'"><div class="stage-icon">';
  h+='<svg width="22" height="22" viewBox="0 0 24 24" fill="none"><circle cx="12" cy="5" r="3" stroke="currentColor" stroke-width="1.5"/><circle cx="5" cy="19" r="3" stroke="currentColor" stroke-width="1.5"/><circle cx="19" cy="19" r="3" stroke="currentColor" stroke-width="1.5"/><path d="M12 8v4M8.5 17L11 13M15.5 17L13 13" stroke="currentColor" stroke-width="1.2"/><circle cx="12" cy="13" r="2" stroke="currentColor" stroke-width="1"/></svg>';
  h+='</div><div class="stage-lbl">Embeddings</div>';
  h+='<div class="sub">Vektoren</div></div></div>';

  h+=connLine(true);

  // NUC
  h+='<div class="stage"><div class="nuc-box">';
  h+='<div class="nuc-chip"><div class="nuc-badge mono">NUC</div></div>';
  h+='<div style="display:flex;align-items:center;gap:5px"><div class="nuc-led"></div>';
  h+='<div style="font-size:10px;font-weight:600;color:var(--muted);letter-spacing:.04em">ASUS NUC 15</div></div>';
  h+='<div class="sub mono">Core Ultra · 64 GB RAM</div>';
  h+='</div></div>';

  // Combine local + remote GPUs for display
  var allGpus=gpus.slice();
  var remotes=state.remote_gpus||[];
  if(remotes.length>0){
    remotes.forEach(function(r){
      allGpus.push({
        name:r.gpu_name||r.name||"Remote GPU",type:"remote",status:r.status||"offline",
        memory_used_mb:0,memory_total_mb:r.vram_mb||0,
        utilization_gpu_percent:0,power_draw_w:0,temperature_c:0,pstate:"--",
        pci_address:"remote",host:r.host,latency_ms:r.latency_ms
      });
    });
  }else{
    // Always show LanGPU placeholder (RTX 5060 Ti 16GB)
    allGpus.push({
      name:"NVIDIA GeForce RTX 5060 Ti",type:"remote",status:"offline",
      memory_used_mb:0,memory_total_mb:16384,
      utilization_gpu_percent:0,power_draw_w:0,temperature_c:0,pstate:"--",
      pci_address:"remote",host:"Windows Desktop",latency_ms:null
    });
  }

  // Split wires to GPUs
  var gpuCount=allGpus.length;
  var splitH=gpuCount>1?Math.max(120,gpuCount*78)+"px":"60px";
  h+='<div class="split" style="height:'+splitH+'">';
  h+='<div style="position:absolute;left:0;top:50%;width:12px;height:1.5px;background:var(--border);transform:translateY(-50%)"></div>';
  if(gpuCount>1){
    // Draw wires for each GPU position
    var positions=[];
    for(var wi=0;wi<gpuCount;wi++){
      var yPos=gpuCount<=1?50:Math.round(22+((100-44)/(gpuCount-1))*wi);
      positions.push(yPos);
    }
    h+='<div style="position:absolute;left:12px;top:'+positions[0]+'%;width:1.5px;height:'+(positions[positions.length-1]-positions[0])+'%;background:var(--border)"></div>';
    positions.forEach(function(yp,idx){
      var col=allGpus[idx]&&allGpus[idx].type==="remote"?"var(--purple)":"var(--border)";
      h+='<div style="position:absolute;left:12px;top:'+yp+'%;right:0;height:1.5px;background:'+col+'"></div>';
    });
  }else{
    h+='<div style="position:absolute;left:12px;top:50%;right:0;height:1.5px;background:var(--border);transform:translateY(-50%)"></div>';
  }
  h+='</div>';

  // GPU column
  h+='<div class="gpu-section">';
  allGpus.forEach(function(g,i){
    var isEgpu=g.type==="egpu";
    var isRemote=g.type==="remote";
    var isOnline=g.status==="online"||g.status==="available";
    var vramPct=g.memory_total_mb>0?Math.round(g.memory_used_mb/g.memory_total_mb*100):0;
    var ledCls=g.utilization_gpu_percent>50?"fast":g.utilization_gpu_percent>5?"slow":"off";
    if(isRemote&&isOnline)ledCls="slow";
    if(isRemote&&!isOnline)ledCls="off";

    h+='<div class="gpu-card'+(isEgpu?" egpu":"")+(isRemote?" remote":"")+(!isOnline&&isRemote?" offline":"")+'">';
    h+='<div class="gpu-top">';

    if(isEgpu){
      // Thunderbolt connector visual
      h+='<div style="display:flex;flex-direction:column;align-items:center;gap:2px;flex-shrink:0">';
      h+='<div class="tb-conn"><div class="mono" style="font-size:9px;color:var(--tb);font-weight:600;writing-mode:vertical-rl;transform:rotate(180deg);letter-spacing:.08em">TB4</div>';
      h+='<div class="tb-line"><div class="tb-dot"></div><div class="tb-dot"></div></div></div>';
      h+='<div class="gpu-pcb"><div class="fan"></div><div class="fan"></div>';
      h+='<div style="position:absolute;bottom:8px;left:50%;transform:translateX(-50%);width:24px;height:3px;border-radius:1px;background:rgba(118,185,0,.5)"></div></div></div>';
    }else if(isRemote){
      // Network/LAN connector visual
      h+='<div style="display:flex;flex-direction:column;align-items:center;gap:2px;flex-shrink:0">';
      h+='<div class="tb-conn"><div class="mono" style="font-size:9px;color:var(--purple);font-weight:600;writing-mode:vertical-rl;transform:rotate(180deg);letter-spacing:.08em">LAN</div>';
      h+='<div class="tb-line" style="background:linear-gradient(90deg,var(--border),var(--purple))"><div class="tb-dot" style="background:var(--purple)"></div><div class="tb-dot" style="background:var(--purple)"></div></div></div>';
      h+='<div style="width:52px;height:36px;border-radius:4px;background:var(--bg-deep);border:1px solid rgba(168,85,247,.3);position:relative;display:flex;align-items:center;justify-content:center">';
      h+='<svg width="24" height="24" viewBox="0 0 24 24" fill="none"><circle cx="12" cy="12" r="9" stroke="rgba(168,85,247,.6)" stroke-width="1.2"/><path d="M2 12h20M12 3a15 15 0 014 9 15 15 0 01-4 9 15 15 0 01-4-9 15 15 0 014-9z" stroke="rgba(168,85,247,.6)" stroke-width="1" fill="none"/></svg>';
      h+='</div></div>';
    }else{
      // Internal GPU PCB
      h+='<div class="gpu-pcb"><div class="fan"></div><div class="fan"></div>';
      h+='<div style="position:absolute;bottom:8px;left:50%;transform:translateX(-50%);width:24px;height:3px;border-radius:1px;background:rgba(118,185,0,.4)"></div></div>';
    }

    h+='<div class="gpu-inf">';
    h+='<div style="display:flex;align-items:center;gap:5px;margin-bottom:1px">';
    if(isRemote){
      h+='<div class="gpu-brand mono" style="color:var(--purple)">NVIDIA GeForce</div>';
      h+='<div class="remote-badge mono">LanGPU</div>';
    }else{
      h+='<div class="gpu-brand mono">NVIDIA GeForce</div>';
      if(isEgpu)h+='<div class="egpu-badge mono">eGPU</div>';
    }
    h+='</div>';
    h+='<div class="gpu-model">'+esc(g.name.replace("NVIDIA GeForce ",""))+'</div>';
    if(isRemote){
      h+='<div class="gpu-spec mono">'+(g.memory_total_mb||0)+' MB · '+(g.host||"LAN")+'</div>';
      if(g.latency_ms!=null)h+='<div class="gpu-spec mono" style="color:'+(g.latency_ms>50?"var(--warm)":"var(--purple)")+'">Latenz: '+g.latency_ms+' ms</div>';
    }else{
      h+='<div class="gpu-spec mono">'+g.memory_total_mb+' MB'+(isEgpu?' · Razer Core X V2':' · PCIe intern')+'</div>';
    }
    h+='<div class="vram-bar"><div class="vram-fill" style="width:'+vramPct+'%;'+(isRemote?'background:linear-gradient(90deg,var(--purple),rgba(168,85,247,.5))':'')+'">';
    h+='</div></div>';
    h+='<div class="vram-lbl mono">VRAM '+vramPct+'%'+(g.memory_used_mb?' · '+g.memory_used_mb+' / '+g.memory_total_mb+' MB':'')+'</div>';
    h+='<div class="g-status"><div class="g-led '+ledCls+'"'+(isRemote?' style="background:var(--purple)"':'')+'></div>';
    h+='<div class="g-stxt mono'+(isOnline?"":" off")+'"'+(isRemote&&isOnline?' style="color:var(--purple)"':'')+'>'+(isOnline?"ACTIVE":"OFFLINE")+'</div></div>';
    h+='</div></div>';

    // Stats row
    if(!isRemote){
      h+='<div class="s-row">';
      h+='<div class="s-cell"><div class="s-lbl mono">Auslastung</div><div class="s-val mono">'+g.utilization_gpu_percent+' %</div>';
      h+='<div class="s-bar"><div class="s-fill" style="width:'+g.utilization_gpu_percent+'%"></div></div></div>';
      h+='<div class="s-cell"><div class="s-lbl mono">Leistung</div><div class="s-val mono">'+g.power_draw_w.toFixed(1)+' W</div>';
      var maxP=isEgpu?300:85;
      h+='<div class="s-bar"><div class="s-fill pwr" style="width:'+(g.power_draw_w/maxP*100).toFixed(0)+'%"></div></div></div>';
      h+='<div class="s-cell"><div class="s-lbl mono">Temperatur</div><div class="s-val mono '+tc(g.temperature_c)+'">'+g.temperature_c+' °C</div>';
      h+='<div class="s-bar"><div class="s-fill tmp" style="width:'+(g.temperature_c/90*100).toFixed(0)+'%"></div></div></div>';
      h+='</div>';
    }else{
      // Remote: show connection info instead
      h+='<div class="s-row">';
      h+='<div class="s-cell"><div class="s-lbl mono">Latenz</div><div class="s-val mono">'+(g.latency_ms!=null?g.latency_ms+' ms':'--')+'</div></div>';
      h+='<div class="s-cell"><div class="s-lbl mono">VRAM</div><div class="s-val mono">'+(g.memory_total_mb||0)+' MB</div></div>';
      h+='<div class="s-cell"><div class="s-lbl mono">Status</div><div class="s-val mono" style="color:'+(isOnline?'var(--purple)':'var(--muted)')+'">'+esc(g.status)+'</div></div>';
      h+='</div>';
    }
    h+='</div>';
  });
  h+='</div>';

  // Merge wires
  if(gpuCount>1){
    h+='<div class="merge" style="height:'+splitH+'">';
    var mPositions=[];
    for(var mi=0;mi<gpuCount;mi++){
      var myPos=Math.round(22+((100-44)/(gpuCount-1))*mi);
      mPositions.push(myPos);
    }
    h+='<div style="position:absolute;right:0;top:'+mPositions[0]+'%;width:1.5px;height:'+(mPositions[mPositions.length-1]-mPositions[0])+'%;background:var(--border)"></div>';
    mPositions.forEach(function(yp,idx){
      var col=allGpus[idx]&&allGpus[idx].type==="remote"?"var(--purple)":"var(--border)";
      h+='<div style="position:absolute;right:0;top:'+yp+'%;left:0;height:1.5px;background:'+col+'"></div>';
    });
    h+='</div>';
  }

  h+=connLine(true);

  // LLM Output stage
  var hasLlm=activeWorkloads.llm;
  h+='<div class="stage"><div class="stage-box" style="border-color:rgba(0,176,240,.3)">';
  h+='<div style="width:60px;height:28px;position:relative;overflow:hidden;border-radius:4px">';
  h+='<div style="position:absolute;height:1.5px;border-radius:1px;background:var(--tb);left:0;width:80%;animation:stream 1.8s linear infinite;opacity:.7;top:5px"></div>';
  h+='<div style="position:absolute;height:1.5px;border-radius:1px;background:var(--tb);left:0;width:60%;animation:stream 1.8s linear infinite .3s;opacity:.7;top:11px"></div>';
  h+='<div style="position:absolute;height:1.5px;border-radius:1px;background:var(--tb);left:0;width:90%;animation:stream 1.8s linear infinite .6s;opacity:.7;top:17px"></div>';
  h+='<div style="position:absolute;height:1.5px;border-radius:1px;background:var(--tb);left:0;width:50%;animation:stream 1.8s linear infinite .9s;opacity:.7;top:23px"></div>';
  h+='</div>';
  h+='<div class="stage-lbl" style="color:var(--tb)">LLM / Output</div></div>';
  h+='<div class="sub mono">Inferenz</div></div>';

  el.innerHTML=h;

  // Add stream keyframes if not present
  if(!document.getElementById("stream-kf")){
    var s=document.createElement("style");s.id="stream-kf";
    s.textContent="@keyframes stream{0%{opacity:0;transform:translateX(-100%)}10%{opacity:.7}90%{opacity:.7}100%{opacity:0;transform:translateX(120%)}}";
    document.head.appendChild(s);
  }
}

function connLine(active){
  var cls=active?"conn":"conn paused";
  return '<div class="'+cls+'"><div class="conn-line"><div class="dot"></div><div class="dot"></div></div></div>';
}

// ─── Detail cards ───────────────────────────────────────
function renderDetailCards(){
  var sorted=state.pipelines.slice().sort(function(a,b){return a.priority-b.priority});
  var el=document.getElementById("detail-grid");
  var h="";

  sorted.forEach(function(p){
    var gpuType=p.gpu_type||"none";
    var cls=gpuType==="egpu"?"egpu":gpuType==="internal"?"internal":gpuType==="remote"?"remote":"";
    var vram=p.actual_vram_mb||p.vram_estimate_mb||0;
    var maxVram=gpuType==="egpu"?16303:gpuType==="internal"?8151:16384;
    var vramPct=maxVram>0?Math.min(100,vram/maxVram*100):0;
    var vramColor=gpuType==="egpu"?"var(--tb)":gpuType==="internal"?"var(--nv)":gpuType==="remote"?"var(--purple)":"var(--muted)";
    var gpuLabel=gpuType==="egpu"?"RTX 5070 Ti":gpuType==="internal"?"RTX 5060":gpuType==="remote"?"Remote":"—";

    h+='<div class="detail-card '+cls+'">';
    h+='<div class="dc-header">';
    h+='<div class="dc-name">'+esc(p.container)+'</div>';
    h+='<div class="dc-project">'+esc(p.project)+'</div>';
    h+='<div class="dc-prio p'+p.priority+' mono">P'+p.priority+'</div>';
    h+='<div class="dc-gpu '+cls+' mono">'+gpuLabel+'</div>';
    h+='</div>';

    // Workloads
    if(p.workload_types&&p.workload_types.length){
      h+='<div class="dc-workloads">';
      p.workload_types.forEach(function(w){
        h+='<div class="dc-wl">'+(WL_ICONS[w.toLowerCase()]||"")+' '+esc(w)+'</div>';
      });
      h+='</div>';
    }

    // Stats
    h+='<div class="dc-stats">';
    h+='<div>VRAM <span class="dc-stat-val mono">'+vram+' MB</span></div>';
    h+='<div>Status <span class="dc-stat-val mono">'+esc(p.status)+'</span></div>';
    h+='</div>';

    // VRAM bar
    if(vram>0){
      h+='<div class="dc-vram"><div class="dc-vbar"><div class="dc-vfill" style="width:'+vramPct.toFixed(1)+'%;background:'+vramColor+'"></div></div></div>';
    }

    // Decision
    if(p.decision_reason&&p.decision_reason!=="n/a"){
      h+='<div class="dc-reason"><strong>'+esc(p.assignment_source||"auto")+':</strong> '+esc(p.decision_reason)+'</div>';
    }

    // Actions
    h+='<div class="dc-actions">';
    h+='<label>Prio: <select onchange="window._setPriority(\''+esc(p.container)+'\',this.value)">';
    for(var i=1;i<=5;i++)h+='<option value="'+i+'"'+(p.priority===i?" selected":"")+'>'+i+'</option>';
    h+='</select></label>';
    h+='<button onclick="window._assignGpu(\''+esc(p.container)+'\')">GPU zuweisen</button>';
    h+='</div></div>';
  });

  if(!h)h='<div style="color:var(--muted);text-align:center;padding:20px">Keine Pipelines konfiguriert</div>';
  el.innerHTML=h;
}

// ─── System panel ───────────────────────────────────────
function renderSystemPanel(){
  var el=document.getElementById("sys-grid");
  document.getElementById("sys-ts").textContent=ts();

  var totalUsed=0,totalTotal=0,ollamaModels=[];
  state.gpus.forEach(function(g){totalUsed+=g.memory_used_mb;totalTotal+=g.memory_total_mb});

  var h="";

  // CPU + RAM from /api/system
  var si=state.sysinfo;
  if(si){
    h+='<div class="sys-cell">';
    h+='<div class="sys-cell-lbl mono">CPU-Auslastung</div>';
    h+='<div class="sys-cell-val mono">'+si.cpu_percent.toFixed(1)+' %</div>';
    h+='<div class="sys-bar"><div class="sys-fill amb" style="width:'+si.cpu_percent.toFixed(0)+'%"></div></div>';
    h+='<div class="sys-cell-sub mono">'+si.cpu_cores+' Kerne · Load '+si.load_avg_1m.toFixed(1)+'</div>';
    h+='</div>';

    h+='<div class="sys-cell">';
    h+='<div class="sys-cell-lbl mono">Host RAM</div>';
    var ramUsedGb=(si.ram_used_mb/1024).toFixed(1);
    var ramTotalGb=(si.ram_total_mb/1024).toFixed(1);
    var ramPct=si.ram_total_mb>0?(si.ram_used_mb/si.ram_total_mb*100):0;
    h+='<div class="sys-cell-val mono">'+ramUsedGb+' / '+ramTotalGb+' GB</div>';
    h+='<div class="sys-bar"><div class="sys-fill tb" style="width:'+ramPct.toFixed(0)+'%"></div></div>';
    var ramFreeGb=(si.ram_available_mb/1024).toFixed(1);
    h+='<div class="sys-cell-sub mono">'+ramFreeGb+' GB verfuegbar</div>';
    h+='</div>';

    if(si.swap_total_mb>0){
      h+='<div class="sys-cell">';
      h+='<div class="sys-cell-lbl mono">Swap</div>';
      var swUsed=(si.swap_used_mb/1024).toFixed(1);
      var swTotal=(si.swap_total_mb/1024).toFixed(1);
      var swPct=si.swap_total_mb>0?(si.swap_used_mb/si.swap_total_mb*100):0;
      h+='<div class="sys-cell-val mono">'+swUsed+' / '+swTotal+' GB</div>';
      h+='<div class="sys-bar"><div class="sys-fill pur" style="width:'+swPct.toFixed(0)+'%"></div></div>';
      h+='</div>';
    }

    h+='<div class="sys-cell">';
    h+='<div class="sys-cell-lbl mono">System-Uptime</div>';
    var uh=Math.floor(si.uptime_seconds/3600);
    var um=Math.floor((si.uptime_seconds%3600)/60);
    h+='<div class="sys-cell-val mono">'+uh+'h '+um+'m</div>';
    h+='<div class="sys-cell-sub mono">'+si.cpu_model.substring(0,30)+'</div>';
    h+='</div>';
  }

  // Per-GPU cells
  state.gpus.forEach(function(g){
    var label=g.type==="egpu"?"eGPU":"Intern";
    var short=g.name.replace("NVIDIA GeForce ","");
    h+='<div class="sys-cell">';
    h+='<div class="sys-cell-lbl mono">'+label+' · '+short+'</div>';
    h+='<div class="sys-cell-val mono '+tc(g.temperature_c)+'">'+g.temperature_c+'°C · '+g.utilization_gpu_percent+'%</div>';
    h+='<div class="sys-cell-sub mono">'+g.power_draw_w.toFixed(1)+' W · '+g.pstate+'</div>';
    var vp=g.memory_total_mb>0?(g.memory_used_mb/g.memory_total_mb*100):0;
    h+='<div class="sys-bar"><div class="sys-fill nv" style="width:'+vp.toFixed(0)+'%"></div></div>';
    h+='</div>';
  });

  // LanGPU cell in sys panel
  var rg=state.remote_gpus&&state.remote_gpus.length>0?state.remote_gpus[0]:null;
  h+='<div class="sys-cell">';
  h+='<div class="sys-cell-lbl mono" style="color:var(--purple)">LanGPU · '+(rg?rg.gpu_name||rg.name:"RTX 5060 Ti")+'</div>';
  if(rg&&(rg.status==="online"||rg.status==="available")){
    h+='<div class="sys-cell-val mono" style="color:var(--purple)">Online</div>';
    h+='<div class="sys-cell-sub mono">'+(rg.host||"LAN")+(rg.latency_ms!=null?' · '+rg.latency_ms+'ms':'')+'</div>';
  }else{
    h+='<div class="sys-cell-val mono" style="color:var(--muted)">Offline</div>';
    h+='<div class="sys-cell-sub mono">'+(rg?rg.host:'nicht registriert')+'</div>';
  }
  h+='</div>';

  // Total VRAM
  h+='<div class="sys-cell">';
  h+='<div class="sys-cell-lbl mono">VRAM Gesamt</div>';
  h+='<div class="sys-cell-val mono">'+(totalUsed/1024).toFixed(1)+' / '+(totalTotal/1024).toFixed(1)+' GB</div>';
  var tp=totalTotal>0?(totalUsed/totalTotal*100):0;
  h+='<div class="sys-bar"><div class="sys-fill nv" style="width:'+tp.toFixed(0)+'%"></div></div>';
  h+='<div class="sys-cell-sub mono">'+tp.toFixed(0)+'% belegt</div>';
  h+='</div>';

  // Pipeline count
  h+='<div class="sys-cell">';
  h+='<div class="sys-cell-lbl mono">Pipelines</div>';
  h+='<div class="sys-cell-val mono">'+state.pipelines.length+'</div>';
  var active=state.pipelines.filter(function(p){return p.status==="assigned"||p.status==="running"||p.status==="active"}).length;
  h+='<div class="sys-cell-sub mono">'+active+' aktiv</div>';
  h+='</div>';

  // Daemon
  if(state.daemon){
    h+='<div class="sys-cell">';
    h+='<div class="sys-cell-lbl mono">Warnstufe</div>';
    h+='<div class="sys-cell-val mono">'+esc(state.daemon.warning_level)+'</div>';
    h+='<div class="sys-cell-sub mono">eGPU: '+esc(state.daemon.egpu_admission_state)+'</div>';
    h+='</div>';

    h+='<div class="sys-cell">';
    h+='<div class="sys-cell-lbl mono">Queue</div>';
    h+='<div class="sys-cell-val mono">'+state.daemon.scheduler_queue_length+'</div>';
    h+='<div class="sys-cell-sub mono">Mode: '+esc(state.daemon.mode||"normal")+'</div>';
    h+='</div>';
  }

  // PCIe Link (eGPU)
  var egpu=state.gpus.filter(function(g){return g.type==="egpu"})[0];
  if(egpu&&egpu.pcie_link_speed){
    h+='<div class="sys-cell">';
    h+='<div class="sys-cell-lbl mono">PCIe Link</div>';
    h+='<div class="sys-cell-val mono">'+esc(egpu.pcie_link_speed)+' x'+(egpu.pcie_link_width||"?")+'</div>';
    if(egpu.pcie_tx_kbps!=null)h+='<div class="sys-cell-sub mono">TX '+egpu.pcie_tx_kbps+' / RX '+egpu.pcie_rx_kbps+' KB/s</div>';
    h+='</div>';
  }

  el.innerHTML=h;
}

// ─── Logs ───────────────────────────────────────────────
function renderLogs(){
  var entries=state.activeLogTab==="audit"?state.audit:state.events;
  var filter=(document.getElementById("log-filter").value||"").toLowerCase();
  var el=document.getElementById("log-list");
  var h="";
  entries.forEach(function(e){
    var msg=e.message||"";var type=e.event_type||"";
    if(filter&&msg.toLowerCase().indexOf(filter)<0&&type.toLowerCase().indexOf(filter)<0)return;
    var sev=(e.severity||"info");
    var cls=sev==="warning"||sev==="warn"?"warn":sev==="error"||sev==="critical"?"error":"";
    h+='<div class="log-entry '+cls+'">';
    h+='<span class="log-ts mono">'+fmtTime(e.timestamp)+'</span>';
    h+='<span class="log-type mono">'+esc(type)+'</span>';
    h+='<span class="log-msg">'+esc(msg)+'</span></div>';
  });
  if(!h)h='<div style="color:var(--muted);text-align:center;padding:20px;font-size:10px">Keine Eintraege</div>';
  el.innerHTML=h;
}
function fmtTime(ts){if(!ts)return"--";try{var d=new Date(ts);return d.toLocaleTimeString("de-DE",{hour:"2-digit",minute:"2-digit",second:"2-digit"})}catch(_){return ts}}

// ─── Actions ────────────────────────────────────────────
window._setPriority=function(c,v){
  var p=parseInt(v,10);if(isNaN(p)||p<1||p>5)return;
  fetch(BASE+"/api/pipelines/"+encodeURIComponent(c)+"/priority",{method:"PUT",headers:{"Content-Type":"application/json"},body:JSON.stringify({priority:p})})
  .then(function(r){return r.json()}).then(function(){fetchPipelines()}).catch(function(){});
};
window._assignGpu=function(c){
  var o=[];state.gpus.forEach(function(g){o.push(g.pci_address+" ("+g.name+")")});
  var a=prompt("GPU PCI-Adresse:\n\n"+o.join("\n"));
  if(!a)return;a=a.split(" ")[0];
  fetch(BASE+"/api/pipelines/"+encodeURIComponent(c)+"/assign",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({gpu_device:a})})
  .then(function(r){return r.json()}).then(function(d){if(d.error)alert(d.error);else fetchPipelines()}).catch(function(){});
};
window._recoveryReset=function(){showConfirm("PCIe-FLR-Reset?","Die eGPU wird fuer ~5 Sekunden offline. CUDA-Container werden migriert.",function(){
  fetch(BASE+"/api/recovery/reset",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({confirm:true})})
  .then(function(r){return r.json()}).then(function(d){if(d.error)alert(d.error);else fetchStatus()}).catch(function(){});
})};
window._tbReconnect=function(){showConfirm("Thunderbolt-Reconnect?","eGPU wird kurzzeitig deautorisiert und neu verbunden.",function(){
  fetch(BASE+"/api/recovery/thunderbolt-reconnect",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({confirm:true})})
  .then(function(r){return r.json()}).then(function(d){if(d.error)alert(d.error);else fetchStatus()}).catch(function(){});
})};

// ─── Setup Download ─────────────────────────────────────
window._downloadSetup=function(){
  showConfirm("Windows-Setup generieren?",
    "Es wird ein ZIP-Paket mit Installationsskripten fuer den Windows-11-Remote-Node erstellt. Das ZIP auf einen USB-Stick kopieren und am Windows-Rechner entpacken.",
    function(){
      var a=document.createElement("a");
      a.href=BASE+"/api/setup/generate";
      a.download="egpu-remote-setup.zip";
      // Use fetch+POST to trigger generation, then download
      fetch(BASE+"/api/setup/generate",{method:"POST"})
        .then(function(r){
          if(!r.ok) throw new Error("Generierung fehlgeschlagen");
          return r.blob();
        })
        .then(function(blob){
          var url=URL.createObjectURL(blob);
          a.href=url;
          document.body.appendChild(a);
          a.click();
          document.body.removeChild(a);
          URL.revokeObjectURL(url);
        })
        .catch(function(e){alert("Fehler: "+e.message)});
    }
  );
};

// ─── Modal ──────────────────────────────────────────────
var confirmCb=null;
function showConfirm(t,b,cb){
  document.getElementById("modal-title").textContent=t;
  document.getElementById("modal-body").textContent=b;
  document.getElementById("confirm-modal").classList.remove("hidden");
  confirmCb=cb;document.getElementById("modal-confirm").focus();
}
document.getElementById("modal-cancel").onclick=function(){document.getElementById("confirm-modal").classList.add("hidden");confirmCb=null};
document.getElementById("modal-confirm").onclick=function(){document.getElementById("confirm-modal").classList.add("hidden");if(confirmCb){confirmCb();confirmCb=null}};
document.getElementById("confirm-modal").addEventListener("keydown",function(e){if(e.key==="Escape"){document.getElementById("confirm-modal").classList.add("hidden");confirmCb=null}});

// ─── Log tabs ───────────────────────────────────────────
document.querySelectorAll("#log-tabs button").forEach(function(b){
  b.addEventListener("click",function(){
    document.querySelectorAll("#log-tabs button").forEach(function(x){x.classList.remove("active")});
    b.classList.add("active");state.activeLogTab=b.getAttribute("data-log")||"events";renderLogs();
  });
});
document.getElementById("log-filter").addEventListener("input",renderLogs);

// ─── Init ───────────────────────────────────────────────
connectSSE();fetchAll();

})();
</script>
</body>
</html>"##;
