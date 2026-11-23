//! .NET 문자열 의미 재현. C# `string.Length`·인덱싱은 UTF-16 코드 단위라 길이 제한과 잘라내기를 같은 기준으로 맞춘다.

use unicode_normalization::UnicodeNormalization;

pub fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// `text[..n]`(UTF-16 기준). 서로게이트 쌍 중간에서 끊기면 그 문자를 뺀다(Rust 문자열은 짝 없는 서로게이트를 담지 못한다).
pub fn utf16_prefix(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (index, c) in text.char_indices() {
        if used + c.len_utf16() > units {
            return &text[..index];
        }
        used += c.len_utf16();
    }
    text
}

/// `ToLowerInvariant`. .NET은 문자 단위 1:1 대응만 하므로 여러 글자로 바뀌는 경우 첫 글자만 쓴다.
pub fn lower_invariant(text: &str) -> String {
    text.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

/// `string.Normalize()`(NFC).
pub fn nfc(text: &str) -> String {
    text.nfc().collect()
}

/// `string.IsNullOrWhiteSpace`.
pub fn is_blank(text: &str) -> bool {
    text.chars().all(char::is_whitespace)
}

/// `Regex.Replace(value, @"\s+", " ").Trim()` 계열 정규화에 쓰는 공백 압축.
pub fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// 대화방 제목 등 한 줄 표시용(`Replace('\r',' ').Replace('\n',' ')`).
pub fn single_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

/// .NET `Environment.NewLine`(`StringBuilder.AppendLine`).
pub const NEW_LINE: &str = if cfg!(windows) { "\r\n" } else { "\n" };

/// `StringComparer.Ordinal`(UTF-16 코드 단위 비교).
pub fn ordinal_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `StringComparer.OrdinalIgnoreCase`.
pub fn ordinal_ignore_case_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let upper = |s: &str| {
        s.chars()
            .map(|c| c.to_uppercase().next().unwrap_or(c))
            .collect::<String>()
    };
    ordinal_cmp(&upper(a), &upper(b))
}

/// `File.ReadAllText`/`StreamReader(detectEncodingFromByteOrderMarks)`: BOM으로 UTF-8·UTF-16을 가리고, 잘못된 바이트는 대체 문자로 바꾼다.
pub fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    let utf16 = |rest: &[u8], le: bool| {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                if le {
                    u16::from_le_bytes(*c)
                } else {
                    u16::from_be_bytes(*c)
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, false);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

pub fn read_text(path: &std::path::Path) -> std::io::Result<String> {
    Ok(decode(&std::fs::read(path)?))
}
