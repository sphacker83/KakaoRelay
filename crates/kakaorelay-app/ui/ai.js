'use strict';
// AI 챗봇 탭. 원본: AiPanel.xaml(.cs). 정리감시·제어방·이미지는 각 파일에 있다.
const Ai = {
  busy: false, generating: false, status: '설정 준비 중',
  rooms: [], open: [], selectedRoom: null,
  botOrder: [],            // 선택 순서대로의 방 키(원본 botSelectionOrder)
  answerRoom: null, summaryRoom: null, summaryPersonaId: null,
  bots: { running: false, count: 0, badge: '○ 자동답장 꺼짐', statuses: [], lastError: '' },
  editingId: null, catalogs: {}, catalogGeneration: 0,
  refreshingOpen: false, lastDiscovery: 0,
  greetings: new Set(),    // 자동인사를 켠 방 키(원본에 없는 기능)
};
const REPLY_MODES = { immediate: '즉시 답변', trigger: '호출어 답변', context: '맥락 판단' };
const MODE_CHIPS = { immediate: '즉시', trigger: '호출', context: '맥락' };
const EFFORTS = ['default', 'none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max', 'ultra', 'ultracode'];
const aiWatch = new Stopwatch(() => renderActivity());

const aiIdle = () => !Ai.busy && !!App.settings && !App.settingsError;
const canStartBots = () => aiIdle() && App.hostReady && !Ai.bots.running;
function setAiStatus(text) { Ai.status = text; renderActivity(); }
function renderActivity() {
  $('aiActivity').textContent = Ai.busy ? `${Ai.status} · ${aiWatch.mmss()} 경과` : Ai.status;
  $('aiActivity').title = $('aiActivity').textContent;
  $('aiProgress').classList.toggle('hidden', !Ai.busy);
}

/// 원본 Work: 한 번에 하나의 AI 화면 작업만 하고, 끝나면 걸린 시간을 붙인다.
async function aiWork(task) {
  if (!aiIdle()) return;
  Ai.busy = true; aiWatch.restart(); refreshAll();
  try { await task(); }
  catch (e) { const text = errorText(e); setAiStatus(text === '중지했습니다.' ? text : '실패: ' + text); }
  finally {
    Ai.busy = false; Ai.generating = false;
    const seconds = aiWatch.stop();
    Ai.status += ` · ${seconds.toFixed(1)}초`;
    refreshAll();
  }
}

function personaName(room) {
  const id = App.settings?.roomPersonas[room.key];
  return (id && personaById(id)?.persona.name) || '';
}

// ── 자동답장 ──
function bindingRoom() { const key = Ai.botOrder[Ai.botOrder.length - 1]; return Ai.open.find(r => r.key === key) || null; }
function renderBotRooms() {
  const list = $('botRooms');
  list.replaceChildren(...Ai.open.map(room => {
    const name = personaName(room);
    const item = el('li', { role: 'option', title: room.display }, [
      el('span', { class: 'title', text: room.title }),
      name ? el('span', { class: 'chip', text: name }) : null,
      el('span', { class: 'chip mode', text: MODE_CHIPS[replyModeFor(room)] || '', title: '이 방의 답변 방식' }),
      Ai.greetings.has(room.key) ? el('span', { class: 'chip', text: '🤖인사', title: '자동인사 켜짐' }) : null,
    ]);
    item.classList.toggle('selected', Ai.botOrder.includes(room.key));
    item.onclick = () => toggleBotRoom(room);
    return item;
  }));
}
function toggleBotRoom(room) {
  if (!canStartBots()) return;
  const wasSelected = Ai.botOrder.includes(room.key);
  Ai.botOrder = Ai.botOrder.filter(k => k !== room.key);
  if (!wasSelected) Ai.botOrder.push(room.key);
  App.settings.selectedBotRooms = [...Ai.botOrder];
  queueSave(); updateBinding(); renderBotRooms();
}
function updateBinding() {
  const room = bindingRoom();
  const s = App.settings;
  setOptions($('bindPersona'), s.personas.map(p => [p.id, p.persona.name]));
  if (room) { $('bindPersona').value = resolvePersona(room)?.id; $('bindMode').value = replyModeFor(room); }
  else $('bindMode').value = s.replyMode;
  $('bindRow').title = room?.title || '';
  $('bindGreeting').checked = !!room && Ai.greetings.has(room.key);
  refreshAi();
}
function bindPersona() {
  const room = bindingRoom();
  if (!aiIdle() || !room) return;
  const profile = personaById($('bindPersona').value); const mode = $('bindMode').value;
  App.settings.roomPersonas[room.key] = profile.id;
  App.settings.roomReplyModes[room.key] = mode;
  saveSettings().then(ok => { if (ok) setAiStatus(`${room.title} · ${profile.persona.name} · ${REPLY_MODES[mode]} 저장됨`); renderRoomsAll(); });
}
/// 선택한 방의 자동인사를 켜거나 끈다. 자동답장 중에도 바꿀 수 있고 다음 폴링부터 반영된다.
async function toggleGreeting() {
  const room = bindingRoom();
  if (!room) return;
  const enabled = $('bindGreeting').checked;
  try {
    Ai.greetings = new Set(await call('set_greeting', { key: room.key, enabled }));
    setAiStatus(`${room.title} · 자동인사 ${enabled ? '켜짐' : '꺼짐'}`);
  } catch (e) { $('bindGreeting').checked = !enabled; setAiStatus(errorText(e)); }
  renderBotRooms();
}
function renderRoomsAll() { renderBotRooms(); renderChatRooms(); }

/// 원본 ApplyRooms. 화면 선택은 저장된 설정에서 복원한다.
function applyRooms(payload) {
  const previous = Ai.selectedRoom;
  Ai.rooms = payload.rooms; Ai.open = payload.open;
  Ai.selectedRoom = Ai.rooms.find(r => sameRoom(r, previous))
    || Ai.rooms.find(r => r.key === App.settings.selectedChatRoom) || Ai.rooms.find(r => r.readable) || null;
  Ai.botOrder = [];
  for (const key of App.settings.selectedBotRooms) if (Ai.open.some(r => r.key === key) && !Ai.botOrder.includes(key)) Ai.botOrder.push(key);
  renderRoomsAll(); updateBinding();
}

async function refreshRooms() {
  await aiWork(async () => {
    setAiStatus('카카오톡 로컬 대화방 확인 중');
    const payload = await call('ai_rooms');
    applyRooms(payload);
    setAiStatus(`열린 방 ${Ai.open.length}개 · 로컬 방 ${payload.rooms.filter(r => r.profile !== 'import').length}개`);
  });
}

/// 원본 RefreshOpenRoomsAsync(2초 주기). 보일 때·미가동·유휴일 때만 돈다.
async function pollOpenRooms() {
  if (!visible('tab-ai') || !aiIdle() || Ai.bots.running || Ai.refreshingOpen) return;
  Ai.refreshingOpen = true;
  try {
    const allowed = Date.now() - Ai.lastDiscovery >= 30000;
    const result = await call('poll_open_rooms', { discoverAllowed: allowed });
    if (result.discovered) Ai.lastDiscovery = Date.now();
    if (!aiIdle() || Ai.bots.running || !result.changed) return;
    applyRooms(result);
  } catch (e) { setAiStatus('열린 방 갱신 실패: ' + errorText(e)); }
  finally { Ai.refreshingOpen = false; }
}

async function startBots() {
  if (!canStartBots() || !(await saveSettings())) return;
  await aiWork(async () => { setAiStatus(await call('start_bots', { keys: Ai.botOrder })); });
}

function applyBots(snapshot) {
  Ai.bots = snapshot;
  $('badge').textContent = snapshot.badge;
  $('botStatuses').replaceChildren(...snapshot.statuses.map(s => el('p', { text: s, title: s })));
  if (snapshot.lastError) setAiStatus(snapshot.lastError);
  refreshAll();
}

// ── 페르소나 ──
function editingProfile() { return personaById(Ai.editingId) || App.settings?.personas[0]; }
function renderPersonaEditor() {
  const s = App.settings; if (!s) return;
  const p = editingProfile(); Ai.editingId = p.id;
  setOptions($('editPersona'), s.personas.map(x => [x.id, x.persona.name]), p.id);
  $('pName').value = p.persona.name; $('pRole').value = p.persona.role; $('pTone').value = p.persona.tone;
  $('pStyle').value = p.persona.style; $('pInstructions').value = p.persona.instructions; $('pAttach').checked = p.attachImages;
}
function bindPersonaField(id, field) {
  $(id).addEventListener('input', () => {
    const p = editingProfile(); if (!p) return;
    p.persona[field] = $(id).value;
    if (field === 'name') { setOptions($('editPersona'), App.settings.personas.map(x => [x.id, x.persona.name]), p.id); renderRoomsAll(); updateBinding(); }
    queueSave();
  });
}

// ── 프로바이더 ──
function modelStatus(provider) {
  const catalog = Ai.catalogs[provider.id] || { status: '모델 목록 확인 전', models: [] };
  const unverified = provider.model && !catalog.models.includes(provider.model);
  return catalog.status + (unverified ? ' · 현재 선택은 목록 미확인' : '');
}
function renderProviders() {
  const s = App.settings; if (!s) return;
  $('grokDelegation').checked = s.grokDelegationEnabled;
  $('provider').value = s.provider;
  $('contextMessages').value = s.contextMessages; $('timeoutSeconds').value = s.timeoutSeconds;
  $('providerCards').replaceChildren(...s.providers.map(provider => {
    const listId = 'models-' + provider.id;
    const catalog = Ai.catalogs[provider.id];
    const choices = catalog?.choices || [''];
    const enabled = el('input', { type: 'checkbox', checked: provider.enabled, onchange: () => { provider.enabled = enabled.checked; queueSave(); } });
    const model = el('input', { class: 'model', value: provider.model, title: '모델 · 비우면 CLI 기본값', ariaLabel: 'AI 모델', oninput: () => { provider.model = model.value.trim(); status.textContent = modelStatus(provider); queueSave(); } });
    model.setAttribute('list', listId);
    const effort = el('input', { class: 'effort', value: provider.effort, ariaLabel: 'AI effort', oninput: () => { provider.effort = effort.value.trim(); queueSave(); } });
    effort.setAttribute('list', 'effort-choices');
    const status = el('p', { class: 'muted small', text: modelStatus(provider) });
    const exe = el('input', { value: provider.executable, ariaLabel: 'AI 실행 파일 경로', title: '비우면 설치된 CLI 자동 탐색 · 경로 변경 후 모델 새로고침', oninput: () => {
      if (provider.executable === exe.value) return;
      provider.executable = exe.value;
      Ai.catalogs[provider.id] = { status: '실행 파일 변경 · 모델 목록을 새로고침하세요.', models: [], choices: [''] };
      status.textContent = modelStatus(provider); queueSave();
    } });
    return el('div', { class: 'provider-card' }, [
      el('div', { class: 'row' }, [el('label', { class: 'check' }, [enabled, document.createTextNode(provider.id)]), model, effort]),
      el('datalist', { id: listId }, choices.filter(Boolean).map(c => new Option(c, c))),
      status, exe,
    ]);
  }));
}
/// 원본 Detect. 프로바이더별 모델 목록을 동시에 조회하고, 그사이 설정이 바뀌면 결과를 버린다.
async function detect() {
  const generation = ++Ai.catalogGeneration;
  const settings = App.settings;
  try { $('cliInfo').textContent = await call('detect_providers', { settings }); } catch (e) { $('cliInfo').textContent = errorText(e); }
  for (const p of settings.providers) Ai.catalogs[p.id] = { status: '설치된 CLI에서 모델 조회 중…', models: [], choices: [''] };
  renderProviders();
  await Promise.all(settings.providers.map(async p => {
    const executable = p.executable;
    const result = await call('query_models', { id: p.id, executable, model: p.model });
    if (generation !== Ai.catalogGeneration || App.settings !== settings || p.executable !== executable) return;
    Ai.catalogs[p.id] = result;
    renderProviders();
  }));
}

// ── 대화분석 ──
function renderChatRooms() {
  const select = $('chatRoom');
  setOptions(select, Ai.rooms.map((r, i) => {
    const name = personaName(r);
    const chips = [name, MODE_CHIPS[replyModeFor(r)]].filter(Boolean).join(' · ');
    return [String(i), chips ? `${r.title}  [${chips}]` : r.title];
  }));
  select.value = Ai.selectedRoom ? String(Ai.rooms.findIndex(r => sameRoom(r, Ai.selectedRoom))) : '';
  select.title = Ai.selectedRoom?.display || '';
}
function selectChatRoom(room) {
  Ai.selectedRoom = room;
  App.settings.selectedChatRoom = room ? room.key : null; queueSave();
  $('transcript').value = ''; $('contextHint').textContent = '대화 읽기로 문맥을 확인하세요.';
  refreshAi();
}
function replaceRoom(room) {
  Ai.rooms = Ai.rooms.map(r => (sameRoom(r, room) ? room : r));
  if (sameRoom(Ai.selectedRoom, room)) Ai.selectedRoom = room;
}
async function readChat() {
  const room = Ai.selectedRoom;
  if (!room || !aiIdle() || !(await saveSettings())) return;
  await aiWork(async () => {
    setAiStatus('로컬 대화 읽는 중');
    const result = await call('read_conversation', { profile: room.profile, id: room.id });
    replaceRoom(result.room); renderChatRooms();
    $('transcript').value = result.transcript; $('contextHint').textContent = result.hint;
    setAiStatus('로컬 대화 읽기 완료 · 아직 AI에 전달하지 않았습니다.');
  });
}
async function generate(mode) {
  const room = Ai.selectedRoom;
  if (!room || !aiIdle() || !(await saveSettings())) return;
  await aiWork(async () => {
    Ai.generating = true; Ai.answerRoom = null; Ai.summaryRoom = null; Ai.summaryPersonaId = null;
    $('answer').value = ''; setAiStatus('대화 문맥으로 AI 응답 생성 중'); refreshAi();
    const result = await call('generate', { profile: room.profile, id: room.id, mode, question: $('question').value });
    replaceRoom(result.room); renderChatRooms();
    $('answer').value = result.text;
    Ai.answerRoom = mode === 'reply' ? result.room : null;
    Ai.summaryRoom = mode === 'analyze' ? result.room : null;
    Ai.summaryPersonaId = mode === 'analyze' ? result.personaId : null;
    setAiStatus(result.status);
  });
}
async function exportChat() {
  const room = Ai.selectedRoom;
  if (!room || !aiIdle() || !(await saveSettings())) return;
  await aiWork(async () => { const text = await call('export_conversation', { profile: room.profile, id: room.id }); if (text) setAiStatus(text); else setAiStatus('저장을 취소했습니다.'); });
}
/// Portable 가져오기. AI 생성 전에는 외부 서비스에 전달하지 않는다.
async function importChat() {
  await aiWork(async () => {
    const room = await call('import_conversation');
    if (!room) { setAiStatus('가져오기를 취소했습니다.'); return; }
    Ai.rooms = Ai.rooms.filter(r => !sameRoom(r, room)).concat([room]);
    Ai.selectedRoom = room; renderChatRooms();
    $('transcript').value = ''; $('contextHint').textContent = '대화 읽기로 문맥을 확인하세요.';
    setAiStatus('대화를 가져왔습니다. AI 생성 전에는 외부 서비스에 전달하지 않습니다.');
  });
}
async function saveNote() {
  if (!canSaveNote() || !(await saveSettings())) return;
  const room = Ai.summaryRoom; const personaId = Ai.summaryPersonaId; const body = $('answer').value;
  await aiWork(async () => {
    setAiStatus(await call('save_summary_note', { profile: room.profile, id: room.id, personaId, body }));
    Ai.summaryRoom = null; Ai.summaryPersonaId = null;
  });
}
const canSaveNote = () => aiIdle() && Ai.summaryRoom && Ai.summaryPersonaId && $('answer').value.trim();
const canCompose = () => aiIdle() && Ai.answerRoom && $('answer').value.trim();

function refreshAi() {
  const idle = aiIdle(); const start = canStartBots(); const canRead = idle && !!Ai.selectedRoom;
  setEnabled(['aiSave', 'chatRoom', 'chatRoomsRefresh', 'question', 'importChat'], idle);
  setEnabled(['pollSeconds', 'selfId', 'botRoomsRefresh', 'botStart'], start);
  $('botRooms').classList.toggle('disabled', !start);
  $('botStop').disabled = !Ai.bots.running;
  setEnabled(['bindPersona', 'bindMode', 'bindButton'], idle && !!bindingRoom());
  $('bindGreeting').disabled = !bindingRoom();
  setEnabled(['readChat', 'exportChat', ...document.querySelectorAll('#ai-analysis [data-mode]')], canRead);
  $('aiCancel').disabled = !Ai.busy;
  $('answer').readOnly = Ai.busy;
  $('answerWaiting').classList.toggle('hidden', !Ai.generating);
  $('saveNote').disabled = !canSaveNote();
  $('compose').disabled = !canCompose();
  $('editPersona').disabled = !idle || Img.busy;
  document.querySelectorAll('#persona-text input, #persona-text textarea, #ai-providers input, #ai-providers select, #ai-providers button').forEach(n => { n.disabled = !idle; });
  $('badge').textContent = Ai.bots.badge;
  renderActivity();
}
App.refreshers.push(refreshAi);

function renderSettings() {
  const s = App.settings; if (!s) return;
  $('pollSeconds').value = s.pollSeconds; $('selfId').value = s.selfId;
  renderPersonaEditor(); renderProviders(); renderRoomsAll(); updateBinding();
}
App.onSettings.push(renderSettings);

function intInput(id, apply) {
  $(id).addEventListener('input', () => { const v = $(id).value.trim(); if (/^-?\d+$/.test(v)) { apply(Number(v)); queueSave(); } });
}

function initAi(boot) {
  Ai.status = boot.settingsError || '준비';
  renderSettings();
  $('aiSave').onclick = async () => { if (aiIdle() && (await saveSettings())) setAiStatus('페르소나·프로바이더·지식베이스 설정을 저장했습니다.'); };
  $('botRoomsRefresh').onclick = refreshRooms; $('chatRoomsRefresh').onclick = refreshRooms;
  $('bindButton').onclick = bindPersona;
  $('bindGreeting').onchange = toggleGreeting;
  call('greeting_rooms').then(keys => { Ai.greetings = new Set(keys); renderBotRooms(); updateBinding(); }).catch(e => setAiStatus(errorText(e)));
  $('botStart').onclick = startBots;
  $('botStop').onclick = () => call('stop_bots');
  intInput('pollSeconds', v => { App.settings.pollSeconds = v; });
  $('selfId').addEventListener('input', () => { App.settings.selfId = $('selfId').value; queueSave(); });
  $('editPersona').onchange = () => { if (Img.busy) { $('editPersona').value = Ai.editingId; return; } Ai.editingId = $('editPersona').value; renderPersonaEditor(); imagesSwitchPersona(); };
  for (const [id, field] of [['pName', 'name'], ['pRole', 'role'], ['pTone', 'tone'], ['pStyle', 'style'], ['pInstructions', 'instructions']]) bindPersonaField(id, field);
  $('pAttach').onchange = () => { editingProfile().attachImages = $('pAttach').checked; queueSave(); };
  $('grokDelegation').onchange = () => { App.settings.grokDelegationEnabled = $('grokDelegation').checked; queueSave(); };
  $('provider').onchange = () => { App.settings.provider = $('provider').value; queueSave(); };
  intInput('contextMessages', v => { App.settings.contextMessages = v; });
  intInput('timeoutSeconds', v => { App.settings.timeoutSeconds = v; });
  document.body.append(el('datalist', { id: 'effort-choices' }, EFFORTS.map(e => new Option(e, e))));
  $('detect').onclick = detect;
  $('chatRoom').onchange = () => selectChatRoom(Ai.rooms[Number($('chatRoom').value)] || null);
  $('readChat').onclick = readChat; $('exportChat').onclick = exportChat; $('importChat').onclick = importChat;
  document.querySelectorAll('#ai-analysis [data-mode]').forEach(b => b.onclick = () => generate(b.dataset.mode));
  $('aiCancel').onclick = () => call('cancel_ai');
  $('answer').addEventListener('input', refreshAi);
  $('saveNote').onclick = saveNote;
  $('compose').onclick = () => { if (canCompose()) composeFromAi(Ai.answerRoom, $('answer').value); };
  listen('ai-progress', text => { if (Ai.busy) setAiStatus(text); });
  listen('ai-status', setAiStatus);
  listen('bots', applyBots);
  setInterval(pollOpenRooms, 2000);
  call('bot_state').then(applyBots);
  detect();
  pollOpenRooms();
}
