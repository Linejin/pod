//! Korean copy added for the pod-app integration (save-before-close, tree
//! file operations, accessible names). Older strings stay inline next to the
//! code that renders them.

pub const UNSAVED_TITLE: &str = "저장하지 않은 변경";
pub const SAVE_AND_CLOSE: &str = "저장 후 닫기";
pub const CLOSE_WITHOUT_SAVING: &str = "저장하지 않고 닫기";
pub const CANCEL: &str = "취소";
pub const NEW_FILE: &str = "새 파일…";
pub const NEW_FOLDER: &str = "새 폴더…";
pub const RENAME: &str = "이름 바꾸기…";
pub const DELETE: &str = "삭제…";
pub const NEW_FILE_TITLE: &str = "새 파일";
pub const NEW_FOLDER_TITLE: &str = "새 폴더";
pub const RENAME_TITLE: &str = "이름 바꾸기";
pub const DELETE_TITLE: &str = "삭제";
pub const NAME_PLACEHOLDER: &str = "이름 (하위 폴더는 / 로 구분)";
pub const ENTER_CREATE: &str = "Enter: 만들기 · Esc: 취소";
pub const ENTER_RENAME: &str = "Enter: 바꾸기 · Esc: 취소";
pub const DELETE_CONFIRM: &str = "삭제";
pub const TREE_LABEL: &str = "파일 트리";
pub const TABS_LABEL: &str = "열린 파일";
pub const CLOSE_TAB: &str = "탭 닫기";
pub const SAVE_ALL_AND_CLOSE: &str = "모두 저장 후 닫기";

// Tab menu (right click, Shift+F10 / menu key) and the open-files list.
pub const MENU_CLOSE: &str = "닫기";
pub const MENU_CLOSE_OTHERS: &str = "다른 탭 닫기";
pub const MENU_CLOSE_RIGHT: &str = "오른쪽 탭 닫기";
pub const MENU_CLOSE_SAVED: &str = "저장된 탭 닫기";
pub const MENU_CLOSE_ALL: &str = "모두 닫기";
pub const MENU_PIN: &str = "고정";
pub const MENU_UNPIN: &str = "고정 해제";
pub const MENU_COPY_PATH: &str = "경로 복사";
pub const MENU_COPY_REL_PATH: &str = "상대 경로 복사";
pub const MENU_REVEAL: &str = "파일 트리에서 보기";
pub const UNAVAILABLE: &str = "사용할 수 없음";
pub const OPEN_TABS_BUTTON: &str = "열린 파일 모두 보기";
pub const OPEN_TABS_MENU: &str = "열린 파일 목록";
pub const CURRENT_TAB: &str = "현재 탭";

pub fn tab_menu_label(name: &str) -> String {
    format!("탭 메뉴: {name}")
}
/// Visible tab title: the name, and for same-named tabs the shortest
/// distinguishing parent path (`mod.rs · podd/src`).
pub fn tab_title(name: &str, parent: Option<&str>) -> String {
    match parent {
        Some(p) => format!("{name} · {p}"),
        None => name.to_string(),
    }
}
pub fn copied(path: &str) -> String {
    format!("복사했습니다: {path}")
}
/// Bulk close with unsaved files: what closes and which files are unsaved.
pub fn unsaved_many_body(closing: usize, dirty: usize) -> String {
    format!("탭 {closing}개를 닫습니다. 다음 파일 {dirty}개에 저장하지 않은 변경이 있습니다.")
}

pub fn unsaved_body(rel: &str) -> String {
    format!("{rel}에 저장하지 않은 변경이 있습니다. 닫기 전에 저장할까요?")
}
pub fn delete_body(rel: &str, dir: bool) -> String {
    if dir {
        format!("폴더 {rel}와 그 안의 모든 파일을 삭제합니다. 되돌릴 수 없습니다.")
    } else {
        format!("{rel} 파일을 삭제합니다. 되돌릴 수 없습니다.")
    }
}
pub fn editor_label(rel: &str) -> String {
    format!("편집기: {rel}")
}
/// The file changed (unsaved edits kept) or vanished on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabDisk {
    Changed,
    Deleted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TabFlags {
    pub dirty: bool,
    pub preview: bool,
    pub disk: Option<TabDisk>,
}

/// Accessible tab name: `main.rs — src/main.rs (미리 보기, 저장 안 됨)`.
pub fn tab_label_full(name: &str, rel: &str, f: TabFlags) -> String {
    let mut notes: Vec<&str> = Vec::new();
    if f.preview {
        notes.push("미리 보기");
    }
    if f.dirty {
        notes.push("저장 안 됨");
    }
    match f.disk {
        Some(TabDisk::Changed) => notes.push("디스크에서 바뀜"),
        Some(TabDisk::Deleted) => notes.push("디스크에서 삭제됨"),
        None => {}
    }
    let base = if rel.is_empty() || rel == name {
        name.to_string()
    } else {
        format!("{name} — {rel}")
    };
    if notes.is_empty() {
        base
    } else {
        format!("{base} ({})", notes.join(", "))
    }
}

pub const NOTHING_TO_REOPEN: &str = "다시 열 탭이 없습니다";

pub fn created(rel: &str) -> String {
    format!("만들었습니다: {rel}")
}
pub fn renamed(from: &str, to: &str) -> String {
    format!("이름을 바꿨습니다: {from} → {to}")
}
pub fn deleted(rel: &str) -> String {
    format!("삭제했습니다: {rel}")
}
pub fn fs_error(what: &str, e: &dyn std::fmt::Display) -> String {
    format!("{what}하지 못했습니다: {e}")
}
pub const ALREADY_EXISTS: &str = "같은 이름이 이미 있습니다";
pub const INVALID_NAME: &str = "이름이 올바르지 않습니다";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_names_say_path_and_state() {
        assert_eq!(
            tab_label_full("main.rs", "main.rs", TabFlags::default()),
            "main.rs"
        );
        assert_eq!(
            tab_label_full(
                "main.rs",
                "src/main.rs",
                TabFlags {
                    dirty: true,
                    ..Default::default()
                }
            ),
            "main.rs — src/main.rs (저장 안 됨)"
        );
        let all = TabFlags {
            dirty: true,
            preview: true,
            disk: Some(TabDisk::Deleted),
        };
        assert_eq!(
            tab_label_full("노트.md", "docs/노트.md", all),
            "노트.md — docs/노트.md (미리 보기, 저장 안 됨, 디스크에서 삭제됨)"
        );
        assert_eq!(tab_title("mod.rs", Some("podd/src")), "mod.rs · podd/src");
        assert_eq!(tab_title("mod.rs", None), "mod.rs");
        assert_eq!(
            unsaved_many_body(3, 2),
            "탭 3개를 닫습니다. 다음 파일 2개에 저장하지 않은 변경이 있습니다."
        );
    }
}
