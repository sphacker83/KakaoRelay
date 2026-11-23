'use strict';
// 메시지·API 탭. 원본: MainWindow.xaml(.cs).
const Msg = {
  working: false,        // 원본 working(수동 작업)
  targets: [],
  selected: null,        // 선택한 ConversationTarget
  targetReady: false,
  submitted: false,      // compose.Submitted
  current: null,         // currentReceipt
  history: [],
  selectedReceipt: null,
  report: null,
  apiDocs: '', openApi: '',
};

function canEdit() { return !Msg.working && !App.shuttingDown; }
function canSend() { return canEdit() && Msg.targetReady && Msg.selected?.selectable && $('messageBody').value.trim().length > 0; }

function refreshMessages() {
  const edit = canEdit();
  setEnabled(['targets', 'targetsRefresh', 'messageBody', 'historyRefresh', 'diagnose'], edit);
  $('sendButton').disabled = !canSend();
  const observable = Msg.current?.observable === true;
  $('observeCurrent').classList.toggle('hidden', !observable);
  $('observeCurrent').disabled = Msg.working || !observable;
  $('observeHistory').disabled = Msg.working || Msg.selectedReceipt?.observable !== true;
  $('sendProgress').classList.toggle('hidden', !Msg.working);
  $('sendResult').classList.toggle('hidden', !(Msg.submitted || Msg.current));
  $('charCount').textContent = `${$('messageBody').value.length.toLocaleString('ko-KR')}자`;
  $('openReport').disabled = !Msg.report;
}
App.refreshers.push(refreshMessages);

async function working(task) {
  Msg.working = true; refreshMessages();
  try { return await task(); } finally { Msg.working = false; refreshMessages(); }
}

function setSend(status, detail) {
  $('sendStatus').textContent = status; $('sendDetail').textContent = detail;
}

function renderTargets() {
  const select = $('targets');
  select.replaceChildren(new Option('', ''), ...Msg.targets.map((t, i) => {
    const option = new Option(t.title, String(i));
    option.disabled = !t.selectable; option.title = t.hint;
    return option;
  }));
  select.value = '';
}

/// 원본 RefreshRoomsAsync.
async function refreshTargets() {
  if (!canEdit()) return;
  await working(async () => {
    selectTarget(null);
    try {
      Msg.targets = await call('scan_rooms');
      renderTargets();
      $('targets').title = Msg.targets.length === 0 ? '열린 대화방이 없습니다.' : `열린 대화방 ${Msg.targets.length}개`;
      $('targetHint').textContent = Msg.targets.length === 0 ? '카카오톡에서 대화방을 열고 ↻를 눌러주세요.' : '받는 대화방을 선택하세요.';
    } catch (e) {
      Msg.targets = []; renderTargets();
      $('targetHint').textContent = errorText(e); $('targets').title = errorText(e);
    }
  });
}

function resetCompose() { Msg.submitted = false; call('compose_reset'); }

/// 원본 SelectedConversation setter + CheckTargetAsync.
function selectTarget(target) {
  if (Msg.selected === target) return;
  Msg.selected = target; Msg.targetReady = false; resetCompose();
  $('targetHint').textContent = target ? '대화방을 확인하고 있습니다.' : '카카오톡에서 대화방을 열고 위에서 선택하세요.';
  refreshMessages();
  if (target) checkTarget(target);
}

async function checkTarget(target) {
  if (Msg.working) return;
  await working(async () => {
    try {
      const result = await call('check_target', { target });
      if (Msg.selected !== target) return;
      Msg.targetReady = result.ready; $('targetHint').textContent = result.hint;
    } catch (e) { Msg.targetReady = false; $('targetHint').textContent = errorText(e); }
  });
}

/// 원본 Send_Click. 같은 작성 세션은 같은 요청 ID로 다시 시도한다.
async function sendManual() {
  if (!canSend()) return;
  Msg.submitted = true;
  setSend('앱이 메시지를 보내는 중', '대화방과 본문을 다시 확인한 뒤 전송을 요청합니다.');
  await working(async () => {
    try {
      const result = await call('send_manual', { target: Msg.selected, body: $('messageBody').value });
      setSend(result.status, result.detail);
      if (result.receipt) Msg.current = result.receipt;
      if (result.cleared) { $('messageBody').value = ''; Msg.submitted = false; }
    } catch (e) { setSend('처리 결과 확인 필요', errorText(e)); }
  });
  await loadHistory();
}

async function loadHistory() {
  try {
    const selectedId = Msg.selectedReceipt?.requestId;
    Msg.history = await call('history');
    const rows = Msg.history.map(item => {
      const row = el('tr', { onclick: () => selectReceipt(item) }, [
        el('td', { text: item.displayTime }), el('td', { text: item.recipient, title: item.recipient }), el('td', { text: item.displayStatus }),
      ]);
      row.dataset.id = item.requestId;
      return row;
    });
    $('historyRows').replaceChildren(...rows);
    selectReceipt(Msg.history.find(r => r.requestId === selectedId) || null, true);
  } catch (e) { $('historyHint').textContent = errorText(e); }
  refreshMessages();
}

function selectReceipt(item, keepHint) {
  Msg.selectedReceipt = item;
  document.querySelectorAll('#historyRows tr').forEach(r => r.classList.toggle('selected', !!item && r.dataset.id === item.requestId));
  if (!keepHint || item) $('historyHint').textContent = item ? item.describe : '확인할 기록을 선택하세요.';
  refreshMessages();
}

/// 원본 Observe.
async function observe(receipt) {
  try {
    await call('observe', { requestId: receipt.requestId, fingerprint: receipt.fingerprint });
    if (Msg.current?.requestId === receipt.requestId) {
      Msg.current = { ...Msg.current, observable: false };
      setSend('대화 확인 완료', '새 발신 말풍선 확인을 기록했습니다. 상대방 읽음 여부는 별도입니다.');
    }
    await loadHistory();
  } catch (e) { $('historyHint').textContent = errorText(e); $('sendDetail').textContent = errorText(e); }
  refreshMessages();
}

/// AI 챗봇 대화분석의 '발송란으로 가져오기'(원본 ComposeRequested).
async function composeFromAi(room, text) {
  if (!canEdit()) return false;
  if ($('messageBody').value.trim()) { setApiStatus('기존 작성 내용을 먼저 처리한 뒤 AI 답변을 가져오세요.'); return false; }
  await refreshTargets();
  const matches = Msg.targets.filter(t => t.title === room.title && t.selectable);
  if (matches.length !== 1) { setApiStatus('AI 답변을 받을 카카오톡 방을 열고 다시 가져오세요.'); return false; }
  $('messageBody').value = text; resetCompose();
  $('targets').value = String(Msg.targets.indexOf(matches[0]));
  selectTarget(matches[0]);
  showTab('main', 'tab-messages');
  return true;
}

function setApiStatus(text) { $('apiStatus').textContent = text; }

// 진단(API 탭)
function applyReport(report) {
  Msg.report = report.path;
  $('diagTitle').textContent = report.title; $('diagSummary').textContent = report.summary;
  const rows = report.windows.map((w, i) => {
    const row = el('tr', { onclick: () => selectWindow(report, i) }, [el('td', { text: w.displayTitle }), el('td', { text: w.displayState })]);
    return row;
  });
  $('diagWindows').replaceChildren(...rows);
  const first = report.windows.findIndex(w => w.visible);
  selectWindow(report, first);
  refreshMessages();
}
function selectWindow(report, index) {
  document.querySelectorAll('#diagWindows tr').forEach((r, i) => r.classList.toggle('selected', i === index));
  const controls = index >= 0 ? report.windows[index].controls : [];
  $('diagControls').replaceChildren(...controls.map(c => el('tr', {}, [el('td', { text: c.className }), el('td', { text: c.patterns })])));
}
async function diagnose() {
  if (Msg.working || App.shuttingDown) return;
  await working(async () => {
    $('diagTitle').textContent = '연결 진단 중';
    try { applyReport(await call('diagnose')); renderPermissions(await call('permissions')); }
    catch (e) { $('diagTitle').textContent = '진단 중단'; $('diagSummary').textContent = errorText(e); }
  });
}
async function loadReport(path) {
  showTab('main', 'tab-api'); $('diagPanel').open = true;
  try { applyReport(await call('load_report', { path })); }
  catch { $('diagTitle').textContent = '결과를 열 수 없습니다'; }
}

// 맥 권한 상태: 첫 실행 상태 영역(상단)과 진단 화면에 같이 보인다.
function permissionRows(info) {
  return info.items.map(item => el('div', { class: 'perm' }, [
    el('span', { class: 'state ' + (item.granted ? 'ok' : 'need'), text: item.granted ? '✓' : '!' }),
    el('span', { class: 'grow', text: `${item.name} · ${item.detail}` }),
    item.granted ? null : el('button', { class: 'small', text: '시스템 설정 열기', onclick: () => call('open_target', { target: item.url }).catch(() => {}) }),
  ]));
}
function renderPermissions(info) {
  if (!info.supported) return;
  $('diagPermissions').replaceChildren(...permissionRows(info));
  $('diagPermissions').classList.remove('hidden');
  const missing = info.items.some(i => !i.granted);
  $('permItems').replaceChildren(...permissionRows(info));
  $('permissions').classList.toggle('hidden', !missing);
}

function initMessages(boot) {
  $('apiDocs').value = boot.apiDocs; $('openApi').value = boot.openApi;
  $('targetsRefresh').onclick = refreshTargets;
  $('targets').onchange = () => selectTarget($('targets').value === '' ? null : Msg.targets[Number($('targets').value)]);
  $('messageBody').oninput = () => { if (!canEdit()) return; resetCompose(); refreshMessages(); };
  $('sendButton').onclick = sendManual;
  $('historyRefresh').onclick = loadHistory;
  $('observeCurrent').onclick = () => Msg.current?.observable && observe(Msg.current);
  $('observeHistory').onclick = () => Msg.selectedReceipt?.observable && observe(Msg.selectedReceipt);
  $('historyFolder').onclick = () => call('open_history_folder').catch(e => { $('historyHint').textContent = errorText(e); });
  $('diagnose').onclick = diagnose;
  $('openReport').onclick = () => call('open_report').catch(e => { $('historyHint').textContent = errorText(e); });
  $('permRefresh').onclick = async () => renderPermissions(await call('permissions'));
  document.querySelectorAll('[data-copy]').forEach(button => button.onclick = async () => {
    const openApi = button.dataset.copy === 'openapi';
    const ok = await copyText(openApi ? $('openApi').value : $('apiDocs').value);
    $('apiCopyStatus').textContent = ok ? (openApi ? 'OpenAPI 명세 전체를 복사했습니다.' : 'API 사용법과 예제 전체를 복사했습니다.') : '클립보드를 사용 중입니다. 복사 버튼을 다시 눌러주세요.';
  });
  setApiStatus(boot.apiStatus);
  listen('api-status', setApiStatus);
  // API 발송 결과(원본 SendFromApiAsync의 SendStatus 갱신)
  // 이벤트는 문구만 전하므로 방금 저장된 최신 영수증을 현재 영수증으로 본다.
  listen('api-sent', async text => {
    await loadHistory();
    Msg.current = Msg.history[0] || null;
    setSend(text, Msg.current ? Msg.current.describe : '');
    refreshMessages();
  });
  renderPermissions(boot.permissions);
  refreshMessages();
  loadHistory().then(refreshTargets);
}
