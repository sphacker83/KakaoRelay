//! 원본 `tests/KakaoRelay.Tests/CipherChecks.cs`의 SQLCipher·WAL 검사 이식.
//! 같은 파일의 자동답장 후보 검사 3개(`AutoReplySession.Candidate(s)`)는 Phase 2(`kakaorelay-core`) 범위다.

use aes::cipher::{BlockEncryptMut, KeyIvInit, block_padding::NoPadding};
use hmac::{Hmac, Mac};
use kakaorelay_windows::cipher::{self, PAGE_SIZE};
use sha2::{Digest, Sha256, Sha512};

/// 고정 입력에서 만든 의사 난수 바이트. 테스트를 결정적으로 만든다.
fn bytes(label: &str, length: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut counter = 0u32;
    while out.len() < length {
        out.extend_from_slice(&Sha256::digest(format!("{label}:{counter}")));
        counter += 1;
    }
    out.truncate(length);
    out
}

struct Fixture {
    key: Vec<u8>,
    salt: Vec<u8>,
    mac_key: [u8; 32],
}

impl Fixture {
    fn new() -> Self {
        let key = bytes("key", 32);
        let salt = bytes("salt", 16);
        let mac_salt: Vec<u8> = salt.iter().map(|b| b ^ 0x3a).collect();
        let mut mac_key = [0u8; 32];
        pbkdf2::pbkdf2_hmac::<Sha512>(&key, &mac_salt, 2, &mut mac_key);
        Self { key, salt, mac_key }
    }

    /// 1번 페이지 형식의 SQLCipher 4 페이지. 평문 50번째 바이트(복호화 후 66번째)에 표식을 둔다.
    fn page(&self, marker: u8) -> Vec<u8> {
        let mut result = vec![0u8; PAGE_SIZE];
        result[..16].copy_from_slice(&self.salt);
        let mut plain = vec![0u8; 4000];
        plain[0] = 0x10;
        plain[2] = 2;
        plain[3] = 2;
        plain[4] = 80;
        plain[50] = marker;
        let iv = bytes(&format!("iv{marker}"), 16);
        result[4016..4032].copy_from_slice(&iv);
        cbc::Encryptor::<aes::Aes256>::new_from_slices(&self.key, &iv)
            .unwrap()
            .encrypt_padded_b2b_mut::<NoPadding>(&plain, &mut result[16..4016])
            .unwrap();
        let mut mac = Hmac::<Sha512>::new_from_slice(&self.mac_key).unwrap();
        mac.update(&result[16..4032]);
        mac.update(&1u32.to_le_bytes());
        result[4032..].copy_from_slice(&mac.finalize().into_bytes());
        result
    }
}

/// 원본 테스트와 같은 WAL: 커밋 프레임(updated) 하나와 커밋되지 않은 프레임(unfinished) 하나.
fn wal(updated: &[u8], unfinished: &[u8]) -> Vec<u8> {
    let mut wal = vec![0u8; 32 + 2 * 4120];
    let be = |wal: &mut Vec<u8>, at: usize, value: u32| {
        wal[at..at + 4].copy_from_slice(&value.to_be_bytes())
    };
    be(&mut wal, 0, 0x377f_0682);
    be(&mut wal, 4, 3_007_000);
    be(&mut wal, 8, 4096);
    be(&mut wal, 16, 123);
    be(&mut wal, 20, 456);
    let (mut s0, mut s1) = (0u32, 0u32);
    let mut sum = |wal: &[u8], at: usize, length: usize| -> (u32, u32) {
        for i in (at..at + length).step_by(8) {
            s0 = s0
                .wrapping_add(u32::from_le_bytes(wal[i..i + 4].try_into().unwrap()))
                .wrapping_add(s1);
            s1 = s1
                .wrapping_add(u32::from_le_bytes(wal[i + 4..i + 8].try_into().unwrap()))
                .wrapping_add(s0);
        }
        (s0, s1)
    };
    let (a, b) = sum(&wal, 0, 24);
    be(&mut wal, 24, a);
    be(&mut wal, 28, b);
    for (at, data, commit) in [(32usize, updated, 1u32), (4152, unfinished, 0)] {
        be(&mut wal, at, 1);
        be(&mut wal, at + 4, commit);
        be(&mut wal, at + 8, 123);
        be(&mut wal, at + 12, 456);
        wal[at + 24..at + 24 + PAGE_SIZE].copy_from_slice(data);
        sum(&wal, at, 8);
        let (a, b) = sum(&wal, at + 24, PAGE_SIZE);
        be(&mut wal, at + 16, a);
        be(&mut wal, at + 20, b);
    }
    wal
}

#[test]
fn sqlcipher_reader_restores_sqlite_header_and_reserved_byte_layout() {
    let fixture = Fixture::new();
    let plain = cipher::decrypt(&fixture.page(10), &[], &fixture.key).unwrap();
    assert_eq!(&plain[..16], b"SQLite format 3\0");
    assert_eq!(plain[20], 80);
    assert_eq!(plain[66], 10);
    assert_eq!(plain[18], 1);
}

#[test]
fn cipher_rejects_tampered_pages_and_wrong_keys_before_parsing() {
    let fixture = Fixture::new();
    let original = fixture.page(10);
    let mut corrupted = original.clone();
    corrupted[100] ^= 1;
    assert!(!cipher::verify_key(&corrupted, &fixture.key));
    assert!(!cipher::verify_key(&original, &[0u8; 32]));
    assert!(cipher::verify_key(&original, &fixture.key));
    assert!(cipher::decrypt(&corrupted, &[], &fixture.key).is_err());
}

#[test]
fn wal_applies_committed_updates_but_excludes_a_later_uncommitted_frame() {
    let fixture = Fixture::new();
    let wal = wal(&fixture.page(20), &fixture.page(30));
    assert_eq!(
        cipher::decrypt(&fixture.page(10), &wal, &fixture.key).unwrap()[66],
        20
    );
}

#[test]
fn torn_wal_tail_cannot_replace_the_last_committed_snapshot() {
    let fixture = Fixture::new();
    let mut wal = wal(&fixture.page(20), &fixture.page(30));
    wal[4200] ^= 1;
    assert_eq!(
        cipher::decrypt(&fixture.page(10), &wal, &fixture.key).unwrap()[66],
        20
    );
}

#[test]
fn corrupt_wal_header_is_rejected() {
    let fixture = Fixture::new();
    let mut wal = wal(&fixture.page(20), &fixture.page(30));
    wal[24] ^= 1;
    let error = cipher::decrypt(&fixture.page(10), &wal, &fixture.key).unwrap_err();
    assert_eq!(error.to_string(), "WAL 헤더 검증 실패");
}

#[test]
fn database_size_must_be_whole_pages() {
    let fixture = Fixture::new();
    let mut database = fixture.page(10);
    database.push(0);
    assert!(cipher::decrypt(&database, &[], &fixture.key).is_err());
}
