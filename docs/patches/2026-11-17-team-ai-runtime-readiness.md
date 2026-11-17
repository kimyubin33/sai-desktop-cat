# 공용 Claude 런타임·배포 준비 점검 — 2026-11-17

실제 Google 파일·협업·Calendar 기본 흐름 확인 후, 공용 Claude 연결의 다음 단계를 진행했다. 최초 점검 당시 개발 환경에는 `SAI_TEAM_AI_ENDPOINT`, 운영용 Claude 키·Cloudflare 계정/토큰·팀 정책·HMAC 설정이 없었다. 사용자는 당시 기존 Claude 중계 주소와 Cloudflare 운영 계정이 **아직 둘 다 없다**고 확인했다. 이후 아래와 같이 계정 준비를 진행했다. 비밀값을 출력하거나 기존 개인 AI 키를 꺼내지 않았다.

## 수정한 문제

Worker의 Google·Claude fetch가 `redirect: "error"`를 사용했다. Node 모의 검사는 이 옵션을 받아들였지만, 실제 workerd는 요청을 만들 때 TypeError를 발생시켰다. 로컬 Worker에서 상태 조회가 `google_unavailable`로 끝나고 외부 요청이 0회인 것을 재현했다. [Cloudflare Request 문서](https://developers.cloudflare.com/workers/runtime-apis/request/)도 이 제한을 명시한다.

두 경로를 `redirect: "manual"`로 바꾸고, 기존의 성공 상태 확인으로 3xx를 거부하게 했다. Google 토큰·Claude 키를 리디렉션 목적지로 전달하지 않는다. Google 302와 Claude 307을 실제 Worker에서 반환해 다른 원점으로 요청하지 않는 검사도 추가했다. 앱의 Google 로그인·Calendar 연결 코드는 바꾸지 않았다.

## 검사와 운영 도구

- `services/team-ai/runtime.test.mjs`: 실제 `worker.mjs`와 SQLite Durable Objects를 사용하는 로컬 런타임 검사 5개. 동시 중복 요청·팀/서비스 한도·재시작 후 기록·응답을 버린 뒤 재시도·공유 취소·리디렉션을 검사한다. 영속 저장 파일이 실제 SQLite 형식이며 시험 원문·키·토큰·계정 원문을 포함하지 않는지도 확인했다.
- 모든 Google·Claude 응답은 로컬 가짜 응답이다. 자동 `cf` 조회·텔레메트리·개발 레지스트리를 끄고 외부 fetch를 가로챈다. 실제 계정이나 과금 호출을 사용하지 않는다.
- 운영 도구는 서비스 폴더의 별도 패키지·잠금 파일로 고정했다. Wrangler 4.164.0의 의존 버전에 맞춰 Miniflare 5.20261117.0-beta와 workerd 1.20261117.1을 사용한다. 이 ARM64 Windows PC에서는 공식 배포 체크섬을 대조한 Node 24.14.1 x64와 x64 workerd로 실행했다. 데스크톱 설치 파일에 도구를 추가하지 않았다.
- `check-deployment.mjs`: 실제 서비스로 접속하지 않고 현재 환경과 공개 설정을 검사한다. 키·계정·팀 ID·정책 원문을 출력하지 않으며, 설정 형식 통과와 실제 인증·모델 접근을 구분한다.
- `wrangler.jsonc`: 필수 Secret 이름 3개를 선언했다. 기본 공개 주소는 계속 꺼져 있고 라우트나 실제 계정을 임의 지정하지 않았다. `.env`도 Git 제외 범위에 추가했다.
- CI에 운영 도구의 잠금 파일 설치와 로컬 런타임 검사를 연결했다. GitHub Actions 실행 자체를 확인한 것은 아니다.

최종 Node 검사 **36개**를 통과했다. 중계 모의 검사 17개, 배포 사전 점검 4개, Worker/SQLite 런타임 5개, 기존 Google·미리보기 빌드 설정 10개다. Wrangler `deploy --dry-run`으로 Worker 묶음과 Durable Object 바인딩도 확인했다. 이 명령은 배포하지 않는다. Rust·Wasm·Windows 앱 소스는 바꾸지 않아 전체 앱 빌드·테스트를 반복하지 않았다.

현재 환경의 오프라인 점검은 공개 주소·계정·라우트·팀 정책/HMAC·Claude 키가 없어 **배포 설정 미완료**를 반환했다. 종료 코드 1은 이 미설정 상태의 정상 결과다. 비식별 보고서는 `target/team-ai-deployment-check.json`, 로컬 묶음은 `target/team-ai-worker-bundle/`에 있다.

## 실제 연결에 남은 사항

사용자는 후속 단계에서 Cloudflare 가입과 웹사이트 로그인을 완료했다고 확인했다. 그러나 Wrangler의 조회 전용 `whoami --json` 검사는 아직 인증되지 않은 상태를 반환했다. 웹사이트 로그인 완료를 배포 도구의 OAuth 연결 완료로 처리하지 않았다.

Windows 보호 저장 모듈이 없어 첫 연결 시도가 인증창을 열기 전에 중단됐다. Wrangler가 지정한 `@napi-rs/keyring@1.3.0`을 공식 CLI의 사용자별 보조 모듈 위치에 설치한 후, 보호 저장을 강제하고 계정 조회·Workers 스크립트 관리 범위만으로 연결을 다시 시작했다. 이 시점에는 계정 소유자의 직접 승인을 기다렸고 서비스 배포·Secret 등록·과금 호출을 실행하지 않았다. 이후 기기 인증과 승인받은 초기 중계 생성은 [Cloudflare 연결 기록](2026-11-17-team-ai-cloudflare-connect.md)에 별도로 기록했다. OAuth URL·토큰·키 원문은 결과에 출력하지 않았다.

운영 위치·주소, 비용 책임자·모델·한도, 허용할 팀과 소유자, 서비스 Secret 등록이 필요하다. [운영 안내](../../services/team-ai/README.md)를 따른다. 기존 개인 키를 서비스에 자동 복사하지 않고, 실제 배포나 과금 호출도 실행하지 않았다.

설정된 중계가 준비되면 앱의 전달 범위 동의 → 상태 조회 → 합성 파일 1개의 승인된 분석 → 후보 근거 확인 → 선택한 할 일 저장을 실계정으로 검사한다. 일반 팀원은 Google 로그인과 팀 권한으로 사용하며 서버 주소·공용 키를 입력하지 않는다. 현재 앱의 팀 Claude는 계속 준비 중이며 이 패치를 실제 AI 연결 완료로 표시하지 않는다.
