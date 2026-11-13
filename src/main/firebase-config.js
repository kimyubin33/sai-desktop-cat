// 이 팀이 쓰는 Firebase 프로젝트(saigajoa)의 연결 정보.
// 설정에서 다른 firebaseConfig 를 붙여 넣으면 그 값이 우선한다.
// apiKey 는 비밀번호가 아니라 프로젝트를 가리키는 공개 값이며, 데이터 보호는 firestore.rules 가 맡는다.
const fs = require('fs');
const path = require('path');

function readLocalGoogleClient() {
  try {
    const j = JSON.parse(fs.readFileSync(path.join(__dirname, '..', '..', 'config', 'google-client.json'), 'utf8'));
    const c = j.installed || j;
    const clientId = c.clientId || c.client_id;
    const clientSecret = c.clientSecret || c.client_secret;
    return clientId && clientSecret ? { clientId, clientSecret } : null;
  } catch {
    return null;
  }
}

module.exports = {
  // Google Cloud 콘솔의 "데스크톱 앱" OAuth 클라이언트 (캘린더·할 일 연결용).
  // GitHub 의 비밀 정보 차단 때문에 저장소에는 넣지 않는다. 다음 중 하나로 넣는다.
  //   1) 앱의 설정 → 외부 연결 → Google 에 붙여 넣기 (팀원 각자)
  //   2) config/google-client.json 파일 (저장소에 올라가지 않음, 설치 파일을 만들 때 함께 포함)
  BUILTIN_GOOGLE: readLocalGoogleClient(),
  BUILTIN_FIREBASE: {
    apiKey: 'AIzaSyC6MBRsYOCchzBRZT7hX2cvbxUujGG_Eq0',
    projectId: 'saigajoa',
  },
};
