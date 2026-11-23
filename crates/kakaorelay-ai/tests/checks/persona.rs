//! 원본: `PersonaChecks.cs`. Core 의존 검사 5개는 core에 있다: 호출어 후보 1개는 `kakaorelay-core/tests/checks/auto_reply.rs`, 이미지 발송 원장 3개와 API 멱등 지문 1개는 `kakaorelay-core/tests/checks/persona.rs`.

use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use kakaorelay_ai::ai_service::AiService;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::magenta;
use kakaorelay_ai::persona_images::{
    PersonaAttachment, PersonaExpression, PersonaImageCatalog, PersonaImageGenerator,
    PersonaImageService, PersonaImageStore,
};
use kakaorelay_ai::settings::{AiSettings, AiSettingsStore, BotPersona, room_key};
use kakaorelay_ai::{Error, Result};

use crate::image_cli::png;
use crate::support::Check;

#[derive(Default)]
struct Generator {
    references: Mutex<Vec<Option<Vec<u8>>>>,
    prompts: Mutex<Vec<String>>,
    fail: Mutex<bool>,
}

#[async_trait]
impl PersonaImageGenerator for Generator {
    async fn generate(
        &self,
        prompt: &str,
        reference: Option<&[u8]>,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        self.references
            .lock()
            .unwrap()
            .push(reference.map(<[u8]>::to_vec));
        self.prompts.lock().unwrap().push(prompt.into());
        if *self.fail.lock().unwrap() {
            return Err(Error::io("fixture failure"));
        }
        Ok(png())
    }
}

const PIXELS: [u8; 20] = [
    255, 0, 255, 255, 240, 15, 245, 128, 0, 0, 255, 255, 255, 255, 255, 255, 255, 0, 255, 0,
];

pub async fn run(root: &Path, check: &Check) {
    let none = CancellationToken::new();
    let settings_store = AiSettingsStore::new(root.join("personas").join("settings.json"));
    let mut settings = AiSettings::default();
    settings.set_persona(BotPersona {
        name: "기존 봇".into(),
        ..BotPersona::default()
    });
    settings_store.save(&mut settings).unwrap();
    let mut settings = settings_store.load().unwrap();
    check.check(
        settings.personas.len() == 3 && settings.personas[0].persona.name == "기존 봇",
        "Legacy persona migrates into three slots without changing its personality",
    );
    let (work, friend) = (
        settings.personas[1].id.clone(),
        settings.personas[2].id.clone(),
    );
    settings
        .room_personas
        .insert(room_key("profile-a", "100"), work);
    settings
        .room_personas
        .insert(room_key("profile-b", "100"), friend);
    settings_store.save(&mut settings).unwrap();
    let mut settings = settings_store.load().unwrap();
    check.check(
        settings.resolve_persona("profile-a", "100").unwrap().id == "work"
            && settings.resolve_persona("profile-b", "100").unwrap().id == "friend"
            && settings.resolve_persona("profile-a", "101").unwrap().id == "default",
        "Room mappings persist and separate equal room IDs across profiles with a default fallback",
    );
    check.check(
        settings.trigger_for_room("profile-a", "100").unwrap()
            == format!("@{}", settings.personas[1].persona.name)
            && settings.trigger() == format!("@{}", settings.persona().name),
        "Mention is forced to the connected bot name; custom legacy triggers cannot override it",
    );
    settings.personas[1].persona.name = "새봇".into();
    check.check(
        settings.trigger_for_room("profile-a", "100").unwrap() == "@새봇",
        "Renaming the connected persona updates its room mention",
    );
    check.check(
        AiService::parse_reply("답변\n[[emotion:happy]]") == ("답변".into(), "happy".into())
            && AiService::parse_reply("일반 답변") == ("일반 답변".into(), "neutral".into()),
        "Emotion metadata is removed from sent text and missing metadata falls back to neutral",
    );
    let catalog = PersonaImageCatalog::new(root.join("images"));
    let store = catalog.store("astra").unwrap();
    let service = PersonaImageService::new(catalog.store("astra").unwrap());
    let generator = Generator::default();
    let generate = |expression: &'static str, reference: bool, style: &'static str| {
        let (service, generator, persona) = (&service, &generator, settings.persona().clone());
        async move {
            service
                .generate(
                    &persona,
                    expression,
                    reference,
                    style,
                    "astra",
                    "fixture",
                    generator,
                    &CancellationToken::new(),
                )
                .await
        }
    };
    let basis = generate("neutral", true, "simple").await.unwrap();
    check.check(
        std::fs::read_to_string(store.root().join("library.json"))
            .unwrap()
            .contains(&basis.persona_name),
        "Image library metadata stores Korean persona names directly",
    );
    let happy = generate("happy", false, "").await.unwrap();
    let another = generate("happy", false, "").await.unwrap();
    let reloaded = PersonaImageStore::new(store.root()).load().unwrap().images;
    let prompts: Vec<String> = [&basis, &happy, &another]
        .iter()
        .map(|i| {
            reloaded
                .iter()
                .find(|s| s.id == i.id)
                .unwrap()
                .prompt
                .clone()
        })
        .collect();
    check.check(
        prompts == *generator.prompts.lock().unwrap(),
        "Each image persists the exact generation prompt across library reloads",
    );
    {
        let references = generator.references.lock().unwrap();
        check.check(
            references[0].is_none()
                && references[1..]
                    .iter()
                    .all(|b| b.as_deref() == Some(&png()[..]))
                && happy.reference_id.as_ref() == Some(&basis.id)
                && another.reference_id.as_ref() == Some(&basis.id),
            "Every expression and regeneration reuses the original reference bytes",
        );
    }
    let library = store.load().unwrap();
    check.check(
        library.active["happy"] == happy.id && library.images.len() == 3,
        "Regeneration keeps the approved image and saves a separate candidate",
    );
    store.use_image(&another.id).unwrap();
    catalog.apply("astra").unwrap();
    check.check(
        catalog.applied_image_path("happy").unwrap()
            == Some(store.image_path(&another.id).unwrap()),
        "Applying a candidate changes only the chosen expression",
    );
    check.check(
        catalog.resolve_attachment("default", "happy").unwrap() == Some(PersonaAttachment { persona_id: "default".into(), provider: "astra".into(), image_id: another.id.clone() })
            && catalog.resolve_attachment("default", "sad").unwrap().is_none(),
        "Reply attachments select the adopted emotion image and omit missing expressions instead of sending neutral",
    );
    for emotion in PersonaExpression::ALL {
        let parsed = AiService::parse_reply(&format!("감정 답변\n[[emotion:{}]]", emotion.id));
        check.check(
            parsed == ("감정 답변".into(), emotion.id.into()),
            &format!(
                "Reply emotion {} is extracted without leaking the marker into the text",
                emotion.id
            ),
        );
    }
    *generator.fail.lock().unwrap() = true;
    assert!(generate("sad", false, "").await.unwrap_err().is_io());
    let library = store.load().unwrap();
    check.check(
        library.images.len() == 3 && library.active["happy"] == another.id,
        "Provider failure preserves the existing library and selection",
    );
    *generator.fail.lock().unwrap() = false;
    let next_basis = generate("neutral", true, "new").await.unwrap();
    check.check(
        store.load().unwrap().reference_id.as_ref() == Some(&basis.id),
        "New reference remains a candidate until explicitly adopted",
    );
    store.use_image(&next_basis.id).unwrap();
    check.check(
        store.load().unwrap().active.len() == 1 && catalog.applied_image_path("sad").unwrap() == Some(store.image_path(&next_basis.id).unwrap()),
        "Changing reference clears old expression selections and missing emotions use the new neutral reference",
    );
    check.check(
        matches!(store.use_image(&happy.id), Err(Error::InvalidOperation(_))),
        "Expressions from different characters cannot be mixed",
    );
    let grok = catalog.store("grok").unwrap();
    grok.add(
        "neutral",
        None,
        "other",
        "fixture",
        "grok",
        "fixture",
        &png(),
        false,
        None,
    )
    .unwrap();
    catalog.apply("grok").unwrap();
    check.check(
        catalog.applied_provider().unwrap().as_deref() == Some("grok")
            && store.load().unwrap().images.len() == 4
            && grok.load().unwrap().images.len() == 1,
        "Astra and Grok sets remain independent when switching applied version",
    );
    let neutral = catalog.resolve_attachment("friend", "neutral").unwrap();
    check.check(
        catalog.resolve_attachment("friend", "happy").unwrap().is_none()
            && neutral.is_some_and(|a| a.persona_id == "friend" && a.provider == "grok" && Some(a.image_id) == grok.load().unwrap().reference_id),
        "Neutral greeting uses the applied provider; missing emotions do not substitute its default image",
    );
    check.check(
        matches!(
            store.delete(&next_basis.id),
            Err(Error::InvalidOperation(_))
        ),
        "Active reference cannot be deleted",
    );
    check.check(
        matches!(store.image_path("../outside"), Err(Error::InvalidData(_))),
        "Image IDs cannot escape the managed library",
    );
    let mut exact = PIXELS;
    check.check(
        magenta::apply(&mut exact, 0, &none).unwrap() == 1
            && exact[..4].iter().all(|b| *b == 0)
            && exact[4..] == PIXELS[4..],
        "Exact magenta removal preserves near colors, existing transparency and other pixels",
    );
    let mut pixels = PIXELS;
    check.check(
        magenta::apply(&mut pixels, 30, &none).unwrap() == 2 && pixels[..8].iter().all(|b| *b == 0) && pixels[11] == 255 && pixels[15] == 255,
        "Magenta tolerance removes near-magenta including partial alpha while preserving red and white",
    );
    let original = store.read(&happy.id).unwrap();
    let before = store.load().unwrap();
    let transparent = store.add_transparent_copy(&happy.id, &png()).unwrap();
    let transparent_basis = store.add_transparent_copy(&next_basis.id, &png()).unwrap();
    let after = store.load().unwrap();
    let find = |id: &str| after.images.iter().find(|i| i.id == id).unwrap().clone();
    check.check(
        find(&transparent.id).prompt == happy.prompt
            && find(&transparent_basis.id).prompt == next_basis.prompt,
        "Transparent copies preserve their original image generation prompts",
    );
    check.check(
        transparent.id != happy.id
            && transparent.reference_id == happy.reference_id
            && transparent.expression == happy.expression
            && find(&transparent.id).is_transparent_copy
            && store.read(&happy.id).unwrap() == original,
        "Transparent candidate persists its expression and reference without overwriting the original",
    );
    let sorted = |m: &indexmap::IndexMap<String, String>| {
        let mut v: Vec<_> = m.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        v.sort();
        v
    };
    check.check(
        transparent_basis.is_reference()
            && after.reference_id == before.reference_id
            && sorted(&after.active) == sorted(&before.active),
        "Saving transparent reference and expression candidates leaves active selections unchanged",
    );
    transparent_families(&root.join("transparent-families"), check);
    settings.persona_mut().name = "호환성".into();
    settings_store.save(&mut settings).unwrap();
    check.check(
        settings_store.load().unwrap().personas[0].persona.name == "호환성",
        "Legacy persona edits update the first slot after migration",
    );
}

fn transparent_families(root: &Path, check: &Check) {
    let png = png();
    let store = PersonaImageStore::new(root);
    let add = |expression: &str, reference: Option<&str>, prompt: &str, transparent: bool| {
        store
            .add(
                expression,
                reference,
                "fixture",
                prompt,
                "astra",
                "fixture",
                &png,
                transparent,
                None,
            )
            .unwrap()
    };
    let basis = add("neutral", None, "reference", false);
    let happy = add("happy", Some(&basis.id), "expression", false);
    let transparent_happy = store.add_transparent_copy(&happy.id, &png).unwrap();
    store.use_image(&transparent_happy.id).unwrap();
    let transparent_basis = store.add_transparent_copy(&basis.id, &png).unwrap();
    store.use_image(&transparent_basis.id).unwrap();
    let loaded = PersonaImageStore::new(root).load().unwrap();
    check.check(
        loaded.active["happy"] == transparent_happy.id
            && loaded.reference_id.as_ref() == Some(&transparent_basis.id)
            && loaded.images.iter().find(|i| i.id == transparent_basis.id).unwrap().reference_family_id.as_ref() == Some(&basis.id),
        "Adopting a transparent reference preserves approved expressions and family identity after restart",
    );
    store.use_image(&happy.id).unwrap();
    store.use_image(&transparent_happy.id).unwrap();
    let sad = add("sad", Some(&transparent_basis.id), "sad", false);
    let transparent_sad = store.add_transparent_copy(&sad.id, &png).unwrap();
    store.use_image(&transparent_sad.id).unwrap();
    let twice = store
        .add_transparent_copy(&transparent_basis.id, &png)
        .unwrap();
    store.use_image(&twice.id).unwrap();
    store.use_image(&basis.id).unwrap();
    let library = store.load().unwrap();
    check.check(
        library.active["sad"] == transparent_sad.id && library.active["happy"] == transparent_happy.id && twice.reference_family_id.as_ref() == Some(&basis.id),
        "Original, transparent and repeatedly processed references accept expressions created from any member of the same family",
    );
    store.use_image(&twice.id).unwrap();
    let different = add("neutral", None, "reference", false);
    store.use_image(&different.id).unwrap();
    check.check(
        store.load().unwrap().active.len() == 1,
        "Regenerated references with identical prompts remain different characters",
    );
    check.check(
        matches!(
            store.use_image(&transparent_happy.id),
            Err(Error::InvalidOperation(_))
        ),
        "Transparent expressions from different characters remain rejected",
    );
    let legacy = add("neutral", None, "reference", true);
    check.check(
        store
            .repair_legacy_transparent_references(|_, _| true)
            .unwrap()
            == 0,
        "Ambiguous legacy reference copies are not linked based on matching prompts",
    );
    store.delete(&legacy.id).unwrap();
    let legacy_source = add("neutral", None, "legacy unique", false);
    let legacy_expression = add("happy", Some(&legacy_source.id), "legacy expression", false);
    let legacy_copy = add("neutral", None, "legacy unique", true);
    store.use_image(&legacy_copy.id).unwrap();
    check.check(
        store
            .repair_legacy_transparent_references(|_, _| false)
            .unwrap()
            == 0,
        "Legacy repair requires matching pixels even when metadata is unique",
    );
    check.check(
        store
            .repair_legacy_transparent_references(|_, _| true)
            .unwrap()
            == 1
            && store
                .repair_legacy_transparent_references(|_, _| true)
                .unwrap()
                == 0,
        "Verified legacy reference repair persists and is idempotent",
    );
    check.check(
        root.join("library.json.before-family-repair.bak").exists(),
        "Legacy family repair keeps the pre-migration manifest backup",
    );
    store.use_image(&legacy_expression.id).unwrap();
    check.check(store.load().unwrap().active["happy"] == legacy_expression.id, "Existing expressions can be adopted after repairing an already active legacy transparent reference");
    store.use_image(&different.id).unwrap();
    store.delete(&legacy_expression.id).unwrap();
    check.check(
        matches!(
            store.delete(&legacy_source.id),
            Err(Error::InvalidOperation(_))
        ),
        "A family root cannot be deleted while transparent references depend on it",
    );
    let none = CancellationToken::new();
    let mut exact = PIXELS;
    magenta::apply(&mut exact, 0, &none).unwrap();
    let mut near = PIXELS;
    magenta::apply(&mut near, 30, &none).unwrap();
    check.check(
        magenta::is_transparent_copy_pixels(&PIXELS, &exact)
            && magenta::is_transparent_copy_pixels(&PIXELS, &near),
        "Legacy pixel matching recognizes exact and tolerance-based magenta removal",
    );
    let mut wrong = near;
    wrong[8] = 1;
    let mut inconsistent = PIXELS;
    inconsistent[4..8].fill(0);
    check.check(
        !magenta::is_transparent_copy_pixels(&PIXELS, &wrong) && !magenta::is_transparent_copy_pixels(&PIXELS, &inconsistent) && !magenta::is_transparent_copy_pixels(&PIXELS, &PIXELS),
        "Legacy pixel matching rejects changed artwork, inconsistent thresholds and unchanged images",
    );
}
