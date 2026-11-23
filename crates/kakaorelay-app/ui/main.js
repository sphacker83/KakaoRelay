'use strict';
// 시작·종료. 원본: App.OnStartup의 화면 초기화, MainWindow.PrepareShutdown.
(async () => {
  const boot = await call('bootstrap');
  App.settings = boot.settings; App.settingsError = boot.settingsError; App.platform = boot.platform; App.hostReady = boot.hostReady;
  $('version').textContent = boot.version;
  initMessages(boot);
  initAi(boot);
  initImages();
  initForward();
  initControl();
  initKnowledge();
  if (boot.report) loadReport(boot.report);
  listen('host-ready', async () => { App.hostReady = true; applyBots(await call('bot_state')); loadControl(); refreshAll(); });
  // 제어방이 설정을 바꾸면 화면 설정을 다시 읽는다(원본 RemoteSettingsChanged).
  listen('settings-changed', () => reloadSettings());
  // 원본 PrepareShutdown: 새 작업을 막고 디바운스 중인 설정을 저장한다.
  listen('prepare-shutdown', async () => {
    App.shuttingDown = true; setApiStatus('작업 완료 후 종료 중'); refreshAll();
    if (saveTimer) await saveSettings();
    call('shutdown_ready');
  });
  refreshAll();
})().catch(e => { document.body.prepend(el('p', { class: 'strong', text: '화면을 시작하지 못했습니다: ' + errorText(e) })); });
