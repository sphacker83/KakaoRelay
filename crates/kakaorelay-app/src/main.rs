//! KakaoRelay 데스크톱 앱 진입점. 원본: `src/KakaoRelay.App/App.xaml.cs`.
//! 명령줄 모드(`--send-image`, `--worker`, `--diagnose`)는 창·트레이·API·주 인스턴스 잠금 없이 끝난다.
// Windows 배포 빌드에서 콘솔 창을 띄우지 않는다.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod cli;
mod commands;
mod instance;
mod os;
mod state;

/// 원본 `App.OnStartup`의 인자 판정. 그 밖의 인자는 종료 코드 64다.
enum Mode {
    SendImage(String, String),
    Worker(String),
    Diagnose(String),
    Gui {
        resume_replies: bool,
        report: Option<String>,
    },
    Usage,
}

fn parse(args: &[String]) -> Mode {
    match args {
        [flag, request, output] if flag == "--send-image" => {
            Mode::SendImage(request.clone(), output.clone())
        }
        [flag, path] if flag == "--worker" => Mode::Worker(path.clone()),
        [flag, path] if flag == "--diagnose" => Mode::Diagnose(path.clone()),
        [] => Mode::Gui {
            resume_replies: false,
            report: None,
        },
        [flag] if flag == "--resume-replies" => Mode::Gui {
            resume_replies: true,
            report: None,
        },
        [flag, path] if flag == "--report" => Mode::Gui {
            resume_replies: false,
            report: Some(path.clone()),
        },
        _ => Mode::Usage,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match parse(&args) {
        Mode::SendImage(request, output) => cli::send_image(&request, &output),
        Mode::Worker(path) => cli::worker(&path),
        Mode::Diagnose(path) => cli::diagnose(&path),
        Mode::Usage => 64,
        Mode::Gui {
            resume_replies,
            report,
        } => app::run(resume_replies, report),
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn unknown_arguments_are_usage_errors() {
        for bad in [
            &["--worker"][..],
            &["--worker", "a", "b"],
            &["--diagnose"],
            &["--send-image", "a"],
            &["--report"],
            &["--resume-replies", "x"],
            &["--other"],
        ] {
            assert!(matches!(parse(&args(bad)), Mode::Usage), "{bad:?}");
        }
    }

    #[test]
    fn gui_modes_are_recognized() {
        assert!(matches!(
            parse(&args(&[])),
            Mode::Gui {
                resume_replies: false,
                report: None
            }
        ));
        assert!(matches!(
            parse(&args(&["--resume-replies"])),
            Mode::Gui {
                resume_replies: true,
                ..
            }
        ));
        assert!(matches!(
            parse(&args(&["--report", "r.json"])),
            Mode::Gui {
                report: Some(_),
                ..
            }
        ));
        assert!(matches!(
            parse(&args(&["--send-image", "a", "b"])),
            Mode::SendImage(..)
        ));
    }
}
