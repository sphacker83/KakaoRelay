//! 요청 ID 형식과 메시지 ID 비교(원본 `BigInteger.Parse`).

use std::cmp::Ordering;

use kakaorelay_ai::{Error, Result};

/// 원본 정규식 `\A[a-zA-Z0-9_-]{1,80}\z`.
pub fn valid_request_id(id: &str) -> bool {
    (1..=80).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// 크기 제한 없는 정수 메시지 ID. 카카오 메시지 ID는 u64 범위를 넘을 수 있어 문자열 자릿수로 비교한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageId {
    negative: bool,
    digits: String,
}

impl MessageId {
    /// 원본 `BigInteger.Parse`(앞뒤 공백, 선택 부호 허용). 형식이 아니면 `FormatException`이다.
    pub fn parse(text: &str) -> Result<Self> {
        let trimmed = text.trim();
        let (negative, body) = match trimmed.as_bytes().first() {
            Some(b'-') => (true, &trimmed[1..]),
            Some(b'+') => (false, &trimmed[1..]),
            _ => (false, trimmed),
        };
        if body.is_empty() || !body.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::Format(format!("메시지 ID 형식이 아닙니다: {text}")));
        }
        let digits = body.trim_start_matches('0');
        let digits = if digits.is_empty() { "0" } else { digits };
        Ok(Self {
            negative: negative && digits != "0",
            digits: digits.to_string(),
        })
    }

    pub fn is_positive(&self) -> bool {
        !self.negative && self.digits != "0"
    }
}

impl Ord for MessageId {
    fn cmp(&self, other: &Self) -> Ordering {
        let magnitude = self
            .digits
            .len()
            .cmp(&other.digits.len())
            .then_with(|| self.digits.cmp(&other.digits));
        match (self.negative, other.negative) {
            (false, false) => magnitude,
            (true, true) => magnitude.reverse(),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
        }
    }
}

impl PartialOrd for MessageId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// 두 메시지 ID 문자열 비교. 어느 쪽이든 형식이 아니면 오류다.
pub fn compare(a: &str, b: &str) -> Result<Ordering> {
    Ok(MessageId::parse(a)?.cmp(&MessageId::parse(b)?))
}

/// 원본 `long.TryParse`/`BigInteger.TryParse` 후 양수 검사. 사용자 ID에 쓴다.
pub fn is_positive_id(text: &str) -> bool {
    MessageId::parse(text).is_ok_and(|id| id.is_positive())
}
