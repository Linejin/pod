//! Korean catalog for gpui-kit owned strings (dock, inputs, dialogs, diff accessibility),
//! installed through rust-i18n's extension hook.

// Toolkit-owned menus and diff accessibility use the same host catalog.
const TOOLKIT_COPY: &[(&str, &str)] = &[
    ("Attachment.Remove", "삭제"),
    ("Attachment.Retry", "다시 시도"),
    ("Select.placeholder", "선택하세요"),
    ("ComboBox.placeholder", "선택하세요"),
    ("ComboBox.search_placeholder", "검색…"),
    ("ComboBox.empty", "검색 결과 없음"),
    ("DropdownButton.more_options", "더 보기"),
    ("Dock.Unnamed", "이름 없음"),
    ("Dock.Close", "닫기"),
    ("Dock.PanelMenu", "패널 메뉴"),
    ("Dock.Zoom In", "크게 보기"),
    ("Dock.Zoom Out", "원래 크기로"),
    ("Dock.Collapse", "접기"),
    ("Dock.Expand", "펼치기"),
    (
        "Dock.Unavailable Panel",
        "이 버전에서 사용할 수 없는 패널입니다.",
    ),
    ("Dialog.ok", "확인"),
    ("Dialog.cancel", "취소"),
    ("Command.placeholder", "명령 입력 또는 검색…"),
    ("Command.empty", "검색 결과가 없습니다."),
    ("List.search_placeholder", "검색…"),
    ("Input.Decrement", "줄이기"),
    ("Input.Increment", "늘리기"),
    ("Input.Replace", "바꾸기"),
    ("Input.Replace All", "모두 바꾸기"),
    ("Input.Clear", "지우기"),
    ("Input.Fold", "접기"),
    ("Input.Unfold", "펼치기"),
    ("Input.Cut", "잘라내기"),
    ("Input.Copy", "복사"),
    ("Input.Paste", "붙여넣기"),
    ("Input.Select All", "모두 선택"),
    ("Input.Go to Definition", "정의로 이동"),
    ("Input.Show Code Actions", "코드 작업 보기"),
    ("Input.Show Password", "비밀번호 표시"),
    ("Input.Hide Password", "비밀번호 숨기기"),
    ("Settings.search_placeholder", "검색…"),
    ("Settings.no_results", "일치하는 설정이 없습니다"),
    (
        "Settings.no_results_description",
        "다른 검색어로 찾아보세요.",
    ),
    ("Settings.Reset All", "모두 초기화"),
    ("Pagination.previous", "이전"),
    ("Pagination.next", "다음"),
    ("Diff.BinaryChanges", "바이너리 파일 변경"),
    ("Diff.NoTextChanges", "텍스트 변경 없음"),
    ("Diff.NoChanges", "변경 없음"),
    ("Diff.EmptyFile", "빈 파일"),
    ("Diff.AddedFile", "추가된 파일"),
    ("Diff.DeletedFile", "삭제된 파일"),
    ("Diff.RenamedFile", "이름 변경"),
    ("Diff.CopiedFile", "복사됨"),
    ("Diff.HiddenLines", "패치에 없는 %{count}줄"),
    ("Diff.ExpandAbove", "위 %{count}줄 보기"),
    ("Diff.ExpandBelow", "아래 %{count}줄 보기"),
    ("Diff.CollapseFile", "%{name} 접기"),
    ("Diff.ExpandFile", "%{name} 펼치기"),
    ("Diff.AddAnnotation", "%{line}줄에 의견 추가"),
    ("Diff.ConflictedFile", "충돌"),
    ("Diff.CurrentChange", "현재 변경"),
    ("Diff.IncomingChange", "들어오는 변경"),
    ("Diff.BaseChange", "공통 기준"),
    ("Diff.AcceptCurrent", "현재 변경 사용"),
    ("Diff.AcceptIncoming", "들어오는 변경 사용"),
    ("Diff.AcceptBoth", "두 변경 모두 사용"),
    ("Diff.AcceptedCurrent", "현재 변경 사용됨"),
    ("Diff.AcceptedIncoming", "들어오는 변경 사용됨"),
    ("Diff.AcceptedBoth", "두 변경 모두 사용됨"),
    ("Diff.UndoResolution", "되돌리기"),
    ("Diff.Original", "원본"),
    ("Diff.Modified", "변경"),
    ("Diff.ExpandLines", "변경 없는 %{count}줄 펼치기"),
    ("Diff.SourceLine", "%{side} %{line}줄"),
    ("Diff.AddedLine", "%{side} %{line}줄, 추가됨"),
    ("Diff.RemovedLine", "%{side} %{line}줄, 삭제됨"),
    ("Diff.UnchangedLines", "변경 없는 %{count}줄"),
    ("Diff.SelectLine", "%{side} %{line}줄 선택"),
    ("Diff.NoNewline", "파일 끝에 줄바꿈 없음"),
    ("Diff.Viewer", "%{name} 변경 비교"),
    ("Diff.ViewerFiles", "파일 %{count}개 변경 비교"),
];

struct KoreanToolkit;
impl rust_i18n::Backend for KoreanToolkit {
    fn available_locales(&self) -> Vec<std::borrow::Cow<'_, str>> {
        vec!["ko".into()]
    }
    fn translate(&self, locale: &str, key: &str) -> Option<std::borrow::Cow<'_, str>> {
        if locale != "ko" {
            return None;
        }
        let key = key.strip_prefix("pod.")?;
        TOOLKIT_COPY
            .iter()
            .find(|(id, _)| *id == key)
            .map(|(_, text)| (*text).into())
    }
    fn messages_for_locale(
        &self,
        locale: &str,
    ) -> Option<Vec<(std::borrow::Cow<'_, str>, std::borrow::Cow<'_, str>)>> {
        (locale == "ko").then(|| {
            TOOLKIT_COPY
                .iter()
                .map(|(key, text)| (format!("pod.{key}").into(), (*text).into()))
                .collect()
        })
    }
}
/// rust-i18n's exported extension hook keeps library source and generated assets untouched.
pub fn init_toolkit_locale() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    static KOREAN: KoreanToolkit = KoreanToolkit;
    ONCE.call_once(|| {
        gpui_kit::component::_rust_i18n_extend(&KOREAN, "pod");
        gpui_kit::component::set_locale("ko");
    });
}
