# config/

저장소에 올리지 않는 로컬 설정을 두는 곳입니다 (`*.json` 은 .gitignore 대상).

`google-client.json` — Google 캘린더·할 일 연결용 "데스크톱 앱" OAuth 클라이언트

```json
{ "clientId": "…apps.googleusercontent.com", "clientSecret": "GOCSPX-…" }
```

Google Cloud 콘솔에서 내려받은 JSON(`{"installed": {...}}`)을 그대로 이 이름으로 저장해도 됩니다.
이 파일이 있으면 앱이 자동으로 쓰고, 없으면 앱의 설정 → 외부 연결 → Google 에 붙여 넣으면 됩니다.
