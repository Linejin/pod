# 모바일 M2 의존성 closure (F0b)

작성: 2026-11-13 · 기준: master `045c8ab`, 프로토콜 v22 · 카드 M2-F0b([23 §8](../23-mobile-m2.md#8-병렬-worktree-구현-카드)) · 기계 판독본 [sbom.json](sbom.json)

## 0. 결정과 사용자 확인 1건

1. **사용자 결정(2026-11-13): daemon TLS는 플랫폼 스택이다.** Windows는 SChannel(+CNG·CryptoAPI), Linux는 시스템 OpenSSL 3(동적 링크, 번들 없음). rustls·ring·aws-lc·RustCrypto TLS는 쓰지 않는다. rustls+ring 안(`Linejin/m2-f0`, rustls의 BSD-3-Clause `subtle` 예외 필요)은 병합하지 않고 방법만 참고했다.
2. **새 crate는 Linux 전용 6개**: openssl 0.10.82(Apache-2.0), openssl-sys 0.9.118(MIT), openssl-macros 0.1.1, foreign-types 0.3.2·foreign-types-shared 0.1.1, vcpkg 0.2.15(모두 MIT/Apache). Windows closure는 이미 lock에 있던 windows-sys 0.61.2 + windows-link 0.2.1뿐이다.
3. **사용자 확인 필요: unicode-ident 1.0.27의 `Unicode-3.0` 항.** openssl crate의 필수 proc-macro(openssl-macros → syn/proc-macro2)가 Linux **build host에서만** 쓴다. 배포물에 링크되지 않고 workspace 빌드(serde_derive 등)에 이미 있다. 배포 closure 예외는 0개이고, gate는 이 crate 버전 하나를 host 전용 pin으로 기록한다. 거부하면 openssl crate 대신 openssl-sys 직접 FFI로 바꿔야 하며, 이 PC에서 검증할 수 없는 unsafe가 크게 늘어난다.

## 1. 구현 선택

| 후보 | 판정 |
|---|---|
| native-tls / tokio-native-tls | 불충분. 버전 상수가 TLS 1.2까지이고 server ALPN·협상 suite 조회·cipher 제한이 없다. server identity가 PKCS#12라 개인키 내보내기가 필요하다. |
| schannel crate 0.1.30 (MIT, lock에 있음) | 불충분. `SCH_CREDENTIALS`의 `CRYPTO_SETTINGS`(CBC 차단)를 넘길 수 없고 context handle이 비공개라 협상 version/suite를 조회할 수 없다. |
| **windows-sys로 SSPI·CNG 직접** | 채택. unsafe는 `crates/pod-mobile/src/sys/windows/`에 한정하고 loopback 시험으로 검증했다. |
| **Linux: openssl crate**(safe wrapper) | 채택. openssl-sys 직접 FFI보다 검증 안 된 unsafe가 적다. 런타임은 미검증(§5). |

`pod-mobile`의 `tls` 모듈(정책·`TlsAcceptor`/`TlsConnection` trait·SPKI 핀)은 feature 없이도 빌드되어 T1 double이 쓸 수 있다. 플랫폼 구현(`ServerConfig`, `PinnedClient`, `crypto`, `identity`)은 `mobile-lan`에서만 들어온다.

## 2. TLS 정책

- `VersionPolicy::platform_default()`: Windows build ≥ 20348(Server 2022, Windows 11) 또는 OpenSSL 3이면 **TLS 1.3만**. Windows 10은 TLS 1.3 server가 없어 `Tls12Aead`이며, TLS 1.2는 ECDHE-ECDSA + AES-GCM/ChaCha20(`0xC02B`, `0xC02C`, `0xCCA9`)만 허용한다.
- Windows: `SCH_CREDENTIALS` + `TLS_PARAMETERS`(허용 version 외 비활성, `CRYPTO_SETTINGS`로 AES-CBC chaining·RSA/DH 키 교환 차단), `SCH_USE_STRONG_CRYPTO`, `SCH_CRED_DISABLE_RECONNECTS`(재개 없음), ALPN `http/1.1`, TLS 1.2 renegotiation 거부, TLS 1.3 post-handshake 처리, close_notify.
- Linux: min/max version, TLS 1.3 suite, TLS 1.2 AEAD cipher list, `NO_TICKET`·`NO_COMPRESSION`·`NO_RENEGOTIATION`, session cache off, ticket 0, early data 0, ALPN 선택, OpenSSL 3 미만 거부.
- 공통: handshake 뒤 `SessionInfo::check`가 version·suite·ALPN을 다시 검사하고 실패하면 닫는다. 협상 version(`SessionInfo.version`, `TLS1.3`/`TLS1.2`)은 P1/T2가 pairing 기록과 PC UI에 남긴다. 신뢰는 CA가 아닌 QR SPKI 핀이 운반한다.

## 3. 키·인증서·암호 (새 Rust 암호 crate 없음)

| 항목 | Windows | Linux (미검증) |
|---|---|---|
| listener 키 | 현재 사용자 Microsoft Software KSP의 영구·내보내기 불가·서명 전용 P-256 키(사용자 DPAPI 보호). 이름 `<prefix><32 hex>`. SChannel server는 이름으로 키를 여는 영구 키를 쓰는 것으로 알려져 있어 영구 키를 택했다(ephemeral 키는 시험하지 않음). | OpenSSL P-256, PKCS#8 PEM 0600, 디렉터리 0700·link 거부 |
| 인증서 | `CertCreateSelfSignCertificate`: SAN IP 1개, EKU serverAuth, CA=false·KU digitalSignature(둘 다 critical), ECDSA-SHA256, 무작위 serial, notBefore = 발급−5분, 90일. 인증서 저장소에 넣지 않고 DER은 호출자가 보관하며 SChannel에는 `CERT_KEY_PROV_INFO`로 키 이름을 붙인다. | X509 builder, 같은 확장, 127비트 무작위 serial |
| SPKI 핀 | `BCryptHash` SHA-256(DER SPKI) | OpenSSL `sha256` |
| 기기 서명 검증 | `CryptDecodeObjectEx` + `CryptImportPublicKeyInfoEx2` + `BCryptVerifySignature` | EVP `Verifier` |
| 256비트 random | `BCryptGenRandom`(system preferred RNG) | `RAND_bytes` |

- 검증 입력은 Android 형식이다: DER SubjectPublicKeyInfo(`PublicKey.getEncoded()`)와 DER `ECDSA-Sig-Value`(`SHA256withECDSA`). 재인코딩이 입력과 같아야 하는 strict DER, uncompressed named-curve P-256만 받는다.
- 회전: 새 identity 생성 → 호출자가 새 generation 저장 → 옛 키 `KeyStore::delete`. 충돌 뒤 남은 키는 `KeyStore::keys()`로 찾는다. generation 기록·원자 저장·DACL은 P1 몫이다.
- `SoftwareSigner`는 저장하지 않는 메모리 P-256 키로, 시험과 desktop 기기 double용이다.

## 4. gate (`tools/check-mobile-licenses.ps1`)

- 대상별로 `shipped`(배포물에 링크, `cargo tree -e normal,no-proc-macro`), `host`(build script·proc-macro), `dev`를 나눈다.
- Apache-2.0·MIT·ISC만. OR는 원문을 동봉한 허용 분기를 고르고 AND는 모두 필요, WITH는 실패. 배포 crate 예외는 없고 host 전용 항은 crate 버전마다 이유와 함께 고정한다(§0-3 하나).
- rustls 계열·ring·aws-lc·subtle·RustCrypto·openssl-src(번들 OpenSSL)가 어디든 나타나면 실패한다. Windows는 windows-sys를 싣고 openssl이 없어야 하며, Linux는 openssl + openssl-sys를 싣는다. 기능 off 빌드에는 TLS crate가 없어야 한다.
- 배포 crate의 native 파일을 헤더로 판정한다. openssl-sys의 `build/expando.c`는 build.rs가 C 전처리만 한다(제품에 컴파일되지 않음).
- `sbom.json`(19 crate: shipped 9, host 10)과 다시 만든 결과가 다르면 실패한다. 검토 후 `-Write`로만 갱신한다. `-SelfTest` 17건이 금지/누락/OR·AND·WITH, 제외 crate, host pin 오용, 대상별 스택, off 누수, lock·원문 누락, 변조를 실패로 확인한다.
- `tools/check-boundaries.ps1`: pod-mobile → podd/pod-client/pod-app/pod-cli/pod-remote/GPUI/alacritty·Rust TLS crate 금지, pod-protocol·pod-client → pod-mobile·TLS crate 금지.

## 5. 재현과 증거 (2026-11-13, Windows 11 build 26200)

```powershell
powershell -ExecutionPolicy Bypass -File tools/check-mobile-licenses.ps1 -SelfTest   # 17 cases ok
powershell -ExecutionPolicy Bypass -File tools/check-mobile-licenses.ps1             # 19 crates, 2 targets
powershell -ExecutionPolicy Bypass -File tools/check-boundaries.ps1 -SelfTest        # 11 cases ok
cargo test -p pod-mobile --features mobile-lan --locked   # 27 tests: tls 5, crypto 7, identity 7, loopback 8
cargo clippy -p pod-mobile --features mobile-lan --all-targets --locked -- -D warnings
cargo check -p podd --features mobile-lan --locked
```

- loopback(127.0.0.1)만: 핀 handshake 후 양방향 데이터(70 KB 다중 record), close_notify, 틀린 핀은 application data 0, 회전된 키는 옛 핀 실패, TLS 1.2 fallback은 AEAD suite만, CBC 전용·TLS 1.2 전용 client·평문 HTTP 거부, handshake deadline.
- 인증서 모양·IPv6 SAN·재적재·회전·다른 키 인증서 거부·prefix 밖 이름 거부. 시험 키는 시험 prefix로 만들고 끝에 지운다(실행 후 남은 키 0개를 `certutil -user -key`로 확인).
- SHA-256 FIPS 예시, RFC 6979 A.2.5 P-256/SHA-256 두 벡터(.NET `ECDsa`로 교차 확인), 틀린 메시지·키·서명, 비정규 DER·압축점·P-384 표기·곡선 밖 점 거부.
- Linux: 이 PC에 Linux toolchain이 없어 컴파일하지 않았다. CI `linux` job이 check + clippy를 하고 test는 정보용(실패해도 job 통과)이다. **런타임 미검증.**

## 6. 남은 일

- §0-3 unicode-ident host pin의 사용자 확인.
- `deny.toml`의 `[graph]`가 Windows target만 보므로 Linux openssl closure의 RustSec 권고는 cargo-deny가 검사하지 않는다(이 카드 소유 밖).
- Linux 실기 런타임, 0700/0600·소유권 검증(R6a 이후). Windows 10의 TLS 1.2 경로는 Windows 11에서 TLS 1.2 전용 client로만 확인했다.
- Android client는 OS TLS + 앱 SPKI 핀이며 Rust closure 밖이다(M3 gate). qrcode·axum 등 [23 §7](../23-mobile-m2.md#7-의존성-license-gate-tm-04)의 남은 후보는 추가할 때 이 gate를 다시 통과해야 한다.
- 설치본 THIRD-PARTY-NOTICES는 `mobile-lan` 배포(V1/M3) 때 이 closure를 포함하는지 확인한다.
