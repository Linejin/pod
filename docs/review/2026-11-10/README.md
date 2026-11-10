# 전체 평가 (2026-11-10, 기준 master `47dff8a`)

영역별 평가 워커 6명(Codex Astra 3, Claude 3)이 각자 독립적으로 매긴 점수와 확인된 발견 사항. 각 보고서에 기준별 점수·근거(file:line)·수정 제안이 있다.

| 영역 | 점수 | 치명 | 높음 | 중간 | 낮음 | 보고서 |
|---|---|---|---|---|---|---|
| 데몬 코어 | 7.05 | 0 | 3 | 2 | 0 | [daemon.md](daemon.md) |
| 오케스트레이션·세션 | 6.9 | 0 | 3 | 2 | 0 | [orch.md](orch.md) |
| 보안·개인정보 | 6.8 | 0 | 1 | 3 | 2 | [security.md](security.md) |
| 엔지니어링 품질·출시 준비 | 6.7 | 0 | 1 | 5 | 8 | [quality.md](quality.md) |
| 앱 UI·UX·디자인 | 6.6 | 0 | 2 | 8 | 9 | [ui.md](ui.md) |
| 편집기·LSP·리뷰·PR | 6.2 | 0 | 2 | 5 | 7 | [editor.md](editor.md) |
| **평균** | **6.71** | **0** | **12** | **25** | **26** | 63건 |

## 공통 원인 (여러 영역에 겹침)

- **자식 프로세스·출력 제한 없음**: archive·forge 시간 제한이 프로세스 트리와 출력 크기를 제한하지 않음(orch-03, security-02, security-03) → 공용 "제한된 자식 프로세스 실행" 도우미 하나로.
- **실패를 성공으로 응답**: discovery 끄기·삭제·설정 저장(orch-05, security-04), 손상된 settings.json 조용히 덮어씀(ui-09).
- **영속 순서 문제**: 상태를 먼저 저장하고 부수 효과(우편·입력)를 나중에(orch-01, orch-02).
- **저장소가 실행 명령을 정함**: `.pod/lsp.json`(editor-01)은 `.pod/config.toml`의 해시 승인 방식을 따라야 함.
- **검증 장치 노후**: 시각 기준 이미지 36/36 불일치(ui-01), CI 없음(quality-01), fmt 미적용(quality-02).

## 수정 계획

| 묶음 | 내용 | 시점 |
|---|---|---|
| fix-editor | editor-01~14 | 바로 |
| fix-ui | ui-02~19, 마지막에 ui-01 기준 이미지 다시 찍기 | 바로 |
| fix-quality | quality-03·04·07~14, security-06, CI 워크플로 파일(실행은 workflow 권한 필요) | 바로 |
| fix-daemon | daemon-01~05, security-01, security-05 | orch-dogfood-fixes 병합 뒤 |
| fix-orch | orch-01~05, security-02~04(공용 제한 실행 도우미) | orch-dogfood-fixes 병합 뒤 |
| fmt + 공개 | quality-02(전체 `cargo fmt`, 다른 작업이 없을 때 한 번), quality-05(공개 저장소 갱신 — 사용자 확인 뒤) | 마지막 |

## 재평가 (수정 뒤, 같은 날, Codex Astra 3명)

같은 기준·가중치로 다시 매김. 이전 지적 63건 중 **해결 확인 52 · 부분 8 · 미해결 2 · 회귀 1**(docs/19 버전 표기, 바로 고침).

| 영역 | 전 | 후 | 보고서 |
|---|---|---|---|
| 데몬 코어 | 7.05 | 7.53 | [rescore-daemon-security.md](rescore-daemon-security.md) |
| 보안·개인정보 | 6.80 | 7.15 | 〃 |
| 오케스트레이션·세션 | 6.85 | 7.05 | [rescore-orch-quality.md](rescore-orch-quality.md) |
| 엔지니어링 품질·출시 준비 | 6.70 | 7.10 | 〃 |
| 앱 UI·UX·디자인 | 6.60 | 7.45 | [rescore-ui-editor.md](rescore-ui-editor.md) |
| 편집기·LSP·리뷰·PR | 6.20 | 7.25 | 〃 |
| **평균** | **6.70** | **7.25** | |

새로 확인된 문제(중복 제거): 높음 4 · 중간 약 10 · 낮음 3. 세 평가자가 모두 찾은 것: **`pod orch commit`이 저장소의 git clean/process filter를 샌드박스 밖에서 실행**(오케스트레이션 보안 점수 7 → 5의 원인).
