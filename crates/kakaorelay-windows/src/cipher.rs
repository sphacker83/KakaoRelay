//! SQLCipher 4 페이지 검증·복호화와 WAL 반영. 원본: `LocalChatReader.cs`의 `ChatCipher`.
//! 키는 평문을 쓰기 전에 페이지 HMAC으로 검증한다. OS와 무관하므로 cfg 없이 두고 맥에서도 테스트한다.

use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::NoPadding};
use hmac::{Hmac, Mac};
use sha2::Sha512;
use zeroize::Zeroizing;

pub const PAGE_SIZE: usize = 4096;
const IV_START: usize = 4016;
const HMAC_START: usize = 4032;
const WAL_HEADER: usize = 32;
const WAL_FRAME: usize = 24 + PAGE_SIZE;
/// 512MiB / 4096. 원본과 같은 DB·WAL 페이지 번호 상한.
const MAX_PAGES: u32 = 131_072;

/// 원본 `InvalidDataException`. 메시지는 원본 문구를 그대로 쓴다.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CipherError(pub &'static str);

type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

fn mac_key(first: &[u8], key: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&first[..16]);
    for byte in &mut salt {
        *byte ^= 0x3a;
    }
    let mut out = Zeroizing::new([0u8; 32]);
    pbkdf2::pbkdf2_hmac::<Sha512>(key, &salt, 2, out.as_mut());
    out
}

fn verify(page: &[u8], number: u32, mac_key: &[u8; 32]) -> bool {
    if page.len() != PAGE_SIZE || number == 0 {
        return false;
    }
    let start = if number == 1 { 16 } else { 0 };
    let Ok(mut mac) = Hmac::<Sha512>::new_from_slice(mac_key) else {
        return false;
    };
    mac.update(&page[start..HMAC_START]);
    mac.update(&number.to_le_bytes());
    // verify_slice는 상수 시간 비교다(원본 FixedTimeEquals).
    mac.verify_slice(&page[HMAC_START..]).is_ok()
}

/// 첫 페이지로 키를 검증한다. 원본 `ChatCipher.VerifyKey`.
pub fn verify_key(first: &[u8], key: &[u8]) -> bool {
    first.len() >= PAGE_SIZE
        && key.len() == 32
        && verify(&first[..PAGE_SIZE], 1, &mac_key(first, key))
}

fn be(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("4바이트"))
}

/// SQLite WAL 누적 체크섬. 헤더 magic 끝 비트가 0이면 little endian, 1이면 big endian으로 읽는다.
struct WalChecksum {
    big_endian: bool,
    s0: u32,
    s1: u32,
}

impl WalChecksum {
    fn add(&mut self, bytes: &[u8]) {
        for pair in bytes.as_chunks::<8>().0 {
            let (first, second) = pair.split_at(4);
            let (first, second): ([u8; 4], [u8; 4]) = (
                first.try_into().expect("4바이트"),
                second.try_into().expect("4바이트"),
            );
            let (a, b) = if self.big_endian {
                (u32::from_be_bytes(first), u32::from_be_bytes(second))
            } else {
                (u32::from_le_bytes(first), u32::from_le_bytes(second))
            };
            self.s0 = self.s0.wrapping_add(a).wrapping_add(self.s1);
            self.s1 = self.s1.wrapping_add(b).wrapping_add(self.s0);
        }
    }
}

/// 커밋된 WAL 프레임을 반영한 뒤 모든 페이지를 인증·복호화해 SQLite 평문 이미지를 만든다. 원본 `ChatCipher.Decrypt`.
pub fn decrypt(database: &[u8], wal: &[u8], key: &[u8]) -> Result<Zeroizing<Vec<u8>>, CipherError> {
    if database.len() < PAGE_SIZE
        || !database.len().is_multiple_of(PAGE_SIZE)
        || !verify_key(database, key)
    {
        return Err(CipherError(
            "지원하지 않는 DB 형식이거나 키 검증에 실패했습니다.",
        ));
    }
    let mac_key = mac_key(database, key);
    let mut merged = Zeroizing::new(database.to_vec());
    if wal.len() > WAL_HEADER {
        let magic = be(wal, 0);
        if !matches!(magic, 0x377f_0682 | 0x377f_0683)
            || be(wal, 8) as usize != PAGE_SIZE
            || be(wal, 4) != 3_007_000
        {
            return Err(CipherError("지원하지 않는 WAL 형식입니다."));
        }
        let mut sum = WalChecksum {
            big_endian: magic == 0x377f_0683,
            s0: 0,
            s1: 0,
        };
        sum.add(&wal[..24]);
        if sum.s0 != be(wal, 24) || sum.s1 != be(wal, 28) {
            return Err(CipherError("WAL 헤더 검증 실패"));
        }
        let mut frames: Vec<(u32, usize)> = Vec::new();
        let (mut committed, mut final_size) = (0usize, 0u32);
        let mut offset = WAL_HEADER;
        while offset + WAL_FRAME <= wal.len() {
            if wal[offset + 8..offset + 16] != wal[16..24] {
                break;
            }
            sum.add(&wal[offset..offset + 8]);
            sum.add(&wal[offset + 24..offset + WAL_FRAME]);
            // 커밋되지 않았거나 잘린 꼬리는 마지막 커밋 스냅숏을 바꾸지 못한다.
            if sum.s0 != be(wal, offset + 16) || sum.s1 != be(wal, offset + 20) {
                break;
            }
            let page = be(wal, offset);
            if page == 0
                || page > MAX_PAGES
                || !verify(&wal[offset + 24..offset + WAL_FRAME], page, &mac_key)
            {
                return Err(CipherError("WAL 페이지 인증 실패"));
            }
            frames.push((page, offset + 24));
            if be(wal, offset + 4) != 0 {
                committed = frames.len();
                final_size = be(wal, offset + 4);
            }
            offset += WAL_FRAME;
        }
        if committed > 0 {
            if final_size > MAX_PAGES {
                return Err(CipherError("DB 크기 제한 초과"));
            }
            merged.resize(final_size as usize * PAGE_SIZE, 0);
            for &(page, at) in &frames[..committed] {
                if page <= final_size {
                    let target = (page as usize - 1) * PAGE_SIZE;
                    merged[target..target + PAGE_SIZE].copy_from_slice(&wal[at..at + PAGE_SIZE]);
                }
            }
        }
    }
    let mut plain = Zeroizing::new(vec![0u8; merged.len()]);
    for (index, page) in merged.as_chunks::<PAGE_SIZE>().0.iter().enumerate() {
        if !verify(page, index as u32 + 1, &mac_key) {
            return Err(CipherError("DB 페이지 인증 실패"));
        }
        let start = if index == 0 { 16 } else { 0 };
        let offset = index * PAGE_SIZE;
        let decryptor = Aes256CbcDec::new_from_slices(key, &page[IV_START..HMAC_START])
            .map_err(|_| CipherError("지원하지 않는 DB 형식이거나 키 검증에 실패했습니다."))?;
        decryptor
            .decrypt_padded_b2b_mut::<NoPadding>(
                &page[start..IV_START],
                &mut plain[offset + start..offset + IV_START],
            )
            .map_err(|_| CipherError("DB 페이지 인증 실패"))?;
    }
    plain[..16].copy_from_slice(b"SQLite format 3\0");
    // 커밋된 WAL은 이미 합쳤으므로 메모리 스냅숏이 WAL 파일을 열려고 하면 안 된다.
    plain[18] = 1;
    plain[19] = 1;
    // 코덱의 예약 80바이트를 유지한다. b-tree 셀은 이 사용 가능 페이지 크기로 배치되어 있다.
    if plain[20] != 80 {
        return Err(CipherError("예상하지 못한 SQLite 페이지 형식"));
    }
    Ok(plain)
}
