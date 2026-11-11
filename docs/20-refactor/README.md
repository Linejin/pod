# 리팩터링 전체 순서 (2026-11-10)

세 설계 문서의 카드 47장을 한 순서로 묶는다. 카드 내용(OWNED 파일, 단계, 동작 보존 증명)은 각 문서가 기준이다.

- [daemon.md](daemon.md): podd·프로토콜·pod-cli·hook·status·remote — D-F0~F3, D-P1~P9, D-T1~T3 (16장)
- [libs.md](libs.md): editor·diff·orch·forge·mods·discover·공용 crate·테스트·tools — L-F0, L-L1~L10, L-T1 (12장)
- [app.md](app.md): pod-app·pod-ui·pod-terminal-view — A-F0~F2, A-P1~P14, A-T1~T2 (19장)

## 원칙

- 동작 보존: 기존 테스트 전부, 프로토콜 JSON golden(D-F0), 기준 이미지 비교 0 실패, 프로토콜 버전 변경 없음.
- 동시에 빌드하는 워커는 3개까지. 같은 시점에 실행 중인 카드끼리 OWNED 파일이 겹치지 않는다.
- `Cargo.toml`·`Cargo.lock`을 바꾸는 카드는 한 번에 하나.
- 백로그(`docs/01` §8)와 이 문서는 코디네이터만 고친다.

## 설계 사이 충돌과 조정

| 겹치는 파일 | 카드 | 조정 |
|---|---|---|
| `Cargo.toml`, `Cargo.lock`, `crates/podd/Cargo.toml` | D-F1 × L-F0 (둘 다 wave 0) | L-F0 먼저, D-F1은 L-F0 병합 뒤 |
| `crates/podd/src/git.rs` | D-P5 × L-L1 | L-L1 먼저(bounded·git 환경을 pod-process로), D-P5는 그 뒤 |
| `crates/podd/tests/{orch_runs,pr,…}.rs`, `tests/common/mod.rs` | L-L9 × D-F0·D-P2·D-P5 | L-L9는 D-F0·D-P2·D-P5 병합 뒤 |
| `crates/pod-remote/tests/system_ssh.rs` | L-L10 × D-P9 | L-L10은 D-P9 병합 뒤 |
| `crates/pod-app/src/{workspace.rs,native_qa.rs}` | A-F1·A-F2 × astra-ui(남은 UI 수정 3건) | A-F1·A-F2는 astra-ui 병합 뒤 |

## 순서

| 단계 | 카드 | 조건 |
|---|---|---|
| G0 기초 | D-F0, L-F0, A-F0 | 바로(서로 파일이 겹치지 않음) |
| G0 이어서 | D-F1 → D-F2 → D-F3 | D-F1은 D-F0·L-F0 뒤, 나머지는 앞 카드 뒤 |
| G0 이어서 | A-F1, A-F2 | A-F0·astra-ui 뒤(A-F2는 astra-ui 뒤) |
| G1 병렬 | L-L1~L-L8, L-L10, D-P1~D-P9, A-P1~A-P14 | 각 문서의 wave 0 카드가 모두 병합된 뒤. 위 조정 순서 지킴(L-L1→D-P5, D-P9→L-L10, D-P2·D-P5→L-L9) |
| G2 마무리 | D-T1 → D-T2 → D-T3, L-T1, A-T1, A-T2 | G1 전부 뒤, Cargo 파일을 바꾸는 D-T1과 L-T1은 차례로 |
| G3 검증 | 기준 이미지 비교·재촬영, 전체 fmt, Astra 재평가 | 마지막 |

G1 안에서는 크기가 큰(L) 카드를 먼저 띄워, 작은 카드가 빈 자리를 채우게 한다.

## A-T2 나누기 (2026-11-11)

레이아웃 카드 5장(A-P10~P14)이 보고한 kit/host 부족분이 약 40가지라 A-T2 한 장으로는 작업 분배 기준(항목 8, 파일 15)을 넘는다. 다음처럼 나눈다.

| 카드 | 내용 | 순서 |
|---|---|---|
| A-T2a | kit·host 보강: 창 키·관찰 연결, dock 접근, 프레임 읽기(라벨·현재 작업·트리 선택·패널 수), 동작(터미널 보이기·작업 터미널 열기·승인 결정·터미널 입력·알림) | 먼저 |
| A-T2b | rail·queue를 renderer()로, legacy 어댑터 제거 | T2a 뒤 병렬 |
| A-T2c | dense·kanban | T2a 뒤 병렬 |
| A-T2d | blocks·review | T2a 뒤 병렬 |
| A-T2z | 작업대 renderer, legacy hook 제거, `main.rs`·`native_qa/layout_contract.rs`, 7개 모드 순환 검사, fan-in/out 재측정 | 마지막 |
