'use strict';
// 제어방 하위 탭. 원본: CommandRoomPanel.xaml(.cs).
const Ctl = { draft: null, rooms: [], editing: false };

function renderControl() {
  const d = Ctl.draft; if (!d) return;
  $('ctlEnabled').checked = d.enabled; $('ctlNatural').checked = d.naturalLanguage;
  $('ctlReply').checked = d.reply; $('ctlKnowledge').checked = d.knowledge; $('ctlForward').checked = d.forward; $('ctlProvider').checked = d.provider;
  renderControlRooms();
}
function selectedControlRoom() { return Ctl.rooms[Number($('ctlRoom').value)] || null; }
function renderControlRooms(selected) {
  const keep = selected || selectedControlRoom();
  setOptions($('ctlRoom'), [['', '']].concat(Ctl.rooms.map((r, i) => [String(i), r.title])));
  const index = Ctl.rooms.findIndex(r => sameRoom(r, keep));
  $('ctlRoom').value = index >= 0 ? String(index) : '';
  renderOperator();
}
function renderOperator() {
  const room = selectedControlRoom();
  $('ctlOperator').textContent = room?.directMemberId ? '허용 상대 ID: ' + room.directMemberId : '일반 1:1 방을 선택하세요.';
}
function collectDraft() {
  Object.assign(Ctl.draft, {
    enabled: $('ctlEnabled').checked, naturalLanguage: $('ctlNatural').checked,
    reply: $('ctlReply').checked, knowledge: $('ctlKnowledge').checked, forward: $('ctlForward').checked, provider: $('ctlProvider').checked,
  });
}
async function loadControl() {
  const state = await call('control_load');
  Ctl.draft = state.draft;
  if (state.room && !Ctl.rooms.some(r => sameRoom(r, state.room))) Ctl.rooms.push(state.room);
  renderControl(); renderControlRooms(state.room);
  $('ctlStatus').textContent = state.status;
  $('ctl-history').value = state.history; $('ctl-help').value = state.help;
}
async function refreshControlRooms() {
  if (Ctl.editing) return;
  Ctl.editing = true;
  const previous = selectedControlRoom();
  try { Ctl.rooms = await call('control_rooms'); renderControlRooms(previous); }
  catch (e) { $('ctlStatus').textContent = errorText(e); }
  finally { Ctl.editing = false; }
}
async function applyControl() {
  if (Ctl.editing || !Ctl.draft) return;
  Ctl.editing = true; collectDraft();
  try { $('ctlStatus').textContent = await call('control_apply', { draft: Ctl.draft, room: selectedControlRoom() }); await loadControl(); }
  catch (e) { $('ctlStatus').textContent = errorText(e); }
  finally { Ctl.editing = false; }
}
function initControl() {
  $('ctlRefresh').onclick = refreshControlRooms;
  $('ctlApply').onclick = applyControl;
  $('ctlRoom').onchange = renderOperator;
  listen('command-status', text => { $('ctlStatus').textContent = text; call('control_load').then(s => { $('ctl-history').value = s.history; }); });
  loadControl().catch(e => { $('ctlStatus').textContent = errorText(e); });
}
