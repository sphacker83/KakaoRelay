//! 원본 검사 러너 이식(`tests/KakaoRelay.Tests/Program.cs`, `KakaoRelay.Portable.Tests`, `KakaoRelay.ModelCatalog.Tests`).
//! 이 실행 파일은 가짜 CLI를 겸한다(원본과 같다). 인자로 묶음 이름을 주면 그 묶음만 실행한다.

mod ai;
mod attachment;
mod auto_knowledge;
mod chat_tool;
mod fakes;
mod image_cli;
mod instructions;
mod knowledge;
mod knowledge_service;
mod model_catalog;
mod persona;
mod png;
mod reply_mode;
mod resident;
mod structured;
mod support;

use support::Check;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = kakaorelay_testkit::fake_cli::run(&args) {
        std::process::exit(code);
    }
    let root = kakaorelay_testkit::TempDir::new("checks");
    // 실제 사용자 CLI 홈(로그인 파일)과 앱 데이터를 건드리지 않도록 스레드를 띄우기 전에 고정한다.
    for (key, dir) in [
        ("CODEX_HOME", "legacy-codex"),
        ("GROK_HOME", "legacy-grok"),
        ("CLAUDE_CONFIG_DIR", "legacy-claude"),
    ] {
        // SAFETY: 아직 다른 스레드가 없다.
        unsafe { std::env::set_var(key, root.path().join(dir)) };
    }
    kakaorelay_ai::paths::set_roots(root.path().join("app"), root.path().join("data"));
    let filter: Vec<&str> = args
        .iter()
        .filter(|a| !a.starts_with('-'))
        .map(String::as_str)
        .collect();
    let selected = |name: &str| filter.is_empty() || filter.contains(&name);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut summary: Vec<(&str, usize)> = vec![];
    runtime.block_on(async {
        macro_rules! group {
            ($name:literal, |$root:ident, $check:ident| $body:expr) => {
                if selected($name) {
                    let $check = Check::new();
                    let dir = root.path().join($name);
                    std::fs::create_dir_all(&dir).unwrap();
                    let $root = dir.as_path();
                    $body.await;
                    summary.push(($name, $check.count()));
                }
            };
        }
        group!("ai", |r, c| ai::run(r, &c));
        group!("resident", |r, c| resident::run(r, &c));
        group!("instructions", |r, c| instructions::run(r, &c));
        group!("persona", |r, c| persona::run(r, &c));
        group!("image-cli", |r, c| image_cli::run(r, &c));
        group!("attachment", |r, c| attachment::run(r, &c));
        group!("reply-mode", |r, c| reply_mode::run(r, &c));
        group!("chat-tool", |r, c| chat_tool::run(r, &c));
        group!("png", |r, c| png::run(r, &c));
        group!("model-catalog", |_r, c| model_catalog::run(&c));
        group!("knowledge", |r, c| knowledge::run(r, &c));
        group!("knowledge-service", |r, c| knowledge_service::run(r, &c));
        group!("structured", |r, c| structured::run(r, &c));
        group!("auto-knowledge", |r, c| auto_knowledge::run(r, &c));
    });
    let total: usize = summary.iter().map(|(_, n)| n).sum();
    for (name, count) in &summary {
        println!("{name}: {count}");
    }
    println!("All {total} checks passed.");
}
