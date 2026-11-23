'use strict';
// 페르소나 이미지 하위 탭. 원본: PersonaImagesPanel.xaml(.cs).
const EXPRESSIONS = [['neutral', '기본'], ['happy', '기쁨'], ['sad', '슬픔'], ['surprised', '놀람'], ['angry', '화남'], ['thinking', '생각'], ['cheering', '응원']];
const Img = { busy: false, provider: 'astra', library: null, selected: null };

const imgIdle = () => !Img.busy && !!App.settings;
function setImgStatus(text) { $('imgStatus').textContent = text; }
function personaId() { return editingProfile()?.id; }

function refreshImages() {
  const idle = imgIdle(); const hasRef = !!Img.library?.hasReference; const hasSel = !!Img.selected;
  setEnabled(['imgProvider', 'imgGptModel', 'imgGrokModel', 'imgDirection', 'imgReference', 'imgExpression', 'imgTolerance', 'imgFolder'], idle);
  setEnabled(['imgApply', 'imgGenerate', 'imgMissing'], idle && hasRef);
  setEnabled(['imgTransparent', 'imgUse', 'imgDelete'], idle && hasSel);
  $('imgCopyPrompt').disabled = !Img.selected?.prompt?.trim();
  $('imgCancel').disabled = !Img.busy;
  $('imgProgress').classList.toggle('hidden', !Img.busy);
  $('imgList').classList.toggle('disabled', !idle);
}
App.refreshers.push(refreshImages);

function setImage(node, src) { if (src) node.src = src; else node.removeAttribute('src'); }

function selectImage(item) {
  Img.selected = item;
  document.querySelectorAll('#imgList li').forEach(li => li.classList.toggle('selected', !!item && li.dataset.id === item.id));
  setImage($('imgSelPreview'), item?.thumbnail);
  $('imgDetail').textContent = item ? item.detail : '후보를 선택해 비교하세요.';
  $('imgPrompt').value = item?.prompt?.trim() ? item.prompt : '저장된 프롬프트 없음';
  refreshImages();
}

/// 원본 Reload. 적용된 이미지는 AI 챗봇 머리글 아바타로도 쓴다.
async function imagesReload(selectedId) {
  const id = personaId(); if (!id) return;
  try {
    const lib = await call('images_load', { personaId: id, provider: Img.provider });
    Img.library = lib;
    $('imgList').replaceChildren(...lib.images.map(item => {
      const li = el('li', { onclick: () => imgIdle() && selectImage(item) }, [el('img', { alt: '' }), el('span', { text: item.label })]);
      setImage(li.firstChild, item.thumbnail); li.dataset.id = item.id;
      return li;
    }));
    setImage($('imgRefPreview'), lib.reference);
    $('imgApplied').textContent = lib.applied;
    setImage($('avatar'), lib.avatar); $('avatar').classList.toggle('hidden', !lib.avatar);
    if (lib.repairError) setImgStatus('이미지 목록 확인 필요: ' + lib.repairError);
    selectImage(lib.images.find(i => i.id === selectedId) || lib.images[0] || null);
  } catch (e) { setImgStatus(errorText(e)); }
  refreshImages();
}
function imagesSwitchPersona() { if (!Img.busy) imagesReload(); }

async function imgOperation(task) {
  if (!imgIdle()) return;
  Img.busy = true; refreshAll();
  try { await task(); } catch (e) { setImgStatus(errorText(e)); }
  finally { Img.busy = false; refreshAll(); }
}

async function generateImages(mode) {
  if (!imgIdle() || !(await saveSettings())) return;
  let last = null;
  const unlisten = await listen('images-generated', id => { last = id; imagesReload(id); });
  await imgOperation(async () => {
    setImgStatus(await call('images_generate', {
      personaId: personaId(), provider: Img.provider, mode, expression: $('imgExpression').value, direction: $('imgDirection').value,
    }));
  });
  unlisten();
  await imagesReload(last);
}

function manage(action) {
  if (!imgIdle()) return;
  call('images_manage', { personaId: personaId(), provider: Img.provider, action, imageId: Img.selected?.id || null })
    .then(text => { setImgStatus(text); imagesReload(Img.selected?.id); }, e => setImgStatus(errorText(e)));
}

function initImages() {
  setOptions($('imgExpression'), EXPRESSIONS, 'happy');
  const s = App.settings;
  $('imgGptModel').value = s.imageGptModel; $('imgGrokModel').value = s.imageGrokModel;
  $('imgGptModel').addEventListener('input', () => { App.settings.imageGptModel = $('imgGptModel').value; queueSave(); });
  $('imgGrokModel').addEventListener('input', () => { App.settings.imageGrokModel = $('imgGrokModel').value; queueSave(); });
  $('imgProvider').onchange = () => { if (Img.busy) { $('imgProvider').value = Img.provider; return; } Img.provider = $('imgProvider').value; imagesReload(); };
  $('imgReference').onclick = () => generateImages('reference');
  $('imgGenerate').onclick = () => generateImages('selected');
  $('imgMissing').onclick = () => generateImages('missing');
  $('imgTolerance').oninput = () => { $('imgTolText').textContent = $('imgTolerance').value; };
  $('imgTransparent').onclick = () => {
    if (!imgIdle() || !Img.selected) return;
    setImgStatus('선택한 이미지의 마젠타를 투명화하는 중…');
    let selected = Img.selected.id;
    imgOperation(async () => {
      const result = await call('images_transparent', { personaId: personaId(), provider: Img.provider, imageId: Img.selected.id, tolerance: Number($('imgTolerance').value) });
      setImgStatus(result.status); selected = result.selected;
    }).then(() => imagesReload(selected));
  };
  $('imgCopyPrompt').onclick = async () => setImgStatus((await copyText(Img.selected.prompt)) ? '생성 프롬프트를 복사했습니다.' : '복사 실패');
  $('imgUse').onclick = () => manage('use');
  $('imgDelete').onclick = () => manage('delete');
  $('imgApply').onclick = () => manage('apply');
  $('imgFolder').onclick = () => call('images_open_folder', { personaId: personaId(), provider: Img.provider }).catch(e => setImgStatus(errorText(e)));
  $('imgCancel').onclick = () => call('images_cancel');
  listen('images-status', text => { if (Img.busy) setImgStatus(text); });
  App.onSettings.push(() => { $('imgGptModel').value = App.settings.imageGptModel; $('imgGrokModel').value = App.settings.imageGrokModel; });
  imagesReload();
}
