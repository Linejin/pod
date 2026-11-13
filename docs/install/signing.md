# Windows 설치 EXE와 공인 서명 준비

공개 OSS의 우선 경로는 [SignPath Foundation 신청 준비](signpath.md)다.
미신청·미승인 상태이며 아래 공인 CA/Azure 경로는 선택 가능한 일반 기능으로
보존한다. OSS 신청용 unsigned 빌드에는 이 자격이 필요하지 않다.

`tools/installer/build.ps1`은 Pod 설치 EXE와 포터블 ZIP을 만든다. 서명 자격이
없는 기본 실행은 **서명 없는 개발 빌드**다. `-SigningConfig`를 주면 개발
이름표도 서명할 수 있다. 정식 버전 `-Release`에는 서명 설정 또는 명시적인
`-Unsigned`가 필요하다. 후자는 신청용 unsigned 후보이며 SAC용이 아니다.
현재 PC의 Smart App Control 설정을 변경하지 않고, Rust 도구 실행이 허용된
별도 Windows x64 빌드 호스트에서 빌드·서명한 결과물을 가져오는 경로다.

## 신뢰 요건

Smart App Control(SAC)은 신뢰된 발급자의 인증서로 서명한 앱을 허용하는
경로를 제공한다. 자체 서명, 로컬 루트 인증서 추가, 관리자 실행은 이 배포
요건을 대신하지 않는다. SHA256SUMS도 파일 동일성을 확인하는 값이며 코드
신뢰를 부여하지 않는다. 서명 검증 성공만으로 SAC 실기기 시험을 대신할 수
없고, SmartScreen의 평판 판단도 별개다.
근거: [Microsoft SAC 서명 안내](https://learn.microsoft.com/en-us/windows/apps/develop/smart-app-control/code-signing-for-smart-app-control),
[SAC 개요](https://learn.microsoft.com/en-us/windows/apps/develop/smart-app-control/overview).

일반 공인 서명 경로를 선택할 경우 다음 중 하나를 **빌드 호스트에** 준비한다. 이 작업은 구매·계정 생성·권한
부여·비밀 업로드를 수행하지 않는다.

| 경로 | 준비할 항목 |
|---|---|
| 공인 CA 인증서 | Microsoft Trusted Root Program에 연결되는 코드 서명 인증서, 해당 CA의 토큰/HSM 드라이버와 키 접근 권한, `My` 저장소의 정확한 thumbprint, RFC 3161 타임스탬프 URL |
| Microsoft Artifact Signing | Azure 구독·Entra ID, 신원 확인 완료 계정, **Public Trust** 인증서 프로필, 계정 지역에 맞는 endpoint, 해당 프로필의 Certificate Profile Signer 역할, 빌드 호스트 인증 |

Artifact Signing의 Private Trust/Test 프로필은 공개 SAC 배포용으로 사용하지
않는다. 서비스 이용 가능 국가·개인/조직 요건은 신청 전에 확인해야 한다.
[공식 설정 절차](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart),
[역할](https://learn.microsoft.com/en-us/azure/artifact-signing/concept-resources-roles),
[신뢰 모델](https://learn.microsoft.com/en-us/azure/artifact-signing/concept-trust-models).

## 외부 Windows x64 호스트에서 실행

Windows PowerShell **5.1**, Git, Visual Studio C++ MSVC 도구, Windows SDK의
x64 SignTool, Inno Setup **6.7.3**, Rust **1.100.0** MSVC를 준비한다.
`notices.ps1`은 .NET Framework를 사용하므로 `pwsh`로 build.ps1을 실행하지
않는다. SignTool은 최신 설치 SDK의 x64 경로를 자동 탐색한다. 필요하면
서명 JSON의 `SignTool`에 절대 경로를 지정한다.

준비한 소스를 새 체크아웃에 놓고 다음 명령을 실행한다. 빌드 출력 디렉터리를
다른 checkout과 공유하지 않는다. 서명 전 실행 파일을 SAC PC로 가져와 실행할
필요는 없다. 아래는 실행 절차이며 이 세션에서 외부 호스트를 만들거나
워크플로를 실행했다는 뜻이 아니다.

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
rustup toolchain install 1.100.0 --profile minimal
rustup override set 1.100.0
cargo install cargo-deny --version 0.20.2 --locked
powershell -NoProfile -File tools/fetch-conpty.ps1
```

각 native 명령의 종료 코드가 0인지 확인한다. ConPTY는 저장소에 고정된
NuGet 버전과 SHA256을 검증해 가져온다. `vendor/conpty`의 원본 Microsoft
서명은 변경하지 않는다.

### 공인 인증서 저장소

`tools/installer/signing-certificate.example.json`을 저장소 밖에 복사해
thumbprint, 저장소 위치(`CurrentUser` 또는 `LocalMachine`), CA가 안내한
타임스탬프 URL을 채운다. 인증서는 `My` 저장소에서 찾는다. 서명키는
CA의 토큰/HSM 공급자를 통해 접근한다. PFX와 암호를 JSON이나 명령줄에
넣는 인터페이스는 제공하지 않는다.

### Artifact Signing

Microsoft의 [SignTool 연동 문서](https://learn.microsoft.com/en-us/azure/artifact-signing/how-to-signing-integrations)를
따라 클라이언트를 준비한다. 이 워크플로는 `Microsoft.ArtifactSigning.Client`
**1.0.151**을 고정한다. 패키지의 `bin/x64` 전체를 유지하고
`Azure.CodeSigning.Dlib.dll`과 x64 SignTool의 아키텍처를 맞춘다.
이 버전의 실제 `Azure.CodeSigning.Dlib.runtimeconfig.json`은 **.NET 10**을
요구하므로 x64 .NET 10 런타임을 준비한다. 스크립트는 이 파일에서 필요한
런타임 버전을 읽어 검사한다.

`tools/installer/artifact-metadata.example.json`의 endpoint·계정·프로필을
채우고, `signing-artifact.example.json`의 `DlibPath`, `MetadataPath`를 설정한다.
파일 경로는 JSON 파일 기준 상대 경로 또는 절대 경로다. JSON에는 비밀을
넣지 않는다. Azure 인증은 빌드 호스트에서 별도로 제공한다. `ExcludeCredentials`로
실제로 사용할 인증 방식만 남길 수 있다. 로컬 대화형 로그인은 Inno 자식
프로세스에서 하지 않고 빌드 전에 완료한다.

### 진단과 패키징

```powershell
# 사전 진단: 빌드, 업로드, 서명 요청을 하지 않는다.
powershell -NoProfile -File tools/installer/build.ps1 -CheckPrerequisites -SigningConfig C:\signing-config\signing.json

# 개발 이름표를 쓰되 실제 공인 서명으로 패키징한다.
powershell -NoProfile -File tools/installer/build.ps1 -SigningConfig C:\signing-config\signing.json

# 정식 릴리스: 준비된 커밋에 v<workspace version> 태그가 있을 때만.
powershell -NoProfile -File tools/installer/build.ps1 -Release -SigningConfig C:\signing-config\signing.json
```

사전 진단은 설정·파일·런타임과 로컬 인증서 접근 가능 여부를 확인한다.
Artifact Signing의 실제 Azure 로그인 유효성과 역할은 서비스의 서명 요청 때
확인된다. 운영자는 지정한 프로필이 Public Trust인지 별도로 확인해야 한다.
정적 JSON 검사만으로 클라우드 권한을 확인했다고 보고하지 않는다.
인증서 저장소 경로는 체인·폐기·EKU·유효기간·
개인키 유무를 검사하지만, 조직이 추가한 사설 루트와 공인 Root Program의
차이는 발급자 요건으로 별도 확인해야 한다. 토큰 PIN/키 작업 권한 역시
실제 서명 시 확인된다.

서명 빌드에는 `-SkipBuild`, `-SkipDeny`를 허용하지 않는다. 기본 개발
명령에서 `-SigningConfig`를 생략하면 서명 없는 패키지를 만들 수 있지만
SAC용 배포본으로 취급하지 않는다. `-PrintVersion`은 도구 실행 없이 이름표만
출력한다. `-Release`의 태그·깨끗한 tracked 파일 검사는 그대로 적용한다.

## 서명과 산출물 순서

1. cargo deny와 `cargo build --release --locked`를 실행하고 라이선스 고지문을 만든다.
2. stage에 복사한 `pod-app.exe`, `podd.exe`, `pod-holder.exe`, `pod.exe`, `pod-hook.exe`를 SHA256 + RFC 3161 SHA256 타임스탬프로 서명한다. 빌드 원본은 변경하지 않는다.
3. stage의 **모든 EXE/DLL**을 검사한다. 필수 7개 파일의 누락도 거부하며, `conpty.dll`과 `x64/OpenConsole.exe`는 Microsoft 발급자 서명을 확인한다.
4. Inno의 `SignTool=PodReleaseSign`, `SignedUninstaller=yes`로 최종 설치 EXE, 제거 프로그램, 설치 중 사용하는 임시 self-copy까지 컴파일 안에서 서명한다. 최종 EXE만 사후 서명하는 경로를 사용하지 않는다.
5. 콜백은 각 서명 직후 검증한다. Inno가 임시 파일을 삭제하기 전에 동일 해시의 검증 사본을 `dist/signing/<build-id>/verified-copies`에 보관한다. 최종 단계에서 이 사본들과 설치 EXE, stage를 다시 검사한다.
6. 서명된 stage로 ZIP을 만들고 **마지막에** 설치 EXE·ZIP·서명 보고서의 SHA256SUMS를 쓴다. 패키징 시작 후 실패하면 새 성공 체크섬을 남기지 않는다.

검사는 `signtool verify /pa /all /v /tw`와 `Get-AuthenticodeSignature`를
함께 사용한다. 오류 및 경고 종료 코드, 유효하지 않은 신뢰 체인, 누락된
타임스탬프, 자체 서명, catalog-only 서명은 실패다.
[SignTool 옵션](https://learn.microsoft.com/en-us/windows/win32/seccrypto/signtool),
[Inno SignTool](https://jrsoftware.org/ishelp/topic_setup_signtool.htm),
[SignedUninstaller](https://jrsoftware.org/ishelp/topic_setup_signeduninstaller.htm).

배포 후보는 `dist/Pod-Setup-<label>-x64.exe`,
`dist/Pod-<label>-x64-portable.zip`, `dist/Pod-<label>-signatures.json`,
`dist/SHA256SUMS.txt`다. `stage`와 `signing` 전체는 배포하지 않는다.
보고서는 검증한 각 파일의 SHA256, 발급자, 인증서 thumbprint, 타임스탬프
발급자를 담는다. 서명과 타임스탬프 때문에 빌드마다 바이트가 달라질 수 있으며
비트 단위 재현성을 약속하지 않는다.

## 준비된 수동 CI (OSS unsigned)

`.github/workflows/windows-installer.yml`은 `workflow_dispatch`만 제공한다.
push·태그 생성에 자동 반응하지 않으며 GitHub Release를 게시하지 않는다.
Rust 1.100.0, cargo-deny 0.20.2, Inno 6.7.3을 고정하고 Inno 다운로드의
SHA256도 확인한다. 서명 계정·Azure·인증서·비밀 설정 없이 unsigned EXE·ZIP과
SignPath 신청용 payload를 준비한다. 승인 후 연동 예시는 활성 workflow와
분리되어 있다. [OSS 신청 절차](signpath.md)를 따른다.

이 파일은 로컬 준비 상태이며 push·실행하지 않았다. 기존 CI의 업로드 권한
제약도 그대로다([CONTRIBUTING](../../CONTRIBUTING.md)).

## 검증과 SAC 실기기 인수

```powershell
# 공인 자격 없이 가능한 보안 실패 경로 검사. 컴파일러는 /O-로 산출을 막는다.
powershell -NoProfile -File tools/installer/test-signing.ps1 -Iscc 'C:\path\to\ISCC.exe'

# 설치 EXE 및 압축 해제한 포터블 폴더 검사 (이 명령은 앱을 실행하지 않는다).
powershell -NoProfile -File tools/installer/sign.ps1 -Action Verify -Path C:\downloads\Pod-Setup-<label>-x64.exe
powershell -NoProfile -File tools/installer/sign.ps1 -Action Payload -Path C:\downloads\Pod-<label>

# 설치 후 제거 프로그램을 포함해 검사한다.
powershell -NoProfile -File tools/installer/sign.ps1 -Action Installed -Path "$env:LOCALAPPDATA\Programs\Pod"
```

실제 서명 빌드가 확보되면 SAC가 켜진 별도 시험 PC에서 다운로드한 설치 EXE,
새 설치·앱 실행·daemon/holder/ConPTY 터미널·훅 복사본·업그레이드·제거와
포터블 실행을 확인한다. 훅 설치를 선택했다면 `%LOCALAPPDATA%\Pod\bin\pod-hook.exe`도
`-Action Verify`로 확인한다. 시험 중 사용자의 에이전트 설정은 별도 시험 계정으로
격리한다. 기존 `test-install.ps1`은 서명/SAC 인수 시험을 대신하지 않는다.
차단 시 Windows CodeIntegrity/Operational 로그를 수집하며, 정책이나
레지스트리 변경·SAC 끄기·관리자 실행으로 우회하지 않는다.

2026-11-13 이 worktree의 실제 `cargo build --release --locked` 시도는
`rustc.exe -vV` 실행 전 Windows 오류 **4551**로 차단됐다. 고정 ConPTY
패키지 해시와 Microsoft 서명·타임스탬프는 검증됐다. **Pod 설치 EXE/ZIP은
생성되지 않았고**, 실제 서명·설치·SAC 실행은 미검증이다. 우선 외부 unsigned
빌드·릴리스와 OSS 신청 조건을 준비하고, SignPath 승인 후 관리 파이프라인과
SAC 실기기 인수를 진행한다. 일반 서명 자격은 선택 경로다. 별도 worktree는 작업을
격리할 뿐 Windows 실행 차단을 해결하지 않는다.
