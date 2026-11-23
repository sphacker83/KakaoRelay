//! Windows 실기 검증 도구. 절차는 `VERIFY.md`를 따른다. 앱(Phase 5) 연결 전 어댑터를 직접 실행한다.
//! 대화 본문은 출력하지 않는다. 발송 명령은 `--confirm`이 있어야 실제로 Enter를 게시한다.

#[cfg(not(windows))]
fn main() {
    eprintln!("이 도구는 Windows 전용입니다.");
    std::process::exit(64);
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    if let Err(message) = win::run(&args) {
        eprintln!("실패: {message}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod win {
    use kakaorelay_core::platform::{Platform, SendTarget, TransportError};
    use kakaorelay_windows::WindowsPlatform;
    use kakaorelay_windows::diagnostics;
    use kakaorelay_windows::instance::{InstanceStart, acquire_main_instance};
    use std::time::Duration;

    const USAGE: &str = "사용법: verify <rooms|local-rooms|read <profile> <roomId> [limit]|send-text <제목> <본문> [--confirm]|send-image <제목> <파일> [--confirm]|diagnose|instance>";

    struct Target {
        recipient: String,
        process_id: i32,
        handle: String,
        message: String,
    }

    impl SendTarget for Target {
        fn request_id(&self) -> &str {
            "verify"
        }
        fn recipient(&self) -> &str {
            &self.recipient
        }
        fn expected_process_id(&self) -> i32 {
            self.process_id
        }
        fn expected_window_handle(&self) -> &str {
            &self.handle
        }
        fn message(&self) -> &str {
            &self.message
        }
    }

    fn text(error: TransportError) -> String {
        error.to_string()
    }

    /// 열린 방 목록에서 제목이 정확히 같은 선택 가능한 방 하나를 고른다.
    fn target(platform: &WindowsPlatform, title: &str, message: &str) -> Result<Target, String> {
        let rooms = platform.rooms().scan().map_err(text)?;
        let matches: Vec<_> = rooms.iter().filter(|r| r.title == title).collect();
        let [room] = matches.as_slice() else {
            return Err(format!(
                "열린 방 '{title}'을 하나로 찾지 못했습니다({}개).",
                matches.len()
            ));
        };
        if !room.selectable() {
            return Err(room.hint().into());
        }
        Ok(Target {
            recipient: room.title.clone(),
            process_id: room.process_id,
            handle: room.handle.clone(),
            message: message.into(),
        })
    }

    pub fn run(args: &[&str]) -> Result<(), String> {
        let platform = WindowsPlatform::new();
        match args {
            ["rooms"] => {
                for room in platform.rooms().scan().map_err(text)? {
                    println!(
                        "{}\tpid={}\thandle={}\t{}",
                        room.title,
                        room.process_id,
                        room.handle,
                        room.hint()
                    );
                }
            }
            ["local-rooms"] => {
                let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
                let rooms = runtime
                    .block_on(platform.reader().rooms())
                    .map_err(|e| format!("{} {}", e.status_code, e.code))?;
                for room in rooms {
                    println!(
                        "{}\t{}\t{}\treadable={}\tdirect={}",
                        room.profile,
                        room.id,
                        room.title,
                        room.readable,
                        room.direct_member_id.is_some()
                    );
                }
            }
            ["read", profile, room_id, rest @ ..] => {
                let limit = rest
                    .first()
                    .map_or(Ok(20), |v| v.parse())
                    .map_err(|_| USAGE.to_string())?;
                let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
                let reader = platform.reader();
                runtime
                    .block_on(reader.rooms())
                    .map_err(|e| format!("{} {}", e.status_code, e.code))?;
                let context = runtime
                    .block_on(reader.read(profile, room_id, limit))
                    .map_err(|e| format!("{} {} {}", e.status_code, e.code, e.detail))?;
                // 본문은 출력하지 않고 길이만 보인다.
                println!(
                    "messages={} readable={}",
                    context.messages.len(),
                    context.room.readable
                );
                for m in &context.messages {
                    println!(
                        "{}\t{}\ttype={}\tdeleted={}\tlength={}",
                        m.id,
                        m.time.to_rfc3339(),
                        m.kind,
                        m.deleted,
                        m.text.chars().count()
                    );
                }
            }
            ["send-text", title, message, rest @ ..] => {
                let target = target(&platform, title, message)?;
                let mut transport = platform.text_transport();
                transport.validate_target(&target).map_err(text)?;
                let draft = transport.read_draft().map_err(text)?;
                println!(
                    "검증 통과. 초안 길이={}",
                    draft.as_deref().map_or(0, |d| d.chars().count())
                );
                if rest != ["--confirm"] {
                    println!("--confirm이 없어 입력하지 않았습니다.");
                    return Ok(());
                }
                if draft.as_deref() != Some("") {
                    return Err("입력창이 비어 있지 않아 중단했습니다.".into());
                }
                transport.write_draft(message).map_err(text)?;
                println!(
                    "되읽기 일치={}",
                    transport.read_draft().map_err(text)?.as_deref() == Some(*message)
                );
                transport.validate_target(&target).map_err(text)?;
                transport.post_enter().map_err(text)?;
                println!(
                    "Enter 게시됨. 입력창 비움 확인={}",
                    transport.confirm_after_enter(&target).map_err(text)?
                );
            }
            ["send-image", title, path, rest @ ..] => {
                let target = target(&platform, title, "image:verify")?;
                let mut transport = platform.image_transport();
                transport.validate(&target).map_err(text)?;
                println!("검증 통과(빈 초안 확인).");
                if rest != ["--confirm"] {
                    println!("--confirm이 없어 첨부하지 않았습니다.");
                    return Ok(());
                }
                transport.attach(std::path::Path::new(path)).map_err(text)?;
                println!(
                    "첨부 요청됨. 미리보기 전송·닫힘 확인={}",
                    transport.confirm_preview().map_err(text)?
                );
            }
            ["diagnose"] => {
                let mut checkpoints = 0;
                let report = diagnostics::collect(&mut |_| checkpoints += 1);
                println!(
                    "status={} stage={} checkpoints={checkpoints}",
                    report.status, report.stage
                );
                println!(
                    "processes={} windows={} warnings={:?}",
                    report.processes.len(),
                    report.windows.len(),
                    report.warnings
                );
                for w in &report.windows {
                    let candidates = w.controls.iter().filter(|c| c.is_input_candidate()).count();
                    println!(
                        "{}\t{}\t{}\tchildren={}\tcontrols={}\tinputCandidates={}",
                        w.handle,
                        w.class_name,
                        w.automation_status,
                        w.native_children.len(),
                        w.controls.len(),
                        candidates
                    );
                }
                println!(
                    "foreground before={} after={}",
                    report.foreground_window_before, report.foreground_window_after
                );
            }
            ["instance"] => match acquire_main_instance().map_err(|e| e.message())? {
                InstanceStart::Primary(instance) => {
                    println!("첫 실행입니다. 60초 동안 표시 요청을 기다립니다.");
                    println!(
                        "표시 요청 수신={}",
                        instance.wait_for_show(Duration::from_secs(60))
                    );
                }
                InstanceStart::AlreadyRunning => {
                    println!("이미 실행 중입니다. 표시 요청을 보냈습니다.")
                }
            },
            _ => return Err(USAGE.into()),
        }
        Ok(())
    }
}
