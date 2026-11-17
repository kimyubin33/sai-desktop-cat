# config/

팀 Claude 중계의 공개 HTTPS 원점은 운영자가 빌드 환경 변수 `SAI_TEAM_AI_ENDPOINT`로 제공합니다. 일반 사용자가 서버 주소를 입력하지 않습니다. 이 변수에 키를 넣지 않습니다. 미설정 빌드는 팀 Claude를 준비 중으로 표시합니다. 실제 배포·Secret 등록은 [운영 계약](../services/team-ai/README.md)을 따릅니다.

저장소에 올리지 않는 로컬 설정을 두는 곳입니다 (`*.json` 은 .gitignore 대상).

`google-client.json` — 운영자가 빌드에 포함하는 Google "데스크톱 앱" OAuth 클라이언트

```json
{ "clientId": "…apps.googleusercontent.com", "clientSecret": "GOCSPX-…" }
```

Google Cloud 콘솔에서 내려받은 JSON(`{"installed": {...}}`)을 그대로 이 이름으로 저장해도 됩니다. 일반 사용자는 앱에 연결 정보를 입력하지 않습니다. 배포 빌드는 GitHub Actions의 기존 `GOOGLE_CLIENT_JSON` 시크릿을 사용합니다.

빌드 전 `node scripts/check-google-client.mjs`로 이 파일의 형식을 확인합니다. 값·파일 내용은 출력하지 않습니다. 수동 **Windows 검증 설치 파일** 워크플로는 `GOOGLE_CLIENT_JSON`에서 이 파일을 만들고 검증한 뒤 빌드하며, 누락·잘못된 형식이면 설치 파일을 만들지 않습니다. 이 확인은 클라이언트의 등록·사용 가능 여부나 Google 공개 검증 상태를 확인하는 API 호출이 아닙니다. 전환 중에는 Actions 아티팩트만 만들고 정식 Releases를 발행하지 않습니다.

검증 설치 파일은 `npm run build:preview`로 만듭니다. 이 명령은 Google 형식 확인, 별도 앱 ID·이름·실행 파일 설정과 `preview` 빌드 기능을 함께 적용하고 결과의 CPU·체크섬을 확인합니다. 검증판은 기존 사이의 자료 폴더·시작 프로그램·업데이트를 공유하지 않습니다. 빌드 기능과 앱 ID가 섞인 실행 파일은 자료를 열기 전에 시작을 중단합니다. 설치 파일 이름만 바꾸는 것으로 이 분리를 대신하지 않습니다.

이 파일이나 포함된 연결 정보가 없는 빌드는 Google 연결을 준비 중으로 표시합니다. 개발 PC에서는 기존 설치의 OAuth 클라이언트 정보를 재사용할 수 있으나, 개인 로그인 토큰·API 키·팀 자료를 이 파일에 복사하지 않습니다. 웹 앱용 클라이언트 대신 데스크톱 앱용 클라이언트를 사용합니다. 콘솔의 공개·테스트 사용자·브랜드 검증 상태는 운영자가 확인해야 합니다.

2026-11-17 사용자가 승인한 방향에 따라 첫 로그인에서 `openid`, `email`, `drive.appdata`, `https://www.googleapis.com/auth/drive`를 함께 요청합니다. 저장된 계정 연결로 앱 안에서 파일·팀 폴더를 탐색하며, 정상적인 파일 작업마다 Google 동의창을 다시 열지 않습니다. 이전 최소 권한 연결은 앱의 **Google 연결 갱신**으로 한 번 갱신합니다. Calendar·Tasks 권한은 포함하지 않습니다.

운영자는 같은 Google 프로젝트의 **Drive API** 활성화, 데스크톱 OAuth 클라이언트·테스트 사용자·공개/검증 상태와 위 동의 범위를 준비해야 합니다. 전체 Drive는 Google의 제한된 권한 범위이므로 공개 배포에 필요한 검증·정책 요건을 운영자가 확인해야 합니다. 콘솔 설정이나 검증 상태는 이 코드 변경만으로 바뀌지 않습니다. 정상 파일 선택에 Picker API·웹 API 키·별도 브라우저 인증을 사용하지 않습니다. 사용자는 콘솔이나 서버를 설정하지 않습니다. 토큰 철회·만료 등으로 재로그인이 필요할 수 있습니다. [이번 연결·가져오기 수정](../docs/patches/2026-11-17-drive-import-session.md)을 참고하세요.

## 테스트 계정의 Google 403 오류

Google 로그인 화면에 **앱이 현재 테스트 중이며 개발자가 승인한 테스터만 액세스할 수 있음**, **403 / access_denied**가 함께 표시되면 개발자가 해당 OAuth 프로젝트의 테스트 사용자 허용 목록을 확인합니다. 운영 계정이 로그인됐다는 사실만으로 다른 테스트 계정이 허용되지는 않습니다.

1. OAuth 클라이언트를 만든 개발자 계정으로 [Google Auth Platform의 Audience](https://console.cloud.google.com/auth/audience)를 엽니다.
2. 상단에서 앱 빌드에 포함된 데스크톱 OAuth 클라이언트가 속한 프로젝트를 선택합니다.
3. **Test users → Add users**에서 사용할 테스트 계정 이메일을 추가하고 **Save**합니다.
4. 사이 앱에서 새 로그인 시도를 시작해 해당 계정으로 동의를 완료합니다. 이전 시도가 대기 중이면 앱 **설정 → 로그인 취소** 후 다시 시작합니다.

이 절차는 개발 테스트 계정의 접근 허용이다. 일반 배포를 위한 Google 앱 검증·권한 범위 검토는 별도 단계로 관리한다. 실제 콘솔 허용 목록을 읽거나 변경하지 않았다면 등록 완료로 표시하지 않는다. 근거: [Google의 OAuth 동의 화면·테스트 사용자 설정](https://developers.google.com/workspace/guides/configure-oauth-consent), [앱 공개 상태별 접근 조건](https://developers.google.com/identity/protocols/oauth2/production-readiness/overview).
