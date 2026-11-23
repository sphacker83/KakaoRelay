'use strict';
// 정리감시 하위 탭. 원본: SummaryForwardPanel.xaml(.cs).
const Fwd = { rules: [], selectedId: null, sources: [], destinations: [], loading: false };

const fwdRule = () => Fwd.rules.find(r => r.rule.id === Fwd.selectedId)?.rule || null;
const fwdCanEdit = () => !!App.settings && !Fwd.loading && !App.shuttingDown;
function setFwdStatus(text) { $('fwdStatus').textContent = text; $('fwdStatus').title = text; }

function refreshForward() {
  setEnabled(['fwdRules', 'fwdDelete', 'fwdNew', 'fwdRefresh', 'fwdDestination', 'fwdInterval', 'fwdEnabled', 'fwdApply'], fwdCanEdit());
  document.querySelectorAll('#fwdSources input').forEach(n => { n.disabled = !fwdCanEdit(); });
}
App.refreshers.push(refreshForward);

function loadRules() {
  Fwd.rules = (App.settings?.summaryForwards || []).map(rule => ({ rule, label: ruleLabel(rule) }));
}
/// 원본 SummaryForwardRule.Label과 같은 형식이다(Rust가 돌려준 label이 있으면 그 값을 쓴다).
function ruleLabel(rule) {
  return `${rule.sources.map(s => s.title).join(', ')} → ${rule.destination?.title || ''} · ${rule.intervalMinutes}분${rule.enabled ? '' : ' · 꺼짐'}`;
}
function renderRules() {
  setOptions($('fwdRules'), [['', '']].concat(Fwd.rules.map(r => [r.rule.id, r.label])), Fwd.selectedId || '');
}

/// 원본 LoadDraft: 선택한 규칙의 값을 편집란에 채운다.
function loadDraft() {
  const rule = fwdRule();
  $('fwdEnabled').checked = rule?.enabled || false;
  $('fwdInterval').value = rule?.intervalMinutes ?? 30;
  for (const source of rule?.sources || []) {
    if (!Fwd.sources.some(c => c.key === sourceKey(source))) Fwd.sources.push({ room: source, key: sourceKey(source), selected: false, availability: '' });
  }
  for (const choice of Fwd.sources) choice.selected = !!rule?.sources.some(r => sourceKey(r) === choice.key);
  const dest = rule?.destination;
  if (dest && !Fwd.destinations.some(d => d.key === sourceKey(dest))) Fwd.destinations.push({ room: dest, key: sourceKey(dest) });
  renderSources(); renderDestinations(dest ? sourceKey(dest) : '');
}
// 화면 안 비교용 키. 설정 저장 형식과 무관하다.
function sourceKey(room) { return JSON.stringify([room.profile, room.id]); }

function renderSources() {
  $('fwdSources').replaceChildren(...Fwd.sources.map(choice => {
    const box = el('input', { type: 'checkbox', checked: choice.selected, onchange: () => { choice.selected = box.checked; } });
    return el('li', {}, [el('label', { class: 'check grow' }, [box, el('span', {}, [
      el('span', { text: choice.room.title }), choice.availability ? el('span', { class: 'sub', text: choice.availability }) : null,
    ])])]);
  }));
  refreshForward();
}
function renderDestinations(key) {
  setOptions($('fwdDestination'), [['', '']].concat(Fwd.destinations.map(d => [d.key, d.room.title])), key);
}

/// 원본 RefreshRooms.
async function refreshForwardRooms() {
  if (!fwdCanEdit()) return;
  Fwd.loading = true; refreshForward();
  try {
    const selected = Fwd.sources.filter(s => s.selected).map(s => s.room);
    const rule = fwdRule();
    const chosen = Fwd.sources.length === 0 && rule ? rule.sources : selected;
    const dest = Fwd.destinations.find(d => d.key === $('fwdDestination').value)?.room || null;
    const result = await call('forward_rooms', { selected: chosen, retained: selected, destination: dest });
    const chosenKeys = new Set(chosen.map(sourceKey));
    Fwd.sources = result.sources.map(s => ({ room: s.room, key: sourceKey(s.room), selected: s.selected || chosenKeys.has(sourceKey(s.room)), availability: s.availability }));
    Fwd.destinations = result.destinations.map(d => ({ room: d.room, key: sourceKey(d.room) }));
    renderSources(); renderDestinations(dest ? sourceKey(dest) : '');
  } catch (e) { setFwdStatus(errorText(e)); }
  finally { Fwd.loading = false; refreshForward(); }
}

async function applyForward() {
  if (!fwdCanEdit()) return;
  try {
    const dest = Fwd.destinations.find(d => d.key === $('fwdDestination').value)?.room || null;
    const result = await call('forward_apply', {
      oldId: Fwd.selectedId, enabled: $('fwdEnabled').checked, interval: $('fwdInterval').value,
      sources: Fwd.sources.filter(s => s.selected).map(s => s.room), destination: dest,
    });
    App.settings = await call('get_settings');
    Fwd.rules = result.rules; Fwd.selectedId = result.selected;
    renderRules(); loadDraft(); setFwdStatus(result.status);
  } catch (e) { setFwdStatus(errorText(e)); }
}

async function deleteForward() {
  if (!fwdCanEdit() || !Fwd.selectedId) return;
  try {
    const result = await call('forward_delete', { id: Fwd.selectedId });
    App.settings = await call('get_settings');
    Fwd.rules = result.rules; Fwd.selectedId = Fwd.rules[0]?.rule.id || null;
    renderRules(); loadDraft();
  } catch (e) { setFwdStatus(errorText(e)); }
}

function initForward() {
  loadRules(); Fwd.selectedId = Fwd.rules[0]?.rule.id || null; renderRules(); loadDraft();
  $('fwdRules').onchange = () => { Fwd.selectedId = $('fwdRules').value || null; loadDraft(); };
  $('fwdNew').onclick = () => { Fwd.selectedId = null; renderRules(); loadDraft(); setFwdStatus('새 규칙 · 방과 주기를 지정하세요.'); };
  $('fwdDelete').onclick = deleteForward;
  $('fwdRefresh').onclick = refreshForwardRooms;
  $('fwdApply').onclick = applyForward;
  listen('forward-status', setFwdStatus);
  // 원본: 탭이 보일 때마다 방을 새로 읽는다.
  document.addEventListener('tabshown', e => { if (e.detail.id === 'ai-forward') refreshForwardRooms(); });
  App.onSettings.push(() => { const id = Fwd.selectedId; loadRules(); Fwd.selectedId = Fwd.rules.some(r => r.rule.id === id) ? id : Fwd.rules[0]?.rule.id || null; renderRules(); loadDraft(); });
}
