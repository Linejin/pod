# SignPath Foundation OSS 신청과 Windows 빌드 준비

**미신청·미승인.** 무료 OSS 지원을 우선 검토하지만 제공 여부는 Foundation의
심사 결정이다. 공개 저장소만으로 승인·무료 서명·SAC 실행을 보장하지 않는다.
현재 작업은 로컬 준비이며 계정 생성, 제출, push, 릴리스, 워크플로 실행은 하지
않았다. 기본 경로에 Azure 계정이나 유료 인증서는 필요하지 않다.

공식 근거: [신청](https://signpath.org/apply),
[지원 조건](https://signpath.org/terms.html),
[GitHub 출처 검증](https://docs.signpath.io/trusted-build-systems/github),
[artifact 구성](https://docs.signpath.io/artifact-configuration/).

## 신청 전 확인 목록

| 요건 | 확인된 사실 | 남은 조치 / 입력 |
|---|---|---|
| OSI 라이선스·공개 소스 | Pod는 Apache-2.0, remote는 `https://github.com/Linejin/pod`. ConPTY는 MIT. | 모든 번들·의존성 라이선스와 비독점 구성 확인. `cargo deny`만으로 Foundation 적격성 전체를 보장하지 않음. |
| 유지보수·평판 | 저장소에 변경 이력, 설계 문서, 테스트가 있음. pre-alpha. | 관리자에게 검증 가능한 사용/기여/유지보수 근거 요청. 사용자 수·평판을 추정하지 않음. 실행 프로그램은 별도 평판 심사 대상. |
| 서명할 형태의 기존 릴리스 | 사용자 확인상 GitHub 릴리스 없음. 이 PC의 Rust는 4551로 차단. | 외부 Windows에서 실제 설치 EXE·ZIP 빌드/시험 → 관리자가 공개 unsigned 릴리스 생성. Actions artifact만으로 공개 릴리스 조건을 충족했다고 간주하지 않음. |
| 다운로드 기능 설명 | README에 IDE·에이전트·worktree·터미널 기능 설명이 있음. | 실제 다운로드 URL, 버전, 시스템 요구, 설치·제거·unsigned/SAC 제한을 릴리스 페이지에 명시. |
| 역할·MFA | 현재 팀 권한/MFA는 미확인. | 커미터·리뷰어·서명 승인자 명단/공개 팀 URL, GitHub와 SignPath MFA를 관리자 확인. 개인정보/복구 코드 공개 금지. |
| 수동 서명 승인 | 정책 초안만 준비. | 모든 production 요청의 수동 승인, submitter/approver 구분을 SignPath 정책에서 설정. GitHub gate만으로 대신하지 않음. |
| Code signing policy | [정책 초안](../../CODE_SIGNING_POLICY.md), README 링크 추가. | 실제 역할 입력, 관리자 확정, 다운로드/릴리스 페이지에서도 동일 제목으로 연결. |
| 개인정보·네트워크 | [소스 기반 초안](../../PRIVACY.md). GitHub PR 자동 polling, SSH, 외부 에이전트 호출 확인. | 담당 연락처·서비스 정책·보존·네트워크 시험 결과 입력. 필요 시 설치 중 고지·비활성화 옵션 구현. |
| 제품 메타데이터 | `assets/windows/resources.rs`가 5개 Pod PE의 ProductName=Pod / ProductVersion을 설정함. | 실제 빌드 파일을 `-RequireMetadata`로 검사. Foundation 서버에서도 이름·버전 제한 유지. |

신청 본문은 [미제출 초안](signpath-application.md)에 있다. 이름·연락처·조직,
역할·MFA·릴리스 URL·사용 근거는 빈칸을 실제 값으로 채운 뒤 관리자가 제출한다.
정확한 신청 폼 필드는 제출 시 공식 페이지에서 확인한다.

## 먼저 unsigned 신청용 배포본 만들기

`.github/workflows/windows-installer.yml`은 GitHub-hosted `windows-2022`에서
수동 실행하는 **unsigned** 전용 준비다. 서명 변수·비밀·계정 없이 Inno 6.7.3과
Rust 1.100.0, cargo-deny 0.20.2를 사용해 빌드한다. ConPTY의 고정 패키지
해시와 기존 Microsoft 서명/타임스탬프도 확인한다. 공개 릴리스를 만들지 않는다.

외부 Windows에서 수동으로도 실행할 수 있다(도구 설치는 [일반 빌드 절차](signing.md)):

```powershell
powershell -NoProfile -File tools/installer/build.ps1 -CheckPrerequisites -Unsigned
powershell -NoProfile -File tools/installer/build.ps1 -Unsigned
$label = powershell -NoProfile -File tools/installer/build.ps1 -PrintVersion
powershell -NoProfile -File tools/installer/prepare-signpath.ps1 -Stage "dist/stage/Pod-$label"
```

정식 숫자 버전의 unsigned 릴리스 후보가 필요하면, 관리자가 준비한 깨끗한
`v<workspace version>` 태그에서 `build.ps1 -Release -Unsigned`를 명시적으로
사용한다. 공인 서명 자격이 없는 정상 준비 경로이며 SAC용으로 표시하지 않는다.

실제 결과가 생기면 설치 EXE·포터블 ZIP·SHA256SUMS와 설치/제거 설명을
검토한다. 보호 설정을 바꾸지 않고 빌드가 허용되는 시험 환경에서 unsigned
설치/제거를 시험하고, SAC 대상 PC에는 unsigned 실행을 권하지 않는다.
후속 공개 릴리스 게시와 신청은 사용자가 별도로 수행해야 한다.

`prepare-signpath.ps1`은 stage의 원본 Microsoft 구성 요소를 검증하되 요청
디렉터리에는 **Pod EXE 5개만** 복사한다. `readiness.json`은 해시·제품 버전·
upstream 검증 결과를 기록한다. 메타데이터가 미완성이어도 신청용 샘플 준비는
가능하게 경고하고, 실제 서명 제출 전에는 `-RequireMetadata`를 반드시 사용한다.
이 manifest는 참고 자료이며 서비스가 검증한 GitHub 출처 증명을 대체하지 않는다.

## 승인 후 연동 경로 (아직 활성화하지 않음)

1. Foundation이 승인한 조직·프로젝트·정책 ID/slug, 인증서 사용 조건을 확인한다. 인증서 명의는 Foundation이며 프로젝트 자체 명의 인증서를 발급받는 모델이 아니다.
2. 모든 참여자 MFA, 커미터/리뷰어/승인자 역할, production 요청별 수동 승인, 허용 저장소/브랜치/태그·GitHub-hosted 빌드·필수 리뷰 제한을 관리자와 SignPath가 설정한다. 일반 API 토큰으로 로컬 파일의 출처를 우회하지 않는다.
3. 같은 공개 GitHub-hosted 실행에서 소스 빌드 → 정책 검사 → `prepare-signpath.ps1 -RequireMetadata` → 전용 artifact upload를 수행한다. `actions/upload-artifact`가 만든 **artifact ID**를 요청에 전달한다.
4. `tools/installer/signpath/payload-artifact-configuration.xml` 초안을 서버 schema로 검토·등록한다. 정확한 Pod EXE 5개만 서명하며 ProductName과 요청 버전을 제한한다. upstream DLL/EXE에는 서명 지시를 추가하지 않는다.
5. `after-approval.steps.yml.example`은 활성 workflow가 아닌 삽입용 예시다. 승인 후 검토한 action commit을 고정하고 서버 설정/권한·토큰을 관리자가 준비한 뒤 같은 빌드 잡에 통합한다. `POD_VERSION`은 검토한 workspace 버전에서 읽는다. `SIGNPATH_API_TOKEN`은 submitter 권한만 부여하고 출력·artifact에 넣지 않는다.
6. 매 요청에 사람이 승인한 후 반환된 EXE 5개의 서명·타임스탬프를 검사한다. 반환 파일만으로 설치 프로그램 전체가 서명됐다고 보고하지 않는다.

### Inno 설치 프로그램의 남은 통합 지점

SignPath의 일반 `pe-file`은 내부를 재작성하는 Inno 전용 서명기가 아니다.
이미 완성한 unsigned 설치 EXE 바깥만 서명하면 내부 Pod 파일·제거 프로그램·
임시 setup 복사본 문제를 해결하지 못한다. 현재 예시는 **payload 단계**까지다.

Foundation과 다음 staged 경로를 합의한 뒤 설치 파이프라인을 연결해야 한다:

1. 같은 검토된 빌드에서 Inno `SignedUninstaller=yes`, SignTool 없이 생성하는 버전별 내부 setup/uninstaller 캐시를 수집하고 Pod payload와 함께 승인된 artifact 구성으로 제출한다. Inno의 별도 캐시 생성·재개 모드와 각 파일의 메타데이터 제한을 먼저 구현·시험해야 한다. 임의의 upstream 런타임 재서명 허용 여부를 추측하지 말고 프로젝트 설치 런타임 범위를 Foundation과 확인한다.
2. 서명된 내부 캐시와 Pod 파일을 검증한 뒤 **재빌드하지 않고** 동일 stage에 반영하고, Microsoft ConPTY/OpenConsole 해시·서명을 보존해 Inno를 재개한다.
3. 완성된 설치 EXE를 별도 GitHub artifact로 올려 최종 서명 요청·수동 승인을 받는다. 설치 파일·임시 실행 구성 요소·실제 설치된 제거 프로그램을 검증한다.
4. 서명된 stage로 포터블 ZIP을 만들고 최종 설치 EXE·ZIP의 체크섬을 다시 만든다. unsigned 체크섬을 재사용하지 않는다. SAC 켠 시험 PC의 새 설치·실행·업그레이드·제거까지 인수한다.

이 staged Inno 경로는 **설계/통합 대기**이며 현재 build.ps1에 SignPath 계정이나
지원되지 않는 동기 서명 provider를 가장해 넣지 않았다. 승인 전부터 운영하지
않는다. 기존 CertificateStore/ArtifactSigning의 동기 Inno 콜백은 선택 가능한
일반 서명 경로로 보존했으며, OSS 신청의 전제는 아니다.

## 승인 후 문구와 배지

현재 README에는 지원 중이라는 문구나 배지를 넣지 않는다. 승인·정책 확정·
실제 서명 검증 후에만 당시 조건에서 요구하는 지원 문구와 SignPath.io /
SignPath Foundation 링크를 Code signing policy 및 릴리스 페이지에 활성화한다.
그 전에는 “신청 준비 / 미승인” 상태를 유지한다.

## 현재 결과

로컬 서명/입력 준비 검사 **29개 통과**, Inno 서명·비서명 분기는 `/O-`로
실제 설치 파일 출력을 끄고 컴파일 검증했다. 정책 검사·문서 링크·PowerShell
문법·수동 unsigned workflow와 비활성 연동 예시를 정적 검사했고, payload XML은
SignPath 공식 XSD로 검증했다. 실제 외부 CI/서비스 서명 검증과는 구분한다.

로컬 실제 Pod 빌드는 rustc 실행 오류 4551로 차단됐고 **설치 EXE/ZIP은 없다**.
SignPath 신청·승인·서명 요청도 없으며 unsigned 외부 CI 역시 실행하지 않았다.
다음 단계는 관리자의 역할/MFA/연락처 확인, 외부 unsigned 빌드와 시험·공개
릴리스, 신청 제출 및 심사, 승인 후 위 staged Inno 통합과 SAC 인수다.
