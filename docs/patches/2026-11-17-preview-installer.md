# Windows 독립 미리보기 패키지 — 2026-11-17

## 이번 변경

이전 검증 빌드는 출력 파일명만 `Sai-Preview-*`로 바꿨다. 설치된 앱의 이름·식별자·실행 파일과 자동 업데이트는 기존 사이의 설정을 사용했으므로 검증판이 기존 앱과 겹칠 수 있었다. **사이 미리보기**를 독립된 앱으로 빌드하도록 수정했다.

| 항목 | 검증판 |
|---|---|
| 표시 이름·시작 메뉴 | 사이 미리보기 |
| 앱 식별자 | `com.kimyubin33.sai.preview` |
| 설치 실행 파일 | `sai-preview.exe` |
| 기기 자료·WebView 저장 | 위 앱 식별자의 별도 Windows 공간. 기존 사이 자료를 복사하지 않음 |
| 시작 프로그램·정식 업데이트 | 등록·다운로드·설치하지 않음 |
| 실행·종료 | 파일 화면을 열고 창을 닫으면 종료. 이번 검증판에는 바탕화면 고양이 자동 실행 없음 |
| 기존 제거 훅 | 정식 앱의 `Sai` 시작 프로그램 값을 지우던 훅을 포함하지 않음 |

`preview` 빌드 기능과 별도 Tauri 설정을 함께 적용한다. 빌드 기능·식별자·이름·실행 파일 설정이 섞이면 자료 저장소를 열기 전에 시작을 중단한다. 설치판과 포터블판은 같은 미리보기 계정 공간을 사용하며, 프로그램이 켜져 있으면 기존 미리보기 창을 연다. 현재 사용자 설치 방식은 유지한다.

Google 로그인은 검증판에서도 사용자가 직접 한다. 같은 OAuth 앱·Google 계정의 원격 이름·팀·설정은 로그인 후 복원하지만 이전 PC의 로컬 토큰·개인 AI 키·대화는 자동 복사하지 않는다. 팀 Claude 중계는 여전히 미배포이며 이번 패키지에서 공용 키를 배포하지 않는다.

## 빌드 방법

로컬과 GitHub Actions 모두 `scripts/build-preview.mjs`를 사용한다. Windows에서 `npm ci` 후 `npm run build:preview`를 실행한다. ARM Windows에서 x64 개발 도구로 빌드할 때는 다음과 같이 명시한다.

```powershell
npm.cmd ci
npm.cmd run build:preview -- --target x86_64-pc-windows-msvc --ui-host-target x86_64-pc-windows-msvc
```

PowerShell이 `npm.ps1` 실행을 막는 환경에서는 위와 같이 `npm.cmd`를 사용한다. 시스템 실행 정책을 변경할 필요는 없다.

명령은 개발 측 Google 연결 정보의 형식 확인 → Wasm → `preview` 기능을 켠 릴리스·NSIS 빌드 → 앱 PE/CPU·설치 파일 확인 → 파일 복사·SHA-256 생성 순서로 진행한다. 입력 검증이나 빌드가 실패하면 배포용 사본 생성으로 넘어가지 않는다. 다른 CPU 실행 파일·없는 설치 파일·손상된 헤더를 성공으로 처리하지 않는다. NSIS 설치 프로그램의 x86 부트스트랩과 실제 앱의 x64/ARM64를 구분한다.

결과는 `target/preview-dist/win-x64` 또는 `win-arm64`의 설치판·포터블판·체크섬·빌드 정보·간단한 사용 안내다. 옛 서버 ZIP·개인 설정·로그·토큰 파일을 이 폴더로 모으지 않는다. GitHub 워크플로는 수동 Actions 아티팩트만 만들고 Releases를 발행하지 않는다. 일반 `npm run build`는 기존 제품 설정으로 빌드하므로 현재 검증 배포에 사용하지 않는다.

## 검증

- `cargo test --locked -p sai --target x86_64-pc-windows-msvc`: **6개 통과**. 새 검사 2개는 정식/미리보기 식별자 혼용과 이름·실행 파일 충돌을 거부함을 확인한다.
- `node --test scripts/build-preview.test.mjs scripts/check-google-client.test.mjs`: **7개 통과**. 새 검사 4개는 PE 헤더/오프셋·CPU/설치 파일 누락·두 CPU의 원본 바이트와 체크섬/파일 선별·입력 경로와 대상 제한을 확인한다. 모의 실행 파일을 사용한 단위 검사다.
- 이번 관련 검사 **13개 통과, 그중 6개 신규**다. 직전 전체 검사 222개의 핵심·Google·중계 코드를 바꾸지 않아 전체 회귀 검사를 다시 실행하지 않았다.
- Wasm과 Windows x64 릴리스·NSIS 빌드 성공. 실제 앱의 CPU는 x64이며 제품 메타데이터는 **사이 미리보기**다. 배포 사본과 ZIP의 SHA-256을 대조했다. 코드 서명 상태는 `NotSigned`다.
- 생성된 NSIS 스크립트의 이름·앱 ID·실행 파일이 검증판 값이고, 기존 `installer-hooks.nsh`와 정식 앱의 시작 프로그램 제거 코드가 포함되지 않음을 확인했다.
- 별도 환경 변수 없이 생성한 포터블판을 실행했다. 기존에 없던 미리보기 전용 Roaming/Local 자료 폴더가 생성되고, **사이 미리보기** 창·프로세스가 유지되며 오류 로그는 0바이트다. 기존 정식 앱의 `Run/Sai`와 `StartupApproved/Run/Sai`는 실행 전후 동일한 권한 환경에서 지문을 비교해 보존됨을 확인했다. 값 원문은 기록하지 않았다.
- 새 실행 파일의 화면 조회는 컴퓨터 사용 도구에서 **`Computer Use app approval timed out`**로 끝났다. 따라서 초기 화면의 요소·설정 안내·버튼 동작·창 닫기 종료는 이번 실행에서 확인했다고 표시하지 않는다. 다른 UI 자동화로 승인 절차를 우회하지 않았다.

### 로컬 결과 파일

아래 크기는 이 첫 패키지 생성 당시 기록이다. 이후 같은 경로의 파일을 갱신했으며 현재 크기·설치 시험 결과는 [후속 검증](2026-11-17-install-restart.md)을 따른다.

| 파일 | 크기 |
|---|---:|
| `target/preview-dist/win-x64/Sai-Preview-Setup-0.9.14.exe` | 3,502,977바이트 |
| `target/preview-dist/win-x64/Sai-Preview-Portable-0.9.14.exe` | 12,022,784바이트 |
| `target/Sai-Preview-win-x64.zip` | 7,927,284바이트 |

ZIP에는 위 설치판·포터블판, `SHA256SUMS.txt`, `preview-build.json`, `먼저 읽어주세요.txt`만 담았다. ZIP 자체의 체크섬은 `target/Sai-Preview-win-x64.zip.sha256`이다. 이 파일들은 로컬 생성물이며 GitHub에 업로드하거나 정식 배포하지 않았다. 사용자는 ZIP을 풀어 Setup으로 설치하거나 Portable을 실행한 뒤 Google 로그인으로 시험할 수 있다. 현재 빌드 정보의 `teamAiConfigured`는 `false`다.

## 아직 확인하지 않은 범위

실제 설치·제거·업데이트, ARM64 앱 빌드, 새 PC의 WebView2 설치, 실제 Google 계정·두 PC의 복원/권한 철회, 팀 Claude 서비스의 설정·배포·유료 분석은 별도 검증이다. 코드 서명 인증서를 추가하지 않았다. 설치와 Google 동의는 사용자가 직접 수행하며 인증·보안 경고를 자동 조작하지 않는다. 기존 자료를 삭제하거나 정식 배포를 실행하지 않았다.

설정 근거: [Tauri의 별도 빌드 설정 병합](https://v2.tauri.app/reference/cli/), [앱 식별자·실행 파일·NSIS 설치 설정](https://v2.tauri.app/reference/config/).
