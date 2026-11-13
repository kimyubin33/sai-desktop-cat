// 이 팀이 쓰는 Firebase 프로젝트(saigajoa)의 연결 정보.
// 설정에서 다른 firebaseConfig 를 붙여 넣으면 그 값이 우선한다.
// apiKey 는 비밀번호가 아니라 프로젝트를 가리키는 공개 값이며, 데이터 보호는 firestore.rules 가 맡는다.
module.exports = {
  // Google Cloud 콘솔의 "데스크톱 앱" OAuth 클라이언트 (캘린더·할 일 연결용).
  // 데스크톱 앱의 보안 비밀번호는 Google 도 비밀로 취급하지 않는 값이다. 아직 없으면 null — 설정에서 넣을 수 있다.
  BUILTIN_GOOGLE: null,
  BUILTIN_FIREBASE: {
    apiKey: 'AIzaSyC6MBRsYOCchzBRZT7hX2cvbxUujGG_Eq0',
    projectId: 'saigajoa',
  },
};
