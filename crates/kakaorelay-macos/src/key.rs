//! 맥 카카오톡 DB 키 재료: 기기 UUID, 사용자 ID, SQLCipher passphrase 유도.
//! 유도식 출처와 실증: `dev/active/rust-port/rust-port-macos-research.md` "DB 읽기".

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use base64::Engine;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

/// 카카오톡 26.10.0의 `secureKey`. 결과는 SQLCipher `PRAGMA key`에 문자열로 넣는 256자 hex다.
pub fn passphrase(uuid: &str, user_id: u64) -> String {
    let mut digest = Sha1::digest(uuid.as_bytes()).to_vec();
    digest.extend_from_slice(&Sha256::digest(uuid.as_bytes()));
    let hashed = base64::engine::general_purpose::STANDARD.encode(digest);
    let user = user_id.to_string();
    // UUID는 ASCII 36자라 바이트 인덱스로 잘라도 문자 경계가 깨지지 않는다.
    let parts = [
        "A",
        &hashed,
        "|",
        "F",
        &uuid[..5],
        "H",
        &user,
        "|",
        &uuid[7..],
    ];
    let joined: String = parts.join("F").chars().rev().collect();
    let salt = &uuid[uuid.len() * 3 / 10..];
    let mut key = [0u8; 128];
    pbkdf2::pbkdf2_hmac::<Sha256>(joined.as_bytes(), salt.as_bytes(), 100_000, &mut key);
    hex::encode(key)
}

/// plist 키 접미사 `:<sha512 hex>`로 저장된 `SHA512(십진 사용자 ID)` 목록과, 키·값에 나타난 숫자 후보.
#[derive(Debug, Default)]
pub struct PlistHints {
    pub hashes: HashSet<[u8; 64]>,
    pub candidates: Vec<u64>,
}

pub fn plist_hints(path: &Path) -> Result<PlistHints, plist::Error> {
    let value = plist::Value::from_file(path)?;
    let mut hints = PlistHints::default();
    let mut seen = HashSet::new();
    let mut add_digits = |text: &str, hints: &mut PlistHints| {
        for run in text.split(|c: char| !c.is_ascii_digit()) {
            // 카카오 사용자 ID는 십진 4~19자리 범위에서만 찾는다(u64 표현 범위).
            if (4..=19).contains(&run.len())
                && let Ok(n) = run.parse::<u64>()
                && seen.insert(n)
            {
                hints.candidates.push(n);
            }
        }
    };
    fn walk(value: &plist::Value, visit: &mut dyn FnMut(&str)) {
        match value {
            plist::Value::Dictionary(map) => {
                for (key, inner) in map {
                    visit(key);
                    walk(inner, visit);
                }
            }
            plist::Value::Array(items) => items.iter().for_each(|inner| walk(inner, visit)),
            plist::Value::String(text) => visit(text),
            plist::Value::Integer(n) => visit(&n.to_string()),
            _ => {}
        }
    }
    let mut texts = Vec::new();
    walk(&value, &mut |text| texts.push(text.to_owned()));
    for text in &texts {
        if let Some((_, suffix)) = text.rsplit_once(':')
            && suffix.len() == 128
            && let Ok(bytes) = hex::decode(suffix)
            && let Ok(hash) = <[u8; 64]>::try_from(bytes)
        {
            hints.hashes.insert(hash);
        }
        add_digits(text, &mut hints);
    }
    Ok(hints)
}

pub fn sha512_of(user_id: u64) -> [u8; 64] {
    Sha512::digest(user_id.to_string().as_bytes()).into()
}

/// 후보 중 해시 목록과 일치하는 첫 사용자 ID.
pub fn match_candidates(hints: &PlistHints) -> Option<u64> {
    hints
        .candidates
        .iter()
        .copied()
        .find(|n| hints.hashes.contains(&sha512_of(*n)))
}

/// 마지막 수단: 1..`limit` 범위의 `SHA512(십진)`을 해시 목록과 대조한다. 10코어에서 수 분이 걸린다.
pub fn brute_force(hashes: &HashSet<[u8; 64]>, limit: u64) -> Option<u64> {
    if hashes.is_empty() {
        return None;
    }
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()) as u64;
    const CHUNK: u64 = 1 << 20;
    let next = AtomicU64::new(1);
    let found = AtomicU64::new(0);
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                let mut buffer = itoa_buffer();
                while !stop.load(Ordering::Relaxed) {
                    let start = next.fetch_add(CHUNK, Ordering::Relaxed);
                    if start >= limit {
                        break;
                    }
                    for n in start..(start + CHUNK).min(limit) {
                        let text = format_decimal(n, &mut buffer);
                        let hash: [u8; 64] = Sha512::digest(text).into();
                        if hashes.contains(&hash) {
                            found.store(n, Ordering::Relaxed);
                            stop.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                }
            });
        }
    });
    match found.load(Ordering::Relaxed) {
        0 => None,
        n => Some(n),
    }
}

fn itoa_buffer() -> [u8; 20] {
    [0; 20]
}

/// 대조 루프에서 매번 `String`을 할당하지 않으려고 십진 문자열을 버퍼에 직접 쓴다.
fn format_decimal(mut n: u64, buffer: &mut [u8; 20]) -> &[u8] {
    let mut i = buffer.len();
    loop {
        i -= 1;
        buffer[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            return &buffer[i..];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 가짜 UUID·ID. 기대값은 연구 문서의 Python 재현식(hashlib.pbkdf2_hmac)으로 계산했다.
    const UUID: &str = "01234567-89AB-CDEF-0123-456789ABCDEF";

    #[test]
    fn passphrase_matches_known_vector() {
        assert_eq!(
            passphrase(UUID, 123_456_789),
            "e79395f193fbd7d4ec8a94d6c2943ff48245b74cb425efce63fb0f7723a624f6786c89174759faa04b2ccdd9db795bc79c8d9505ff8b3fccbcc12fc9a26412eb06971ef2ed04927c53753af9efd3837ff2a929325b156cccd6f4e325d9c335940f2977b2b8e8d6f2e4a78593058bb9fda813e31a569b0356457a637e912630e5"
        );
    }

    #[test]
    fn sha512_matches_known_vector() {
        assert_eq!(
            hex::encode(sha512_of(123_456_789)),
            "d9e6762dd1c8eaf6d61b3c6192fc408d4d6d5f1176d0c29169bc24e71c3f274ad27fcd5811b313d681f7e55ec02d73d499c95455b6b5bb503acf574fba8ffe85"
        );
    }

    #[test]
    fn plist_hints_find_hash_suffix_and_digit_candidates() {
        let dir = std::env::temp_dir().join(format!("kr-macos-plist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("prefs.plist");
        let mut map = plist::Dictionary::new();
        map.insert(
            format!(
                "DESIGNATEDFRIENDSREVISION:{}",
                hex::encode(sha512_of(987_654))
            ),
            1.into(),
        );
        map.insert(
            "FSChatWindowFrame_987654_1".into(),
            "{{0, 0}, {10, 10}}".into(),
        );
        map.insert("Other".into(), plist::Value::Array(vec![55_555.into()]));
        plist::Value::Dictionary(map).to_file_xml(&path).unwrap();
        let hints = plist_hints(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(hints.hashes.len(), 1);
        assert!(hints.candidates.contains(&55_555));
        assert_eq!(match_candidates(&hints), Some(987_654));
    }

    #[test]
    fn brute_force_finds_small_id_and_formats_decimal() {
        let mut buffer = itoa_buffer();
        assert_eq!(format_decimal(0, &mut buffer), b"0");
        assert_eq!(
            format_decimal(u64::MAX, &mut buffer),
            u64::MAX.to_string().as_bytes()
        );
        let hashes = HashSet::from([sha512_of(4_321)]);
        assert_eq!(brute_force(&hashes, 10_000), Some(4_321));
        assert_eq!(brute_force(&hashes, 4_000), None);
    }
}
