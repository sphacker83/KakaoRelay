//! 페르소나 이미지 하위 탭. 원본: `PersonaImagesPanel.xaml.cs`, `MagentaPng.cs`.
//! 이미지는 이 앱에서만 생성·변경한다(대화 명령·발송 API로는 바꿀 수 없다).

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use kakaorelay_ai::image_cli::PersonaImageCliGenerator;
use kakaorelay_ai::magenta;
use kakaorelay_ai::persona_images::{
    PersonaExpression, PersonaImageCatalog, PersonaImageService, PersonaImageStore,
};
use kakaorelay_ai::settings::{AiProviderSettings, AiSettings, PersonaProfile};
use serde_json::{Value, json};
use tauri::AppHandle;

use super::{App, CmdResult, emit, fail};

fn provider_name(provider: &str) -> &'static str {
    if provider == "astra" {
        "GPT · Codex CLI"
    } else {
        "Grok CLI"
    }
}

fn profile(settings: &AiSettings, persona_id: &str) -> CmdResult<PersonaProfile> {
    settings
        .personas
        .iter()
        .find(|p| p.id == persona_id)
        .cloned()
        .ok_or_else(|| "페르소나를 찾지 못했습니다.".to_owned())
}

fn catalog(persona_id: &str) -> CmdResult<PersonaImageCatalog> {
    Ok(PersonaImageCatalog::new(
        AiSettings::image_root(persona_id).map_err(fail)?,
    ))
}

fn store(persona_id: &str, provider: &str) -> CmdResult<PersonaImageStore> {
    catalog(persona_id)?.store(provider).map_err(fail)
}

/// 원본 `Preview`: 폭 320px로 줄인 PNG data URL. 화면은 이 값을 `img`에만 쓴다.
fn preview(path: &Path) -> Option<String> {
    let image = image::open(path).ok()?;
    let image = if image.width() > 320 {
        image.resize(320, u32::MAX, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let mut bytes = Cursor::new(vec![]);
    image.write_to(&mut bytes, image::ImageFormat::Png).ok()?;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
    ))
}

/// 원본 `Reload`. 이전 버전의 투명화 기준 이미지 연결을 고친 뒤 목록·미리보기·적용 상태를 돌려준다.
#[tauri::command]
pub async fn images_load(state: App<'_>, persona_id: String, provider: String) -> CmdResult<Value> {
    let profile = profile(&state.settings(), &persona_id)?;
    tokio::task::spawn_blocking(move || load(&profile, &provider))
        .await
        .map_err(fail)?
}

fn load(profile: &PersonaProfile, provider: &str) -> CmdResult<Value> {
    let catalog = catalog(&profile.id)?;
    let store = catalog.store(provider).map_err(fail)?;
    let repaired = store.repair_legacy_transparent_references(|source, candidate| {
        magenta::is_transparent_copy(source, candidate).unwrap_or(false)
    });
    let library = store
        .load()
        .map_err(|e| format!("이미지 목록 확인 필요: {e}"))?;
    let mut images = library.images.clone();
    images.sort_by_key(|i| std::cmp::Reverse(i.created_at));
    let items: Vec<Value> = images
        .iter()
        .map(|image| {
            let active = library.active.values().any(|id| id == &image.id);
            json!({
                "id": image.id,
                "label": format!("{}{}", if active { "✓ " } else { "" }, image.display()),
                "detail": format!("{} · {}", image.display(), image.model),
                "prompt": image.prompt,
                "thumbnail": store.image_path(&image.id).ok().and_then(|p| preview(&p)),
            })
        })
        .collect();
    let reference = library
        .reference_id
        .as_ref()
        .and_then(|id| store.image_path(id).ok())
        .and_then(|p| preview(&p));
    let applied = catalog.applied_provider().map_err(fail)?;
    let avatar = catalog
        .applied_image_path("neutral")
        .map_err(fail)?
        .and_then(|p| preview(&p));
    Ok(json!({
        "images": items,
        "reference": reference,
        "hasReference": library.reference_id.is_some(),
        "applied": match applied.as_deref() {
            None => "적용된 이미지 없음".to_owned(),
            Some(p) => format!("현재 적용: {} · {}", profile.label, if p == "astra" { "GPT" } else { "Grok" }),
        },
        "avatar": avatar,
        "active": library.active.keys().cloned().collect::<Vec<_>>(),
        "repairError": repaired.err().map(|e| e.to_string()),
    }))
}

/// 원본 `Generate`. `mode`: `reference`(기준 이미지), `selected`(선택 감정), `missing`(없는 감정 모두).
#[tauri::command]
pub async fn images_generate(
    app: AppHandle,
    state: App<'_>,
    persona_id: String,
    provider: String,
    mode: String,
    expression: String,
    direction: String,
) -> CmdResult<String> {
    let settings = state.settings();
    let persona = profile(&settings, &persona_id)?;
    let store = store(&persona_id, &provider)?;
    let library = store.load().map_err(fail)?;
    let reference = mode == "reference";
    let expressions: Vec<PersonaExpression> = match mode.as_str() {
        "reference" => vec![PersonaExpression::ALL[0]],
        "missing" => PersonaExpression::ALL
            .into_iter()
            .filter(|e| !library.active.contains_key(e.id))
            .collect(),
        _ => vec![PersonaExpression::find(&expression).map_err(fail)?],
    };
    if expressions.is_empty() {
        return Ok("모든 감정 이미지가 있습니다. 바꿀 감정을 선택해 재생성하세요.".into());
    }
    let (cli, model) = if provider == "astra" {
        ("codex", settings.image_gpt_model.clone())
    } else {
        ("grok", settings.image_grok_model.clone())
    };
    let configured = settings.provider(cli).map_err(fail)?;
    let generator = PersonaImageCliGenerator::new(
        AiProviderSettings {
            model: model.clone(),
            effort: "low".into(),
            executable: configured.executable.clone(),
            ..AiProviderSettings::new(cli)
        },
        Arc::new(magenta::normalize_png),
        None,
    );
    let service = PersonaImageService::new(store);
    let operation = state.ops.begin("images").map_err(fail)?;
    let suffix = if provider == "astra" {
        " / imagegen"
    } else {
        " / image_gen"
    };
    let mut outcome = Ok(());
    for (i, emotion) in expressions.iter().enumerate() {
        emit(
            &app,
            "images-status",
            format!(
                "{} · {} 생성 중 ({}/{})",
                provider_name(&provider),
                if reference {
                    "기준 이미지"
                } else {
                    emotion.name
                },
                i + 1,
                expressions.len()
            ),
        );
        let made = service
            .generate(
                &persona.persona,
                emotion.id,
                reference,
                &direction,
                &provider,
                &format!("{model}{suffix}"),
                &generator,
                &operation.token,
            )
            .await;
        match made {
            Ok(image) => emit(&app, "images-generated", image.id),
            Err(error) => {
                outcome = Err(error);
                break;
            }
        }
    }
    drop(operation);
    Ok(match outcome {
        Ok(()) => {
            "생성 완료 · 후보를 확인하고 ‘선택 이미지 채택’, ‘이 버전 적용’을 눌러주세요.".into()
        }
        Err(error) if error.is_cancelled() => {
            "생성을 중지했습니다. 이미 완료한 이미지와 기존 선택은 보관됩니다.".into()
        }
        Err(error) => format!("생성 실패: {error}"),
    })
}

/// 원본 `MakeTransparent_Click`. 결과는 별도 PNG 후보로 저장한다. 반환: (상태 문구, 선택할 이미지 ID).
#[tauri::command]
pub async fn images_transparent(
    state: App<'_>,
    persona_id: String,
    provider: String,
    image_id: String,
    tolerance: i32,
) -> CmdResult<Value> {
    let store = store(&persona_id, &provider)?;
    let operation = state.ops.begin("images").map_err(fail)?;
    let token = operation.token.clone();
    let source = image_id.clone();
    let converted = tokio::task::spawn_blocking(move || {
        let bytes = store.read(&source)?;
        let (png, changed) = magenta::convert(&bytes, tolerance, &token)?;
        if changed == 0 {
            return Ok((0, None));
        }
        if token.is_cancelled() {
            return Err(kakaorelay_ai::Error::Cancelled);
        }
        let copy = store.add_transparent_copy(&source, &png)?;
        Ok((changed, Some(copy.id)))
    })
    .await
    .map_err(fail)?;
    Ok(match converted {
        Ok((0, _)) => {
            json!({ "status": "투명화할 마젠타가 없습니다. 허용 오차를 조절해 보세요.", "selected": image_id })
        }
        Ok((changed, id)) => json!({
            "status": format!("{}개 픽셀 투명화 · 별도 PNG 후보로 저장했습니다. 사용하려면 ‘선택 이미지 채택’을 누르세요.", group(changed)),
            "selected": id,
        }),
        Err(error) if error.is_cancelled() => {
            json!({ "status": "투명화를 중지했습니다.", "selected": image_id })
        }
        Err(error) => json!({ "status": format!("투명화 실패: {error}"), "selected": image_id }),
    })
}

/// .NET `N0` 형식(천 단위 쉼표).
fn group(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 원본 `UseImage_Click`·`ApplyVersion_Click`·`DeleteImage_Click`.
#[tauri::command(async)]
pub fn images_manage(
    state: App<'_>,
    persona_id: String,
    provider: String,
    action: String,
    image_id: Option<String>,
) -> CmdResult<String> {
    if state.ops.busy("images") {
        return Err("다른 작업을 처리 중입니다. 잠시 후 다시 시도하세요.".into());
    }
    let catalog = catalog(&persona_id)?;
    let store = catalog.store(&provider).map_err(fail)?;
    match (action.as_str(), image_id) {
        ("use", Some(id)) => {
            store.use_image(&id).map_err(fail)?;
            Ok("선택한 이미지를 이 버전에 채택했습니다.".into())
        }
        ("delete", Some(id)) => {
            store.delete(&id).map_err(fail)?;
            Ok("선택한 후보를 삭제했습니다.".into())
        }
        ("apply", _) => {
            catalog.apply(&provider).map_err(fail)?;
            Ok(format!(
                "{}을 페르소나에 적용했습니다.",
                provider_name(&provider)
            ))
        }
        _ => Err("후보를 선택해 비교하세요.".into()),
    }
}

#[tauri::command]
pub fn images_open_folder(persona_id: String, provider: String) -> CmdResult<()> {
    let store = store(&persona_id, &provider)?;
    std::fs::create_dir_all(store.root()).map_err(fail)?;
    crate::os::open(&store.root().to_string_lossy()).map_err(fail)
}

#[tauri::command]
pub fn images_cancel(state: App<'_>) {
    state.ops.cancel("images");
}

#[cfg(test)]
mod tests {
    #[test]
    fn groups_thousands_like_dotnet_n0() {
        assert_eq!(super::group(7), "7");
        assert_eq!(super::group(1234), "1,234");
        assert_eq!(super::group(1234567), "1,234,567");
    }
}
