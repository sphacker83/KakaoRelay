//! `image` crate로 옮긴 PNG 재인코딩·마젠타 투명화(원본은 앱의 WPF `MagentaPng`·`NormalizePng`)와
//! 프로세스 트리 종료를 고정한다. 원본 검사 묶음에는 없던 Rust 추가 검사다.

use std::io::Cursor;
use std::path::Path;

use image::{ImageFormat, Rgba, RgbaImage};
use kakaorelay_ai::cancel::CancellationToken;
use kakaorelay_ai::{Error, magenta};

use crate::support::Check;

fn encode(image: &RgbaImage, format: ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(vec![]);
    image::DynamicImage::ImageRgba8(image.clone())
        .write_to(&mut out, format)
        .unwrap();
    out.into_inner()
}

fn decode(bytes: &[u8]) -> RgbaImage {
    image::load_from_memory(bytes).unwrap().to_rgba8()
}

pub async fn run(_root: &Path, check: &Check) {
    let none = CancellationToken::new();
    // 마젠타, 근접 마젠타(부분 알파), 빨강, 흰색, 이미 투명한 마젠타.
    let source = RgbaImage::from_fn(5, 1, |x, _| match x {
        0 => Rgba([255, 0, 255, 255]),
        1 => Rgba([245, 15, 240, 128]),
        2 => Rgba([255, 0, 0, 255]),
        3 => Rgba([255, 255, 255, 255]),
        _ => Rgba([255, 0, 255, 0]),
    });
    let png = encode(&source, ImageFormat::Png);
    let (converted, changed) = magenta::convert(&png, 30, &none).unwrap();
    let result = decode(&converted);
    check.check(
        changed == 2
            && converted.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10])
            && result.get_pixel(0, 0).0 == [0, 0, 0, 0]
            && result.get_pixel(1, 0).0 == [0, 0, 0, 0]
            && result.get_pixel(2, 0).0 == [255, 0, 0, 255]
            && result.get_pixel(3, 0).0 == [255, 255, 255, 255]
            && result.get_pixel(4, 0).0 == [255, 0, 255, 0],
        "Magenta PNG conversion clears only near-magenta pixels and re-encodes as PNG",
    );
    check.check(
        magenta::is_transparent_copy(&png, &converted).unwrap(),
        "Decoded transparent copy is recognized from PNG bytes",
    );
    check.check(
        magenta::convert(&png, 0, &none).unwrap().1 == 1,
        "Exact tolerance converts only pure magenta",
    );
    let plain = encode(
        &RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, 255])),
        ImageFormat::Png,
    );
    check.check(
        magenta::convert(&plain, 100, &none).unwrap() == (vec![], 0),
        "Images without magenta report no change and no output",
    );
    let jpeg = encode(
        &RgbaImage::from_pixel(4, 4, Rgba([200, 100, 50, 255])),
        ImageFormat::Jpeg,
    );
    let normalized = magenta::normalize_png(&jpeg).unwrap();
    check.check(
        normalized.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10])
            && decode(&normalized).dimensions() == (4, 4),
        "CLI JPEG output is normalized to a PNG with the same dimensions",
    );
    let lossless = magenta::normalize_png(&png).unwrap();
    check.check(
        decode(&lossless) == source,
        "PNG normalization preserves straight-alpha pixels exactly",
    );
    check.check(
        matches!(
            magenta::normalize_png(b"not an image"),
            Err(Error::InvalidData(_))
        ),
        "Undecodable CLI output is rejected as invalid data",
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    check.check(
        magenta::convert(&png, 30, &cancelled) == Err(Error::Cancelled),
        "Magenta conversion honors cancellation",
    );
    process_tree(check).await;
}

#[cfg(unix)]
async fn process_tree(check: &Check) {
    use tokio::io::AsyncBufReadExt;
    let mut command = kakaorelay_ai::process::CliCommand::new("/bin/sh", std::env::temp_dir());
    command.args = vec!["-c".into(), "sleep 300 & echo $!; wait".into()];
    let mut child = command.spawn().unwrap();
    let mut lines = tokio::io::BufReader::new(child.take_stdout().unwrap()).lines();
    let grandchild: i64 = lines
        .next_line()
        .await
        .unwrap()
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    check.check(
        crate::support::running_now(grandchild),
        "Grandchild process starts under the CLI process",
    );
    child.kill_tree();
    check.check(
        !crate::support::running(grandchild),
        "Killing a CLI terminates its whole process group, including grandchildren",
    );
}

#[cfg(not(unix))]
async fn process_tree(_check: &Check) {}
