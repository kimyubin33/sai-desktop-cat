# 팀 Claude 중계 — 운영과 검증

2026-11-17 사용자는 기존 Claude 연결을 이어 쓰기로 선택했다. 개발·운영 측의 중계 소스이며 일반 사용자 설치 프로그램이 아니다. 승인받아 외부 연결을 끈 초기 Worker·SQLite 바인딩과 허용 팀이 없는 정책·서명키 Secret을 만들었다. 2026-11-18 소유자 샘플 분석 1회와 별도 계정의 접근·회수 검증을 마친 뒤 비공개로 복귀했다. 이후 별도 승인으로 **앤드위브 AI 한 팀·서비스/팀 전체 하루 10회**의 상시 연결을 켜고 소유자 상태 조회·미허용 팀 차단·앱 표시를 확인했다. 이번 활성화의 추가 Claude 분석은 0회다. 다른 PC·역할·실제 팀원 분석 검증은 남아 있다. Firebase·사용자 PC 서버·공용 키 내려받기를 추가하지 않는다.

## 처리 범위

`worker.mjs`는 Cloudflare Workers 진입점이며 `TeamBudget`은 SQLite 기반 Durable Object를 사용한다.

| 경로 | 처리 |
|---|---|
| `POST /v1/status` | 현재 Google 계정·팀 권한 확인, 연결 설정·일일 시도 횟수 조회. Claude 호출 없음. |
| `POST /v1/analyze` | 명시적 텍스트 분석 후 요약·할 일 후보 반환. Drive·협업·Calendar 변경 없음. |

앱의 [파일·텍스트 검토 패치](../../docs/patches/2026-11-17-ai-review.md)가 분석 IPC와 파일 버튼을 연결한다. 전송 본문 확인과 사용자 실행 후에만 요청하며, 근거와 후보를 검토해 선택한 할 일만 기존 협업 저장 경로로 보낸다. 주소가 없는 빌드는 본문 확인까지만 가능하다. 현재 로컬 Windows x64 미리보기에는 기존 중계 주소가 들어 있으며 승인한 앤드위브 AI 한 팀의 연결을 활성화했다. 미허용 팀은 상태 조회에서도 차단한다. 팀장 셀프 등록·키 입력 화면은 이번 범위가 아니다.

## 신뢰 경계

앱은 빌드에 넣은 `SAI_TEAM_AI_ENDPOINT`의 HTTPS 원점만 사용하고 리디렉션을 따라가지 않는다. 첫 연결 때 **Drive 전체 접근 권한을 포함한 짧은 수명의 Google access token을 중계로 전달한다는 사실**을 안내하고 동의를 받는다. 동의 전에는 상태 조회도 중계로 전송하지 않는다. 동의는 이 PC의 계정·서비스 원점별로 저장한다. 일반 사용자가 주소를 입력하지 않는다.

중계는 고정 Google HTTPS 주소에서 UserInfo, Drive 사용자 permission ID, 실제 폴더의 사이 표시·소유자·조회 권한과 명시적 사용자 공유 항목을 직접 확인한다. 공개 링크·그룹·도메인 접근만으로 허용하지 않는다. 일반 Google 폴더·계정 지정 초대가 대상이며 조직 공유 드라이브는 지원하지 않는다. 소유자가 바뀌면 등록 정보와 달라져 거부한다. 사용자 입력의 계정 ID·관리자 표시를 권한으로 사용하지 않는다.

Google access token은 Google API에만, 서비스 Secret의 Claude 키는 Claude API에만 사용한다. Google 비밀번호·refresh token·개인 AI 대화는 받지 않는다. **운영자가 access token의 유효기간 동안 전체 Drive 권한을 사용할 수 있는 신뢰 경계는 남는다.** 선택 파일로 권한을 축소한 토큰이나 종단 간 암호화를 구현한 것이 아니며 운영자 접근 불가를 보장하지 않는다. 실제 선택 내용은 중계와 Anthropic이 처리한다.

[개인 Calendar 연결](../../docs/patches/2026-11-17-my-calendar.md)은 별도 동의·별도 보호 토큰을 사용한다. 앱은 Calendar 토큰을 중계에 보내지 않으며, 기본 Google 토큰의 반환 범위에 Calendar 권한이 섞인 경우에도 중계 요청을 거부한다. 별도 토큰 저장만으로 Google의 권한 결합 정책까지 보장하지 않는다.

핸들러는 원문·응답·토큰·키를 로그에 출력하지 않고 추적을 기본 비활성화한다. Cloudflare·Google·Anthropic의 처리와 인프라 로그까지 없다는 보장은 아니다.

## 한도·재시도·응답

- 허용 팀 최대 16개. 팀마다 실제 소유자 permission ID·고정 모델·일일 시도 수(1~100)·최대 출력 토큰(256~2048)을 운영 설정으로 지정한다. 전체 서비스 일일 시도 수는 1~500이다. 기본 허용 팀이나 실제 과금 모델은 넣지 않았다.
- Durable Object 트랜잭션으로 전체·팀 시도 수를 각각 예약한 뒤 Claude를 호출한다. 하나라도 거부되면 호출하지 않는다. 실패·응답 유실 때 예약을 환불하지 않는 보수적인 제한이다. 시도 수는 성공 수나 실제 청구액과 같지 않으며 UTC 기준 일일 구간을 사용한다.
- 입력은 UTF-8 24KiB, 출력은 팀의 토큰 상한으로 제한한다. 금액의 절대 상한은 아니므로 제공자 결제 한도와 운영 예산도 정해야 한다.
- 계정·팀·요청 번호의 해시와 본문의 HMAC 지문을 보관해 동시 요청·재시작·지문 키 교체 후 같은 시도를 거부한다. 원문과 결과는 저장하지 않는다. 메타데이터는 30일 후 정리한다. 생성 시각이 15분을 넘은 요청은 거부한다.
- 응답을 잃으면 서버에서 다시 받을 수 없다. 같은 번호는 거부하며 새 분석은 사용자의 명시적 실행과 새 비용이 될 수 있다. 자동 재호출하지 않는다.
- JSON 응답의 12개 이하 후보·원문에 실제 포함된 근거·날짜를 검사한다. 잘린 결과·모델 거부·임의 실행 필드는 성공으로 표시하지 않는다. 분석 후에도 권한을 다시 확인한다. 문서 속 명령을 실행할 도구는 제공하지 않는다.

## 운영자가 정할 것 — 미완료

운영 중인 중계가 없을 때는 운영자 한 명이 아래 순서로 준비한다. 일반 팀원에게 이 설정을 요구하지 않는다.

1. [Cloudflare 공식 시작 안내](https://developers.cloudflare.com/workers/get-started/guide/)의 계정 가입부터 진행한다. 가입·로그인은 계정 소유자가 직접 한다. 이 저장소에 Worker 프로젝트와 도구가 이미 있으므로 안내의 새 프로젝트 생성 명령을 다시 실행할 필요는 없다.
2. 계정 준비 후 개발 도구의 Cloudflare 로그인을 연결하고 사용할 계정을 확인한다. 웹사이트 로그인과 Wrangler 연결은 별도다. 이 중계에서 쓰는 OAuth 범위는 `account:read user:read workers_scripts:write`이며 계정 소유자가 브라우저에서 직접 동의한다. 로컬 콜백 연결이 어려우면 공식 `wrangler login --device`를 사용한다. Windows에서는 `--use-keyring`과 `CLOUDFLARE_AUTH_USE_KEYRING=true`로 보호 저장을 요청한다. Wrangler 4.164.0의 Windows 보조 모듈은 `@napi-rs/keyring@1.3.0`이다. 비대화 실행에서 모듈이 없으면 인증 전에 중단되므로 먼저 공식 CLI의 보조 모듈 설치를 마친다. 토큰 원문을 출력하는 명령은 사용하지 않는다. 공개 주소는 `workers.dev` 기본 주소 또는 보유한 사용자 도메인 중 정한다. 계정 준비를 완료했다고 배포나 결제까지 승인한 것으로 간주하지 않는다.
3. [Claude API 시작 안내](https://platform.claude.com/docs/en/get-started)에 따라 사용 가능한 API 계정·운영용 키를 확인한다. 기존 개인 키의 소유·재사용 가능 여부는 별도로 확인하고 자동으로 추출하지 않는다. 모델·예산·사용 한도와 배포 내용을 정한 뒤 실제 서비스 Secret 등록과 검증 호출을 진행한다.

다음 운영 항목은 계정 준비 뒤 확정한다.

1. Cloudflare 계정·공개 주소·개인정보 처리 지역/정책, 예산과 Claude 모델·가격·결제 한도.
2. 서비스 Secret에 `ANTHROPIC_API_KEY`, 임의의 긴 `AI_HMAC_KEY`, `TEAM_AI_POLICY` 등록. 개인 PC의 기존 Claude 키를 자동 추출·복사하지 않는다. 키를 저장소·명령 인자·Drive·팀원 PC에 넣지 않는다.
3. 정책 형식은 `version: 1`, `dailyRequests`, `teams`(팀 폴더 ID → `ownerPermissionId`, `model`, `dailyRequests`, `maxOutputTokens`)다. 실제 소유자와 비용 책임자가 승인한 팀만 운영자가 등록한다.
4. Worker 도메인·Durable Object 바인딩/마이그레이션, 요청 속도 제한·인프라 예산·최소 로그 설정. `workers_dev: false`는 주소를 정하기 전 기본 공개를 막는다.
5. 확인한 HTTPS 원점을 빌드 환경의 `SAI_TEAM_AI_ENDPOINT`에 넣어 앱을 빌드한다. GitHub Actions에서는 같은 이름의 저장소 변수를 사용한다. 이 변수는 공개 주소 전용이며 비밀 키를 넣지 않는다.
6. 실제 두 Google 계정으로 권한 목록 조회 가능 범위, 명시적 공유·공유 취소·소유자 변경·동시 한도·실제 Claude 호출·응답 유실을 확인한다. 아래 로컬 workerd·SQLite 검사는 배포된 Cloudflare 환경이나 Google·Claude 실서비스 검증을 대신하지 않는다.

실제 배포·키 이동·과금 호출은 위 결정과 승인 뒤 진행한다. 파일/텍스트 선택부터 검토한 항목의 협업 저장까지 앱 경로는 연결했으나, 실서비스 검증을 완료한 상태는 아니다.

### 초기 Worker 생성과 키 등록

[Cloudflare 연결 기록](../../docs/patches/2026-11-17-team-ai-cloudflare-connect.md)의 승인 범위는 초기 Worker 1개·SQLite 바인딩·초기 정책과 서명키 Secret이다. `workers_dev: false`, `preview_urls: false`, 허용 팀 0개로 시작했다. 앱의 공개 연결이나 유료 분석까지 승인한 것으로 확장하지 않는다.

계정별 초안은 Git에서 제외된 `target/team-ai-operator/`에 보관한다. `wrangler.bootstrap.jsonc`는 최초 생성 전용으로 외부 연결을 끄고 필수 Secret 선언을 생략한다. 일반 배포 설정의 필수 Secret 3개 선언은 유지한다. 초기 서명키는 메모리에서 생성해 Wrangler 표준 입력으로만 보냈으며 기존 Secret은 덮어쓰지 않는다. 초기 정책은 `version: 1`, `dailyRequests: 1`, `teams: {}`다.

Claude 키는 소유자가 해당 Worker의 **Settings → Variables and Secrets → Add → Secret**에 `ANTHROPIC_API_KEY`라는 이름으로 입력하고 **Deploy**한다. 일반 Text 변수를 사용하거나 대화·저장소에 키를 전달하지 않는다. 등록 결과는 Secret 이름 목록으로만 확인한다. [Cloudflare Secrets 안내](https://developers.cloudflare.com/workers/configuration/secrets/)를 참고한다.

[Secret 확인·1회 실검증 계획](../../docs/patches/2026-11-18-team-ai-secret-check.md)에서 세 이름의 존재와 검증 팀의 실제 소유자를 확인했다. 개발용 읽기 도구는 앱 설정을 수정하지 않으며 결과는 로컬 운영 초안에만 저장한다. Sonnet 5.5의 도구 없는 추출 요청 형식을 보완했다. 이후 승인된 [실검증](../../docs/patches/2026-11-18-team-ai-live-verification.md)에서 첫 권한 조회의 빈 페이지 토큰 문제를 재현·수정했고, 저장된 연결 동의를 재사용해 실제 분석 1회가 성공했다. 결과는 팀이나 Calendar에 저장하지 않았다. 빈 정책·비공개 배포와 공개 주소의 404, 필수 Secret 보존을 확인했다. 승인된 유료 검증 1회는 사용 완료됐으며 상시 사용을 켜지 않았다.

### 팀원 권한만 확인하는 검증

[팀원 접근·회수 검증](../../docs/patches/2026-11-18-team-member-access-check.md)에서 별도 Google 계정의 초대 전·후·회수 후 접근을 비교했다. `access-check-worker.mjs`는 상태 조회만 기존 권한 검사에 위임하며, 분석은 외부 요청·예산 예약 전에 차단한다. 실계정의 초대 전 404, 임시 초대 후 파일 200·해시 일치와 중계 200 / `owner: false`를 확인했다. 같은 Google 로그인을 유지하고 공유 권한만 회수했을 때 폴더·파일 404와 중계 403을 확인했다. 그 뒤 신규 임시 권한 회수·소유자만 남은 공유 상태·빈 정책·일반 진입점의 비공개 배포·외부 404·Secret 보존을 확인했다. 이번 추가 Claude 호출은 0회이며 다른 PC·역할·팀원 실제 분석·상시 운영 전체를 완료한 결과는 아니다.

### 첫 운영 활성화

[첫 운영 기록](../../docs/patches/2026-11-18-team-ai-normal-operation.md)에 사용자 선택인 **앤드위브 AI 한 팀·서비스/팀 전체 하루 10회**와 기존 Sonnet 5.5·출력 1,024토큰을 반영했다. 실제 소유자·초안 지문을 다시 확인하고, 구체적인 상시 운영 승인 후 정책과 공개 연결을 적용했다. 기존 동의의 상태 조회는 200·소유자·한도 10, 미허용 검증 팀은 403이었다. 반복 조회에서 사용 횟수 0을 유지했고 앱에서도 0 / 10회를 확인했다. 추가 분석 없는 읽기 도구 검사 4개와 필수 Secret 이름 보존을 확인했다. API 키·서명키·사용 기록은 유지했으며 이번 추가 유료 분석·협업 저장·일정 등록은 0회다.

## 개발 검사

루트의 `npm run test:team-ai`는 Node 기본 테스트 도구와 모의 fetch·트랜잭션 저장소, 배포 설정 검사 25개를 실행한다. npm 패키지 설치·실제 키·외부 API 접속이 필요하지 않다. Rust `google::team_ai` 검사는 루프백 HTTP와 가짜 계정만 사용한다.

추가 로컬 런타임 검사는 실제 `worker.mjs` 진입점과 SQLite Durable Objects를 workerd에서 실행한다. 운영 도구는 이 디렉터리의 별도 `package.json`·잠금 파일로 관리하며 데스크톱 빌드나 사용자 설치 파일에 포함하지 않는다.

```powershell
npm.cmd ci --prefix services/team-ai --no-fund --no-audit
npm.cmd run test:team-ai:runtime
npm.cmd --prefix services/team-ai run build:check
```

Node 22.12 이상이 필요하다. Windows ARM64 개발 PC에서는 x64 Node와 x64 workerd로 검사한다. 이번 검증은 Node 24.14.1 x64·Wrangler 4.164.0과 해당 Wrangler가 사용하는 Miniflare 5.20261117.0-beta·workerd 1.20261117.1을 고정해 실행했다. Miniflare의 명시적 v4 옵션 변환기를 사용한다. 관련 도구를 일반 팀원에게 설치하도록 요구하지 않는다.

런타임 검사 8개는 동시 중복·팀/서비스 한도, 프로세스 재시작 후 시도 기록, 결과를 버린 요청의 재시도 차단, 처리 전후 공유 취소, Google·Claude 리디렉션 차단, Sonnet 5.5의 제한된 텍스트 추출 설정, Google 권한 목록 페이지 토큰과 권한 검증 전용 진입점의 분석 차단을 확인한다. 외부 fetch는 전부 가짜 응답으로 가로채며 `cf` 자동 조회·텔레메트리·개발 레지스트리도 끈다. 실제 키·사용자 파일·유료 호출을 사용하지 않는다. 자체 생성한 `target/team-ai-runtime-*` 저장소만 경로 확인 후 제거한다.

Workers에서는 `redirect: "error"`가 요청 전 TypeError를 발생시켜 기존 구현의 Google 조회가 항상 실패했다. `manual`로 바꾸고 3xx를 실패로 처리해 자격 증명을 다른 주소로 전달하지 않도록 수정했다. 이 문제는 일반 Node 모의 검사에서 드러나지 않았고 로컬 workerd에서 재현했다. [검증 기록](../../docs/patches/2026-11-17-team-ai-runtime-readiness.md)에 범위를 남겼다.

## 운영 설정 사전 점검

```powershell
npm.cmd run check:team-ai
npm.cmd run check:team-ai -- --config target/team-ai-operator/wrangler.jsonc
```

이 명령은 현재 프로세스 환경과 공개 `wrangler.jsonc` 또는 명시한 계정별 초안을 확인하고, 설정이 부족하면 종료 코드 1을 반환한다. 계정은 환경 변수 선택을 우선하며 없으면 초안의 `account_id`를 사용한다. HTTPS 원점·공개 라우트 일치, Cloudflare 계정 선택, SQLite 바인딩·마이그레이션, 필수 Secret 선언, 팀 정책·HMAC·Claude 키 형식을 검사한다. 출력은 항목별 성공 여부·팀 수·시도 한도이며 주소·계정 ID·키·정책 원문을 출력하지 않는다. 잘못된 파일·인수도 원문 없이 실패한다. 네트워크 요청·배포·설정 변경은 하지 않는다.

`.dev.vars`·`.env`·기존 데스크톱 키는 자동으로 읽거나 옮기지 않는다. 실제 서비스의 Secret 등록 여부, Cloudflare 인증, Claude 키의 유효성·모델 접근·요금은 별도로 확인해야 한다. `localConfigurationValid`는 실서비스 연결 성공을 뜻하지 않는다. `wrangler.jsonc`에는 필수 Secret 이름을 선언했고 기본 공개 주소는 계속 꺼 두었다. `.dev.vars`·`.env`·`.wrangler`는 Git에서 제외한다.

PowerShell에서 npm 스크립트 실행이 차단된 환경은 `npm.cmd run test:team-ai` 또는 `node --test services/team-ai/gateway.test.mjs`를 사용한다. Windows 실행 정책을 변경하지 않는다.

근거: [Google permissions.list](https://developers.google.com/workspace/drive/api/reference/rest/v3/permissions/list), [Claude Messages API](https://platform.claude.com/docs/en/api/messages/create), [Claude 인증](https://platform.claude.com/docs/en/manage-claude/authentication), [API 2026-10-21](https://platform.claude.com/docs/en/api/migration-2026-10-21), [Durable Object 트랜잭션](https://developers.cloudflare.com/durable-objects/best-practices/rules-of-durable-objects/), [Workers Request·리디렉션](https://developers.cloudflare.com/workers/runtime-apis/request/), [Wrangler 설정·필수 Secrets](https://developers.cloudflare.com/workers/wrangler/configuration/), [Miniflare](https://developers.cloudflare.com/workers/testing/miniflare/).
