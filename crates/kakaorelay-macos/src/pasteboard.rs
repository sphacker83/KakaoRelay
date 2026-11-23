//! 일반 클립보드 보관·복원. 이미지 발송은 파일 URL을 붙여넣으므로 사용자 클립보드를 모든 타입 그대로 되돌려야 한다.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSData, NSString, NSURL};

use kakaorelay_core::platform::TransportError;

use crate::app::failed;

/// 클립보드 항목별 (타입, 데이터) 사본.
pub struct Snapshot {
    items: Vec<Vec<(Retained<NSString>, Retained<NSData>)>>,
}

// NSData·NSString은 불변 객체라 스레드 간 이동이 안전하다. 전송 객체는 `Send`여야 한다.
unsafe impl Send for Snapshot {}

fn general() -> Retained<NSPasteboard> {
    NSPasteboard::generalPasteboard()
}

/// 클립보드 변경 횟수. 복사가 실제로 일어났는지 판별한다.
pub fn change_count() -> isize {
    general().changeCount()
}

/// 클립보드의 일반 텍스트.
pub fn text() -> Option<String> {
    general()
        .stringForType(&NSString::from_str("public.utf8-plain-text"))
        .map(|s| s.to_string())
}

pub fn save() -> Snapshot {
    let board = general();
    let items = board
        .pasteboardItems()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.types()
                        .iter()
                        .filter_map(|kind| item.dataForType(&kind).map(|data| (kind.clone(), data)))
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default();
    Snapshot { items }
}

/// 보관한 내용으로 되돌린다. 원래 비어 있었으면 비운 상태로 둔다.
pub fn restore(snapshot: &Snapshot) -> Result<(), TransportError> {
    let board = general();
    board.clearContents();
    if snapshot.items.is_empty() {
        return Ok(());
    }
    let objects: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = snapshot
        .items
        .iter()
        .map(|types| {
            let item = NSPasteboardItem::new();
            for (kind, data) in types {
                item.setData_forType(data, kind);
            }
            ProtocolObject::from_retained(item)
        })
        .collect();
    if board.writeObjects(&NSArray::from_retained_slice(&objects)) {
        Ok(())
    } else {
        Err(failed("클립보드를 원래 내용으로 복원하지 못했습니다."))
    }
}

pub fn put_file(path: &std::path::Path) -> Result<(), TransportError> {
    let url = NSURL::from_file_path(path)
        .ok_or_else(|| failed("첨부 파일 경로를 URL로 만들지 못했습니다."))?;
    let board = general();
    board.clearContents();
    let objects = [ProtocolObject::<dyn NSPasteboardWriting>::from_retained(
        url,
    )];
    if board.writeObjects(&NSArray::from_retained_slice(&objects)) {
        Ok(())
    } else {
        Err(failed("첨부 파일을 클립보드에 넣지 못했습니다."))
    }
}
