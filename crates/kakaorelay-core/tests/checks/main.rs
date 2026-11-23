//! 원본 검사 러너 이식(`tests/KakaoRelay.Tests/Program.cs`의 Core 묶음).
//! 이 실행 파일은 진단 worker(`--worker <경로>`)를 겸한다(원본과 같다). 인자로 묶음 이름을 주면 그 묶음만 실행한다.

mod api;
mod auto_reply;
mod chat_tool;
mod command_room;
mod delegation;
mod diagnostics;
mod fakes;
mod greetings;
mod host;
mod persona;
mod summary_forward;
mod support;
mod test_send;
mod unreadable;

use support::Check;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 2 && args[0] == "--worker" {
        std::process::exit(diagnostics::worker(&args[1]));
    }
    let root = kakaorelay_testkit::TempDir::new("core-checks");
    // 실제 사용자 앱 데이터(원장·설정)를 건드리지 않도록 스레드를 띄우기 전에 고정한다.
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
                    #[allow(clippy::unit_arg)]
                    $body;
                    summary.push(($name, $check.count()));
                }
            };
        }
        group!("test-send", |r, c| test_send::run(r, &c));
        group!("unreadable", |r, c| unreadable::run(r, &c));
        group!("api", |r, c| api::run(r, &c).await);
        group!("auto-reply", |r, c| auto_reply::run(r, &c).await);
        group!("chat-tool", |r, c| chat_tool::run(r, &c).await);
        group!("command-room", |r, c| command_room::run(r, &c).await);
        group!("diagnostics", |r, c| diagnostics::run(r, &c).await);
        group!("delegation", |r, c| delegation::run(r, &c).await);
        group!("host", |r, c| host::run(r, &c).await);
        group!("persona", |r, c| persona::run(r, &c));
        group!("summary-forward", |r, c| summary_forward::run(r, &c).await);
        group!("greetings", |r, c| greetings::run(r, &c));
    });
    let total: usize = summary.iter().map(|(_, n)| n).sum();
    for (name, count) in &summary {
        println!("{name}: {count}");
    }
    println!("All {total} checks passed.");
}
