//! 원본: `MagentaTransparency.cs`와 앱의 `MagentaPng.cs`·`PersonaImagesPanel.NormalizePng`(WPF 이미징)을 `image` crate로 옮겼다.
//! 화소 판정은 R·B에 대해 대칭이라 BGRA 대신 RGBA 버퍼에 그대로 적용한다(직선 알파).

use std::io::Cursor;

use image::{DynamicImage, ImageFormat, ImageReader};

use crate::cancel::CancellationToken;
use crate::error::{Error, Result};

const MAX_PIXELS: u64 = 32_000_000;

/// `source`의 마젠타 근접 화소만 완전 투명(0,0,0,0)으로 바꾼 사본인지 판정한다. 허용 오차 하나로 설명돼야 한다.
pub fn is_transparent_copy_pixels(source: &[u8], candidate: &[u8]) -> bool {
    if source.len() != candidate.len() || !source.len().is_multiple_of(4) {
        return false;
    }
    let (mut minimum, mut maximum, mut changed) = (0i32, 100i32, false);
    for (original, result) in source
        .as_chunks::<4>()
        .0
        .iter()
        .zip(candidate.as_chunks::<4>().0)
    {
        if original[3] == 0 {
            if original != result {
                return false;
            }
            continue;
        }
        let distance = (255 - original[0] as i32)
            .max(original[1] as i32)
            .max(255 - original[2] as i32);
        if original == result {
            maximum = maximum.min(distance - 1);
        } else if *result == [0, 0, 0, 0] {
            minimum = minimum.max(distance);
            changed = true;
        } else {
            return false;
        }
        if minimum > maximum {
            return false;
        }
    }
    changed
}

/// 마젠타 근접 화소를 투명하게 바꾸고 바뀐 화소 수를 돌려준다. 다른 색과 알파는 그대로다.
pub fn apply(pixels: &mut [u8], tolerance: i32, cancel: &CancellationToken) -> Result<usize> {
    if !(0..=100).contains(&tolerance) {
        return Err(Error::argument("tolerance"));
    }
    if !pixels.len().is_multiple_of(4) {
        return Err(Error::argument("BGRA 픽셀 데이터가 올바르지 않습니다."));
    }
    let tolerance = tolerance as u8;
    let mut changed = 0;
    for (index, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        if (index * 4) & 65535 == 0 && cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if pixel[3] != 0
            && 255 - pixel[0] <= tolerance
            && pixel[1] <= tolerance
            && 255 - pixel[2] <= tolerance
        {
            pixel.fill(0);
            changed += 1;
        }
    }
    Ok(changed)
}

fn decode(bytes: &[u8]) -> Result<DynamicImage> {
    let bad =
        |e: &dyn std::fmt::Display| Error::invalid_data(format!("이미지를 읽을 수 없습니다: {e}"));
    let reader = || {
        ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| bad(&e))
    };
    let (width, height) = reader()?.into_dimensions().map_err(|e| bad(&e))?;
    if width as u64 * height as u64 > MAX_PIXELS {
        return Err(Error::invalid_data("이미지 해상도가 너무 큽니다."));
    }
    reader()?.decode().map_err(|e| bad(&e))
}

fn encode(image: &DynamicImage) -> Result<Vec<u8>> {
    let mut output = Cursor::new(vec![]);
    image
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|e| Error::invalid_data(e.to_string()))?;
    Ok(output.into_inner())
}

/// CLI가 만든 PNG·JPEG·WebP를 첫 프레임 PNG로 다시 인코딩한다(3,200만 화소 제한).
pub fn normalize_png(bytes: &[u8]) -> Result<Vec<u8>> {
    encode(&decode(bytes)?)
}

/// 두 이미지가 같은 크기이고 `candidate`가 `source`의 마젠타 투명화 사본인지 판정한다.
pub fn is_transparent_copy(source: &[u8], candidate: &[u8]) -> Result<bool> {
    let (original, result) = (decode(source)?.to_rgba8(), decode(candidate)?.to_rgba8());
    Ok(original.dimensions() == result.dimensions()
        && is_transparent_copy_pixels(original.as_raw(), result.as_raw()))
}

/// 마젠타 투명화 PNG와 바뀐 화소 수. 바뀐 화소가 없으면 빈 PNG와 0을 돌려준다.
pub fn convert(
    input: &[u8],
    tolerance: i32,
    cancel: &CancellationToken,
) -> Result<(Vec<u8>, usize)> {
    let mut pixels = decode(input)?.to_rgba8();
    let changed = apply(&mut pixels, tolerance, cancel)?;
    if changed == 0 {
        return Ok((vec![], 0));
    }
    let png = encode(&DynamicImage::ImageRgba8(pixels))?;
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok((png, changed))
}
