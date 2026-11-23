//! 가짜 Codex/Claude/Grok CLI. 원본 테스트 실행 파일이 겸하던 동작
//! (`tests/KakaoRelay.Tests/Program.cs:5-18`, `ResidentCliChecks.Fixture`, `ModelCatalog.Tests/Program.cs:6-25`)을 합쳤다.
//!
//! - `app-server`/`--input-format`: 상주 프로토콜(초기화, 설정 조회, 스레드, 턴, 모델 목록). 프롬프트
//!   `HANG`은 멈추고, `CRASH`는 종료하고, `BADJSON`은 잘못된 줄을 쓰고, `FAIL`은 실패 결과를 돌려준다.
//! - `--output-last-message <파일>`: 표준 입력 전체를 그 파일에 쓴다(`HANG`이면 멈춘다).
//! - `models`: Grok 모델 목록 출력(로그인 안 됨 표시 포함).

use std::io::{BufRead, Read, Write};

use serde_json::{Value, json};

fn hang() -> ! {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

fn emit(value: Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

/// 가짜 CLI로 호출된 경우 종료 코드를 돌려준다. 해당하지 않으면 `None`.
pub fn run(args: &[String]) -> Option<i32> {
    let has = |name: &str| args.iter().any(|a| a == name);
    if has("app-server") || has("--input-format") {
        return Some(resident(args));
    }
    if let Some(index) = args.iter().position(|a| a == "--output-last-message") {
        let mut input = String::new();
        let _ = std::io::stdin().read_to_string(&mut input);
        if input == "HANG" {
            hang();
        }
        std::fs::write(&args[index + 1], input).expect("응답 파일 쓰기");
        return Some(0);
    }
    if has("models") {
        println!(
            "You are not authenticated.\nAvailable models:\n * fixture-grok (default)\n - fixture-other"
        );
        return Some(0);
    }
    None
}

fn resident(args: &[String]) -> i32 {
    let claude = args.iter().any(|a| a == "--input-format");
    let mut sid = if claude {
        let flag = if args.iter().any(|a| a == "--resume") {
            "--resume"
        } else {
            "--session-id"
        };
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_default()
    } else {
        String::new()
    };
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let item: Value = serde_json::from_str(&line).expect("JSON 줄");
        let mut id = Value::Null;
        let prompt;
        if !claude {
            let (Some(method), Some(request_id)) =
                (item.get("method").and_then(Value::as_str), item.get("id"))
            else {
                continue;
            };
            id = request_id.clone();
            let p = item.get("params").cloned().unwrap_or(Value::Null);
            match method {
                "initialize" => {
                    emit(json!({"id": id, "result": {}}));
                    continue;
                }
                "config/read" => {
                    emit(
                        json!({"id": id, "result": {"config": {"mcp_servers": {"test": {"command": "fake"}}, "plugins": {"plugin@fixture": {"enabled": true}}}}}),
                    );
                    continue;
                }
                "model/list" => {
                    let second = p.get("cursor").is_some_and(Value::is_string);
                    emit(
                        json!({"id": id, "result": {"data": [{"model": if second { "second-model" } else { "installed-model" }, "displayName": "Fixture model"}], "nextCursor": if second { Value::Null } else { json!("next") }}}),
                    );
                    continue;
                }
                "thread/start" | "thread/resume" => {
                    let config = &p["config"];
                    assert_eq!(config["web_search"], "live", "Live web search not enabled");
                    assert!(
                        config["mcp_servers.test.enabled"] == false
                            && config["plugins.plugin@fixture.enabled"] == false,
                        "Integrations not disabled"
                    );
                    let file_tools = config["features.shell_tool"] == true;
                    assert_eq!(
                        p["sandbox"],
                        if file_tools {
                            "workspace-write"
                        } else {
                            "read-only"
                        },
                        "Sandbox does not match file tools"
                    );
                    if file_tools && cfg!(windows) {
                        assert_eq!(
                            config["windows.sandbox"], "unelevated",
                            "Windows file sandbox missing"
                        );
                    }
                    sid = if method == "thread/start" {
                        uuid::Uuid::new_v4().to_string()
                    } else {
                        p["threadId"].as_str().unwrap_or_default().to_string()
                    };
                    emit(json!({"id": id, "result": {"thread": {"id": sid}}}));
                    continue;
                }
                "turn/start" => {
                    prompt = p["input"][0]["text"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                }
                _ => {
                    emit(json!({"id": id, "error": {"code": -32601, "message": "unsupported"}}));
                    continue;
                }
            }
        } else {
            match item.get("type").and_then(Value::as_str) {
                Some("user") => {}
                Some("control_request") => {
                    emit(
                        json!({"type": "control_response", "response": {"subtype": "success", "request_id": "models", "response": {"models": [{"value": "alias[1m]", "displayName": "Fixture alias"}]}}}),
                    );
                    continue;
                }
                _ => continue,
            }
            prompt = item["message"]["content"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
        match prompt.as_str() {
            "HANG" => hang(),
            "CRASH" => return 0,
            "BADJSON" => {
                println!("invalid JSON");
                let _ = std::io::stdout().flush();
                continue;
            }
            _ => {}
        }
        let state = std::env::current_dir()
            .unwrap_or_default()
            .join(format!("fixture-{sid}.txt"));
        let count = std::fs::read_to_string(&state)
            .ok()
            .and_then(|t| t.trim().parse::<i64>().ok())
            .unwrap_or(0)
            + 1;
        std::fs::write(&state, count.to_string()).expect("상태 파일 쓰기");
        let answer =
            json!({"pid": std::process::id(), "count": count, "prompt": prompt}).to_string();
        if claude {
            emit(
                json!({"type": "result", "subtype": "success", "is_error": prompt == "FAIL", "session_id": sid, "result": answer}),
            );
        } else {
            emit(json!({"id": id, "result": {"turn": {"id": format!("turn-{count}")}}}));
            emit(
                json!({"method": "item/completed", "params": {"threadId": sid, "item": {"type": "agentMessage", "phase": "commentary", "text": "do not send this progress"}}}),
            );
            emit(
                json!({"method": "item/completed", "params": {"threadId": sid, "item": {"type": "agentMessage", "phase": "final_answer", "text": answer}}}),
            );
            emit(
                json!({"method": "turn/completed", "params": {"threadId": sid, "turn": {"status": if prompt == "FAIL" { "failed" } else { "completed" }}}}),
            );
        }
    }
    0
}
