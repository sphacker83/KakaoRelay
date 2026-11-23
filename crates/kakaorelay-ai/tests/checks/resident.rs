//! 원본: `ResidentCliChecks.cs`.

use std::path::Path;

use kakaorelay_ai::Error;
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::cli_runner::CliAiRunner;
use kakaorelay_ai::settings::AiProviderSettings;

use crate::support::{Check, fake_exe, field, running};

fn cancel_after(ms: u64) -> CancellationToken {
    let token = CancellationToken::new();
    let clone = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        clone.cancel();
    });
    token
}

pub async fn run(root: &Path, check: &Check) {
    let none = CancellationToken::new();
    for id in ["codex", "claude"] {
        let provider = AiProviderSettings {
            executable: fake_exe(),
            model: "fixture".into(),
            effort: "high".into(),
            ..AiProviderSettings::new(id)
        };
        let folder = root.join("resident").join(id);
        let other_folder = root.join("resident").join(format!("{id}-other"));
        let runner = CliAiRunner::new();
        let prompt = "한글 \"따옴표\"\n줄바꿈 $(literal)";
        let first = runner
            .run_conversation(&provider, prompt, 10, &none, &folder, None, &[])
            .await
            .unwrap();
        let second = runner
            .run_conversation(&provider, "next", 10, &none, &folder, Some(&first.1), &[])
            .await
            .unwrap();
        let pid = field(&first.0, "pid");
        check.check(
            first.1 == second.1 && pid == field(&second.0, "pid") && field(&second.0, "count") == 2,
            &format!("{id} reuses the same live process and session for the next turn"),
        );
        let echoed: serde_json::Value = serde_json::from_str(&first.0).unwrap();
        check.check(
            echoed["prompt"] == prompt,
            &format!("{id} resident stdin preserves Unicode, quoting and multiline text"),
        );
        let other = runner
            .run_conversation(
                &provider,
                "another room",
                10,
                &none,
                &other_folder,
                None,
                &[],
            )
            .await
            .unwrap();
        check.check(
            field(&other.0, "pid") != pid && other.1 != first.1 && field(&other.0, "count") == 1,
            &format!("{id} rooms have independent resident processes and conversations"),
        );
        let cancel = cancel_after(250);
        let (hung, independent) = tokio::join!(
            runner.run_conversation(&provider, "HANG", 10, &cancel, &folder, Some(&first.1), &[]),
            runner.run_conversation(
                &provider,
                "still works",
                10,
                &none,
                &other_folder,
                Some(&other.1),
                &[]
            )
        );
        check.check(
            hung == Err(Error::Cancelled)
                && !running(pid)
                && field(&independent.unwrap().0, "count") == 2,
            &format!(
                "{id} cancelling a turn kills only its own process while another room continues"
            ),
        );
        let restored = runner
            .run_conversation(&provider, "resume", 10, &none, &folder, Some(&first.1), &[])
            .await
            .unwrap();
        check.check(
            restored.1 == first.1
                && field(&restored.0, "count") == 3
                && field(&restored.0, "pid") != pid,
            &format!("{id} reconnects using the explicit saved session after process termination"),
        );
        let timeout = runner
            .run_conversation(&provider, "HANG", 1, &none, &folder, Some(&first.1), &[])
            .await;
        check.check(
            timeout == Err(Error::Cancelled) && !running(field(&restored.0, "pid")),
            &format!("{id} timeout terminates its resident process"),
        );
        for fault in ["FAIL", "CRASH", "BADJSON"] {
            let result = runner
                .run_conversation(&provider, fault, 10, &none, &folder, Some(&first.1), &[])
                .await;
            check.check(matches!(result, Err(ref e) if e.is_io() || matches!(e, Error::InvalidOperation(_))), &format!("{id} {fault} invalidates the connection without replaying the turn"));
        }
        let last = runner
            .run_conversation(&provider, "final", 10, &none, &folder, Some(&first.1), &[])
            .await
            .unwrap();
        let last_pid = field(&last.0, "pid");
        let shutdown = async {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            runner.dispose();
            runner.dispose();
        };
        let (active, ()) = tokio::join!(
            runner.run_conversation(&provider, "HANG", 10, &none, &folder, Some(&first.1), &[]),
            shutdown
        );
        check.check(
            active == Err(Error::Cancelled),
            &format!("{id} shutdown cancels an in-flight turn"),
        );
        check.check(
            !running(last_pid) && !running(field(&other.0, "pid")),
            &format!("{id} disposal stops all owned resident processes"),
        );
    }
    let args = CliAiRunner::resident_arguments(
        &AiProviderSettings {
            effort: "high".into(),
            ..AiProviderSettings::new("claude")
        },
        Some("saved-id"),
        "unused",
        false,
    )
    .unwrap();
    check.check(
        args.contains(&"--input-format".into())
            && args.iter().filter(|x| *x == "stream-json").count() == 2
            && args.contains(&"--resume".into())
            && args.contains(&"saved-id".into())
            && args.contains(&"high".into())
            && !args.contains(&"--no-session-persistence".into()),
        "Claude resident arguments preserve effort and persisted explicit resume",
    );
}
