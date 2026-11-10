# 에이전트 표시 캡처 (docs/10 §D5.11)

`tools/visual/capture.ps1 -Output crates/pod-app/qa/agent-marks -Scenes workspace -Style <id>` — release build, `--ui-fixture`, 보조 모니터, 144 DPI(1440×960 physical).
스타일: quiet · islands · mono · access × dark/light. 각 manifest는 `manifest-<style>.json`, UIA 트리는 `*.tree.txt`.

육안 검토(8장 모두): 작업자 행 이름 앞 mark(Claude 주황 둥근 사각형 C, Codex 녹색-검정 육각형 `>_`, 셸 outline), 상태 아이콘·문구는 오른쪽, 작업 행 끝에 mark 묶음. 행 높이는 테마 그대로(access 40px), 한글 잘림 없음.
UI fixture의 작업자는 작업마다 Claude · Codex · 셸 순서로 에이전트를 보고한다(`source.rs`).
