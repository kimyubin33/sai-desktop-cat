// 실제 Firebase 프로젝트(src/main/firebase-config.js)가 사이에 맞게 설정됐는지 점검한다.
// 실행: npm run check:firebase
//  1) 익명 로그인 사용 설정  2) Firestore 데이터베이스  3) 보안 규칙 게시  4) 전체 동작·권한 검사
// 4번은 시험용 팀("팀", "안동소주팀")과 작업 몇 개를 실제로 만듭니다. 앱 사용에는 영향이 없습니다.
const { spawnSync } = require('child_process');
const path = require('path');
const { BUILTIN_FIREBASE: cfg } = require('../src/main/firebase-config');
const { FirebaseBackend } = require('../src/core/team/firebase-backend');

const ok = (m) => console.log(`  ✓ ${m}`);
const fail = (m, fix) => {
  console.log(`  ✗ ${m}`);
  if (fix) console.log(`    → ${fix}`);
  process.exit(1);
};

(async () => {
  console.log(`\n사이 Firebase 점검 — 프로젝트: ${cfg.projectId}\n`);

  // 1) 익명 로그인
  const b = new FirebaseBackend({ config: cfg });
  let uid;
  try {
    uid = await b.signIn();
    ok('익명 로그인');
  } catch (e) {
    const m = String(e.message);
    if (/ADMIN_ONLY_OPERATION|OPERATION_NOT_ALLOWED/.test(m))
      fail('익명 로그인이 꺼져 있어요.', 'Firebase 콘솔 → Authentication → 로그인 방법 → 익명 → 사용 설정');
    if (/CONFIGURATION_NOT_FOUND/.test(m))
      fail('Authentication 이 아직 시작되지 않았어요.', 'Firebase 콘솔 → Authentication → 시작하기 → 익명 사용 설정');
    fail(`로그인 실패: ${m}`, '인터넷 연결과 src/main/firebase-config.js 의 apiKey·projectId 를 확인해 주세요.');
  }

  // 2) Firestore 존재 + 3) 규칙: 참여 코드 조회(로그인 사용자 허용)와 팀 목록 조회(항상 거부)
  // 없는 문서와 없는 데이터베이스가 둘 다 404 라서 응답 내용을 직접 본다
  const res = await fetch(`${b.docsBase}/joinCodes/CHECK0`, { headers: { authorization: `Bearer ${await b.token()}` } });
  const body = await res.text();
  if (res.status === 404 && /database.*does not exist|not exist for project/i.test(body))
    fail('Firestore 데이터베이스가 없어요.', 'Firebase 콘솔 → Firestore Database → 데이터베이스 만들기 (서울, 프로덕션 모드)');
  if (res.status === 403 && /has not been used|disabled/i.test(body))
    fail('Cloud Firestore API 가 꺼져 있어요.', 'Firebase 콘솔 → Firestore Database 를 한 번 열어 데이터베이스를 만들어 주세요.');
  if (res.status === 403)
    fail('보안 규칙이 사이 규칙이 아니에요 (기본 규칙은 모든 접근을 막아요).', 'Firestore → 규칙 탭에 저장소의 firestore.rules 를 붙여 넣고 게시');
  if (res.status !== 404 && !res.ok) fail(`Firestore 접근 실패 (${res.status}): ${body.slice(0, 200)}`);
  ok('Firestore 데이터베이스');
  try {
    await b.listDocs('teams');
    fail('보안 규칙이 너무 열려 있어요 (아무나 팀 목록을 볼 수 있음).', 'Firestore → 규칙 탭에 저장소의 firestore.rules 를 붙여 넣고 게시');
  } catch (e) {
    if (e.status !== 403) fail(`규칙 확인 실패: ${e.message}`);
    ok('보안 규칙 게시됨');
  }

  // 4) 에뮬레이터용과 같은 동작·권한 검사를 실제 프로젝트에서
  console.log('\n  동작·권한 검사 (팀 만들기, 참여, 주제 신호, 보류, 팀 약속, 협업, 권한 거부) …\n');
  // Node 22+ 는 --test 뒤의 경로를 패턴으로 읽어서, Windows 의 \ 경로는 파일을 못 찾는다.
  // 저장소 폴더에서 / 로 쓴 상대 경로로 실행하고, TAP 형식으로 결과를 받는다.
  const r = spawnSync(process.execPath, ['--test', '--test-reporter=tap', 'test/backend-firebase.emulator.js'], {
    cwd: path.join(__dirname, '..'),
    env: { ...process.env, SAI_FIREBASE_REAL: '1' },
    encoding: 'utf8',
  });
  const out = (r.stdout || '') + (r.stderr || '');
  let passed = 0;
  let failed = 0;
  for (const line of out.split('\n')) {
    const m = /^(ok|not ok) \d+ - (.*)$/.exec(line.trim());
    if (!m) continue;
    if (m[1] === 'ok') passed++;
    else failed++;
    console.log(`  ${m[1] === 'ok' ? '✓' : '✗'} ${m[2]}`);
  }
  // 검사가 하나도 돌지 않았다면 통과로 보지 않는다
  if (passed + failed === 0) {
    console.log('\n  ✗ 동작·권한 검사가 실행되지 않았어요. 아래 내용을 Claude 에게 그대로 보내 주세요.\n');
    console.log(`  node ${process.version}, 종료 코드 ${r.status}${r.error ? `, ${r.error.message}` : ''}`);
    console.log(out.slice(0, 2000));
    process.exit(1);
  }
  console.log(`\n  ${passed}개 통과, ${failed}개 실패`);
  if (r.status !== 0 || failed) {
    console.log('\n  일부 검사가 실패했어요. 아래 내용을 Claude 에게 그대로 보내 주세요.\n');
    console.log(out.split('\n').filter((l) => /not ok|error|expected|actual|Firestore|실패/.test(l)).slice(0, 40).join('\n'));
    process.exit(1);
  }
  console.log('\n모두 통과했어요. 팀 연결을 쓸 준비가 됐어요.\n');
})();
