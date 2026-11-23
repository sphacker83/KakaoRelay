'use strict';
// 공통 도우미. 모델 출력·대화 본문·방 이름은 모두 textContent/value로만 넣는다(HTML로 해석하지 않는다).
const T = window.__TAURI__;
const $ = id => document.getElementById(id);
const call = (cmd, args) => T.core.invoke(cmd, args);
const listen = (name, handler) => T.event.listen(name, event => handler(event.payload));
window.addEventListener('error', e => call('ui_log', { text: `${e.message} @${e.filename}:${e.lineno}` }));
window.addEventListener('unhandledrejection', e => call('ui_log', { text: 'rejection: ' + (e.reason?.stack || e.reason) }));
const errorText = e => (typeof e === 'string' ? e : e?.message || String(e));

const App = {
  settings: null,
  settingsError: null,
  platform: 'macos',
  hostReady: false,
  shuttingDown: false,
  refreshers: [],
  onSettings: [],
};

/// 화면 상태를 다시 그린다(원본 PropertyChanged 전체 갱신).
function refreshAll() { for (const r of App.refreshers) r(); }

function el(tag, props = {}, children = []) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (key === 'text') node.textContent = value;
    else if (key === 'class') node.className = value;
    else if (key.startsWith('on')) node.addEventListener(key.slice(2), value);
    else if (key === 'dataset') Object.assign(node.dataset, value);
    else node[key] = value;
  }
  for (const child of children) if (child) node.append(child);
  return node;
}

function setOptions(select, items, value) {
  select.replaceChildren(...items.map(([v, label, disabled]) => {
    const option = new Option(label, v);
    if (disabled) option.disabled = true;
    return option;
  }));
  if (value !== undefined && items.some(([v]) => v === value)) select.value = value;
}

function setEnabled(ids, enabled) {
  for (const id of ids) { const node = typeof id === 'string' ? $(id) : id; if (node) node.disabled = !enabled; }
}

// 탭 전환. data-group으로 묶인 버튼과 페이지를 함께 바꾼다.
function showTab(group, id) {
  document.querySelectorAll(`.tabs[data-group="${group}"] button`).forEach(b => b.classList.toggle('active', b.dataset.tab === id));
  document.querySelectorAll(`.page[data-group="${group}"]`).forEach(p => p.classList.toggle('hidden', p.id !== id));
  document.dispatchEvent(new CustomEvent('tabshown', { detail: { group, id } }));
}
document.querySelectorAll('.tabs').forEach(nav => nav.addEventListener('click', e => {
  const button = e.target.closest('button[data-tab]');
  if (button) showTab(nav.dataset.group, button.dataset.tab);
}));
const visible = id => { const node = $(id); return !!node && node.offsetParent !== null && !document.hidden; };

// 설정 자동 저장: 원본처럼 마지막 변경 700ms 뒤 저장한다.
let saveTimer = null;
let saveHandlers = [];
function queueSave() {
  if (!App.settings || App.settingsError) return;
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => { saveTimer = null; saveSettings().catch(() => {}); }, 700);
}
function cancelQueuedSave() { clearTimeout(saveTimer); saveTimer = null; }
/// 원본 `AiPanel.Save`. 실패하면 상태 문구에 오류를 보이고 false를 돌려준다.
async function saveSettings() {
  if (!App.settings) return false;
  if (App.settingsError) { setAiStatus(App.settingsError); return false; }
  cancelQueuedSave();
  try {
    App.settings = await call('save_settings', { settings: App.settings });
    for (const h of saveHandlers) h();
    return true;
  } catch (e) { setAiStatus(errorText(e)); return false; }
}
async function reloadSettings() {
  cancelQueuedSave();
  App.settings = await call('get_settings');
  for (const h of App.onSettings) h();
  refreshAll();
}

function personaById(id) { return App.settings?.personas.find(p => p.id === id); }
function resolvePersona(room) {
  const s = App.settings; if (!s) return null;
  const id = s.roomPersonas[room.key];
  return personaById(id) || s.personas[0];
}
function replyModeFor(room) { return App.settings?.roomReplyModes[room.key] || App.settings?.replyMode || 'trigger'; }

// 진행 시간 표시용 스톱워치(원본 Stopwatch + 1초 ticker).
class Stopwatch {
  constructor(onTick) { this.onTick = onTick; this.start = 0; this.timer = null; }
  restart() { this.start = performance.now(); clearInterval(this.timer); this.timer = setInterval(this.onTick, 1000); }
  stop() { clearInterval(this.timer); this.timer = null; return this.seconds(); }
  seconds() { return (performance.now() - this.start) / 1000; }
  mmss() { const s = Math.floor(this.seconds()); return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`; }
}

async function copyText(text) {
  try { await navigator.clipboard.writeText(text); return true; }
  catch {
    const area = el('textarea', { value: text }); document.body.append(area); area.select();
    const ok = document.execCommand('copy'); area.remove(); return ok;
  }
}

/// 방 키 비교용. 설정의 방 키는 Rust `room_key`와 같은 형식이어야 한다(Rust에서 받은 `key`를 그대로 쓴다).
function sameRoom(a, b) { return a && b && a.profile === b.profile && a.id === b.id; }
