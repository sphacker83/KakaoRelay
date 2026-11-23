'use strict';
// 지식베이스 탭. 원본: KnowledgePanel.xaml(.cs), AutoKnowledgePanel.xaml(.cs).
const Kb = { busy: false, personaId: null, notes: [], selected: null, loaded: false };
const Ak = { rooms: [], loading: false, busy: false, log: [] };

const kbIdle = () => !!App.settings && !Kb.busy && !App.shuttingDown;
const kbProfile = () => personaById(Kb.personaId) || App.settings?.personas[0];
const kbVault = () => kbProfile()?.knowledge.vaultPath || '';
function setKbStatus(text) { $('kbStatus').textContent = text; $('kbStatus').title = text; }

function folderName(path) { return path.replace(/[\\/]+$/, '').split(/[\\/]/).pop(); }
function renderConnection() {
  const p = kbProfile();
  $('kbConnection').textContent = !p ? '봇을 선택하세요.' : !p.knowledge.vaultPath.trim() ? '연결 설정에서 보관함을 선택하세요.'
    : `${p.knowledge.enabled ? '● 답변에 사용 중' : '○ 답변에 사용 안 함'} · ${folderName(p.knowledge.vaultPath)}`;
}
function refreshKnowledge() {
  const idle = kbIdle();
  document.querySelectorAll('#kb-files button, #kb-files input, #kb-settings button, #kb-settings input, #kb-settings textarea, #kbPersona').forEach(n => { n.disabled = !idle; });
  $('kbEdit').disabled = !idle || !Kb.selected;
  $('kbCancel').disabled = !Kb.busy;
  const configure = !!App.settings && !Ak.loading && !App.shuttingDown;
  setEnabled(['akRefresh', 'akApply', 'akEnabled', 'akInterval', 'akAutoSave'], configure);
  document.querySelectorAll('#akRooms input').forEach(n => { n.disabled = !configure; });
  $('akRun').disabled = Ak.busy || Ak.loading || App.shuttingDown || !App.settings?.autoKnowledge.enabled;
}
App.refreshers.push(refreshKnowledge);

async function kbWork(task) {
  if (!kbIdle()) return;
  Kb.busy = true; refreshAll();
  try { await task(); } catch (e) { setKbStatus(errorText(e)); }
  finally { Kb.busy = false; refreshAll(); }
}

function showNotes(result) {
  Kb.notes = result.notes; Kb.selected = null;
  $('kbNotes').replaceChildren(...Kb.notes.map(note => {
    const li = el('li', { title: note.source, onclick: () => selectNote(note) }, [el('span', { class: 'title', text: note.source })]);
    li.dataset.path = note.path;
    return li;
  }));
  $('kbLibraryHint').textContent = result.hint;
  refreshKnowledge();
}
async function selectNote(note) {
  Kb.selected = note;
  document.querySelectorAll('#kbNotes li').forEach(li => li.classList.toggle('selected', !!note && li.dataset.path === note.path));
  $('kbPreview').value = note ? await call('note_preview', { path: note.path }) : '노트를 선택하면 내용을 볼 수 있습니다.';
  refreshKnowledge();
}
async function reloadNotes() {
  await kbWork(async () => { showNotes(await call('notes_list', { vault: kbVault() })); setKbStatus('노트 목록을 새로 불러왔습니다.'); });
}
function selectKbPersona(id) {
  if (Kb.busy) return;
  Kb.personaId = id; const p = kbProfile();
  $('kbVault').value = p.knowledge.vaultPath; $('kbEnabled').checked = p.knowledge.enabled;
  Kb.notes = []; $('kbNotes').replaceChildren(); Kb.selected = null;
  renderConnection(); refreshKnowledge();
  if (Kb.loaded) reloadNotes();
}
function renderKbPersonas() {
  setOptions($('kbPersona'), App.settings.personas.map(p => [p.id, p.persona.name]), kbProfile()?.id);
}

async function importFiles(paths) {
  if (!paths.length) return;
  await kbWork(async () => {
    const result = await call('notes_import', { vault: kbVault(), paths });
    showNotes(result.notes); $('kbPreview').value = result.preview; setKbStatus(result.status);
  });
}
async function search() {
  await kbWork(async () => {
    const result = await call('notes_search', { vault: kbVault(), query: $('kbQuery').value });
    await selectNote(null); $('kbPreview').value = result.preview; setKbStatus(result.status);
  });
}

/// 원본 Write_Click의 대화상자. 제목·내용·길이를 검사한 뒤 저장한다.
function writeNote() {
  if (!kbIdle()) return;
  const vault = kbVault();
  if (!vault.trim()) { setKbStatus('연결 설정에서 보관함을 선택하고 저장하세요.'); return; }
  $('wdTitle').value = ''; $('wdBody').value = ''; $('wdHint').textContent = '제목과 내용을 적거나 붙여 넣으세요.';
  $('writeDialog').showModal();
}
function validateWrite(event) {
  const title = $('wdTitle').value; const body = $('wdBody').value;
  if (!title.trim() || !body.trim()) { event.preventDefault(); $('wdHint').textContent = '제목과 내용을 모두 입력하세요.'; return; }
  if (new TextEncoder().encode(body).length > 256 * 1024 - 2000) { event.preventDefault(); $('wdHint').textContent = '내용이 너무 깁니다. 여러 노트로 나누어 저장하세요.'; }
}
async function saveWritten() {
  if ($('writeDialog').returnValue !== 'save') return;
  const title = $('wdTitle').value; const body = $('wdBody').value;
  await kbWork(async () => {
    const result = await call('note_write', { vault: kbVault(), title, body });
    showNotes(result.notes);
    const note = Kb.notes.find(n => n.path === result.path); if (note) await selectNote(note);
    setKbStatus('노트를 저장했습니다. 다음 질문부터 참고합니다.');
  });
}

async function saveConnection(all) {
  if (!kbIdle() || !kbProfile()) return;
  await kbWork(async () => {
    const result = await call('knowledge_save_connection', { personaId: kbProfile().id, all, vault: $('kbVault').value, enabled: $('kbEnabled').checked });
    App.settings = result.settings; showNotes(result.notes); renderConnection(); setKbStatus(result.status);
  });
}
async function pickFolder(title) { return call('pick_folder', { title }); }

// ── 대화 자동 정리 ──
function akLog(message) {
  Ak.log.unshift(message); Ak.log = Ak.log.slice(0, 8);
  $('akStatus').value = Ak.log.join('\n');
}
function renderAkRooms() {
  $('akRooms').replaceChildren(...Ak.rooms.map(room => {
    const box = el('input', { type: 'checkbox', checked: room.selected, onchange: () => { room.selected = box.checked; } });
    return el('li', {}, [el('label', { class: 'check grow' }, [box, el('span', {}, [el('span', { text: room.title }), el('span', { class: 'sub', text: room.destination })])])]);
  }));
  refreshKnowledge();
}
function loadAutoSettings() {
  const c = App.settings.autoKnowledge;
  $('akEnabled').checked = c.enabled; $('akInterval').value = c.intervalMinutes; $('akAutoSave').checked = c.autoSave;
}
async function refreshAkRooms(refresh) {
  Ak.loading = true; refreshKnowledge();
  try {
    const previous = Ak.rooms.filter(r => r.selected).map(r => r.key);
    Ak.rooms = await call('auto_rooms', { refresh, previous }); renderAkRooms();
    if (refresh) akLog('정리할 방을 체크한 뒤 설정 적용을 누르세요.');
  } catch (e) { akLog(errorText(e)); }
  finally { Ak.loading = false; refreshKnowledge(); }
}
async function applyAuto() {
  try {
    const interval = Number($('akInterval').value.trim());
    const text = await call('auto_apply', {
      enabled: $('akEnabled').checked, autoSave: $('akAutoSave').checked, intervalMinutes: Number.isInteger(interval) ? interval : 0,
      rooms: Ak.rooms.filter(r => r.selected).map(r => ({ profile: r.profile, id: r.id, title: r.title })),
    });
    App.settings = await call('get_settings');
    akLog(text); refreshKnowledge(); runAuto(false);
  } catch (e) { akLog(errorText(e)); }
}
async function runAuto(force) {
  if (Ak.busy || App.shuttingDown) return;
  Ak.busy = true; refreshKnowledge();
  try { await call('auto_run', { force }); } catch (e) { akLog(errorText(e)); }
  finally { Ak.busy = false; refreshKnowledge(); }
}

function initKnowledge() {
  Kb.personaId = App.settings.personas[0]?.id;
  renderKbPersonas(); selectKbPersona(Kb.personaId);
  $('kbPersona').onchange = () => selectKbPersona($('kbPersona').value);
  $('kbRefresh').onclick = reloadNotes;
  $('kbUpload').onclick = async () => importFiles(await call('pick_files'));
  $('kbWrite').onclick = writeNote;
  $('wdSave').onclick = validateWrite;
  $('writeDialog').addEventListener('close', saveWritten);
  $('kbSearch').onclick = search;
  $('kbQuery').addEventListener('keydown', e => { if (e.key === 'Enter') { e.preventDefault(); search(); } });
  $('kbEdit').onclick = () => Kb.selected && call('open_target', { target: 'obsidian://open?path=' + encodeURIComponent(Kb.selected.path) }).catch(e => setKbStatus(errorText(e)));
  $('kbFolder').onclick = async () => { const path = await pickFolder('봇이 참고할 옵시디언 보관함 선택'); if (path) { $('kbVault').value = path; setKbStatus('연결 저장을 누르면 적용됩니다.'); } };
  $('kbCreateVault').onclick = async () => {
    const parent = await pickFolder('새 보관함을 만들 상위 폴더 선택'); if (!parent) return;
    try { $('kbVault').value = await call('create_vault', { parent }); $('kbEnabled').checked = true; setKbStatus('보관함을 만들었습니다. 연결 저장을 누르세요.'); }
    catch (e) { setKbStatus(errorText(e)); }
  };
  const requireVault = () => { const v = kbVault(); if (!v.trim()) throw '연결 설정에서 보관함을 선택하고 저장하세요.'; return v; };
  $('kbOpenFolder').onclick = () => { try { call('open_target', { target: requireVault() }).catch(e => setKbStatus(errorText(e))); } catch (e) { setKbStatus(errorText(e)); } };
  $('kbObsidian').onclick = () => { try { call('open_target', { target: 'obsidian://open?path=' + encodeURIComponent(requireVault()) }).catch(e => setKbStatus(errorText(e))); } catch (e) { setKbStatus(errorText(e)); } };
  $('kbSave').onclick = () => saveConnection(false);
  $('kbSaveAll').onclick = () => saveConnection(true);
  $('kbCancel').onclick = () => call('knowledge_cancel');
  listen('knowledge-panel-status', text => { if (Kb.busy) setKbStatus(text); });
  // 파일 끌어 놓기: 웹뷰는 경로를 주지 않으므로 창 이벤트(drop-files)의 경로를 쓴다.
  const drop = $('kbDrop');
  const filesPage = $('kb-files');
  filesPage.addEventListener('dragover', e => { e.preventDefault(); drop.classList.add('over'); });
  filesPage.addEventListener('dragleave', () => drop.classList.remove('over'));
  filesPage.addEventListener('drop', e => { e.preventDefault(); drop.classList.remove('over'); });
  listen('drop-files', paths => { if (visible('kb-files') && kbIdle()) importFiles(paths); });
  // 대화 자동 정리
  loadAutoSettings();
  $('akRefresh').onclick = () => refreshAkRooms(true);
  $('akApply').onclick = applyAuto;
  $('akRun').onclick = () => runAuto(true);
  $('akReview').onclick = () => call('auto_review_folder', { vault: kbVault() }).catch(e => akLog(errorText(e)));
  listen('knowledge-status', akLog);
  akLog(App.settings.autoKnowledge.enabled ? '자동 정리 사용 중 · 저장된 처리 위치에서 재개합니다.' : '자동 정리가 꺼져 있습니다. 방을 선택하고 설정을 적용하세요.');
  call('auto_statuses').then(s => s.statuses.forEach(akLog));
  refreshAkRooms(false);
  document.addEventListener('tabshown', e => { if (e.detail.id === 'tab-knowledge' && !Kb.loaded) { Kb.loaded = true; reloadNotes(); } });
  App.onSettings.push(() => { renderKbPersonas(); selectKbPersona(kbProfile()?.id); loadAutoSettings(); refreshAkRooms(false); });
}
