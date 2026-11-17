# Cloudflare 연결과 비공개 초기 중계 — 2026-11-17

후속 상태는 [2026-11-18 Secret 확인·실검증 준비](2026-11-18-team-ai-secret-check.md)를 따른다. 아래 기록은 초기 생성 시점의 범위와 검사 결과다.

사용자가 Cloudflare 가입·웹사이트 로그인과 Claude API 키 보유를 확인했다. 웹사이트 로그인 직후 Wrangler는 미인증 상태였으므로 배포 도구 연결을 별도로 진행했다.

## 연결과 생성

Windows 보호 저장용 `@napi-rs/keyring@1.3.0`을 Wrangler의 사용자별 보조 모듈 위치에 설치했다. 기본 콜백 연결은 브라우저 오류로 사용자가 창을 닫았으며 정확한 원인은 확인되지 않았다. Cloudflare 공식 기기 인증으로 전환해 사용자가 직접 승인했다. CLI 성공과 `whoami --json`으로 계정 1개, `user:read`, `account:read`, `workers_scripts:write`, 갱신용 `offline_access` 범위를 확인했다. 보호 저장을 강제한 CLI 호출이 성공했고 암호화된 인증 파일의 존재를 메타데이터로 확인했다. 인증 파일이나 키 값을 읽거나 출력하지 않았다.

사용자는 **비공개 검증 중계 생성 허용**으로 Worker 1개·SQLite 바인딩·초기 정책/서명키 등록을 승인했다. 생성 직전 조회로 동명 Worker가 없음을 다시 확인했다. 초안의 이름·계정·외부 연결 설정을 검증한 뒤 `sai-team-ai`를 배포했고 CLI는 **No targets deployed**를 반환했다. `workers_dev`와 `preview_urls`는 모두 false다.

`AI_HMAC_KEY`와 `TEAM_AI_POLICY`를 등록하고 Secret 이름 목록으로 확인했다. 서명키는 암호학적 난수 32바이트에서 생성해 표준 입력으로만 보냈으며 명령 인자·파일·결과에 원문을 넣지 않았다. 기존 동명 Secret은 보존하도록 처리했다. 정책은 `version: 1`, `dailyRequests: 1`, `teams: {}`다.

배포 목록으로 중계의 존재를 다시 확인했다. 원격 예산 객체는 호출하지 않았다. 원격 SQLite 동작·실제 Google 권한·Claude 응답을 검증한 결과는 아니다. 계정 ID·계정별 설정은 Git에서 제외된 `target/team-ai-operator-account.json`과 `target/team-ai-operator/`에 보관한다. 이 문서에는 계정 ID·이메일·토큰·Secret 원문을 넣지 않는다. Claude 키는 소유자가 Cloudflare 보호 입력란에 직접 등록하도록 안내했다.

## 검사와 남은 범위

`check-deployment.mjs`에 `--config`로 계정별 초안을 지정하는 기능을 추가했다. 환경 변수의 계정 선택을 우선하고 없으면 초안의 `account_id`를 확인한다. 실제 계정 ID·설정 값·잘못된 경로·본문·인수를 출력하지 않는 검사도 추가했다.

관련 Node 검사 **38개**가 통과했다. 중계 모의 17개, 배포 사전 점검 6개, Worker/SQLite 런타임 5개, Google·미리보기 빌드 설정 10개다. 일반 초안과 초기 생성 초안의 `deploy --dry-run`도 통과했다. 데스크톱 코드·설치 파일은 바꾸지 않았다.

완료 범위는 **운영자 인증 → 비공개 초기 Worker·SQLite 바인딩 → 비활성 정책·서명키 등록**이다. 팀원 앱의 중계 주소는 아직 설정하지 않았고 허용 팀은 0개다. 공개 라우트·모델·예산·허용 팀과 소유자를 정하고, 승인받은 합성 자료 1회의 실제 분석·검토·할 일 저장을 확인하는 단계가 남아 있다. 이번 단계의 Claude 호출은 0회다.

참고: [Wrangler 기기 인증·보호 저장](https://developers.cloudflare.com/workers/wrangler/commands/general/), [Cloudflare Secret 등록](https://developers.cloudflare.com/workers/configuration/secrets/).
