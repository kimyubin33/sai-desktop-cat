// 개발용: SAI_SNAPSHOT_DIR 환경 변수가 있으면 시나리오를 실행하며 창을 캡처해 PNG로 저장하고 종료한다.
// 화면이 없는 환경(CI 등)에서 고양이와 대화창, 팀 연결 흐름이 제대로 그려지는지 확인할 때 쓴다.
//   SAI_SNAPSHOT_SCENARIO = basic(기본) | team-create | team-join | team-share | team-receive | team-review
const fs = require('fs');
const path = require('path');

function runSnapshots(app, ctx) {
  const dir = process.env.SAI_SNAPSHOT_DIR;
  if (!dir) return false;
  fs.mkdirSync(dir, { recursive: true });
  const scenario = process.env.SAI_SNAPSHOT_SCENARIO || 'basic';
  const steps = SCENARIOS[scenario](ctx);
  (async () => {
    for (const step of steps) {
      await new Promise((r) => setTimeout(r, step.wait || 800));
      try {
        if (step.action) await step.action();
        const win = step.window && step.window();
        if (step.capture !== false && win && !win.isDestroyed()) {
          await new Promise((r) => setTimeout(r, step.settle || 500));
          const img = await win.webContents.capturePage();
          fs.writeFileSync(path.join(dir, `${scenario}-${step.name}.png`), img.toPNG());
        }
      } catch (err) {
        console.error('snapshot failed', step.name, err);
      }
    }
    for (const s of Object.values(ctx.stores)) s.saveNow();
    app.exit(0);
  })();
  return true;
}

function helpers({ cat, chatWindow }) {
  const js = (code) => () => chatWindow.win.webContents.executeJavaScript(code);
  const force = (state) => () => {
    cat.hideBubble();
    cat.behavior.enter(state);
    cat.sendState(true);
  };
  const click = (sel) => js(`document.querySelector(${JSON.stringify(sel)}).click()`);
  const setVal = (sel, v) => js(`document.querySelector(${JSON.stringify(sel)}).value = ${JSON.stringify(v)}`);
  return { js, force, click, setVal, w: () => chatWindow.win, c: () => cat.win };
}

function complaintSteps(h, openChat) {
  return [
    { name: 'chat-start', wait: 300, action: () => openChat(), window: h.w, settle: 1500 },
    { name: 'chat-mode', wait: 300, action: h.js("__saiDemo.startMode('complaint')"), window: h.w },
    { name: 'chat-1', wait: 300, action: h.js("__saiDemo.send('시간도 안 지키고 맡은 일도 제대로 안 해서 너무 화나요. 진짜 무책임해요')"), window: h.w },
    { name: 'chat-2', wait: 300, action: h.js("__saiDemo.send('어제 같이 검토하기로 했는데 연락이 없었어요')"), window: h.w },
    { name: 'chat-3', wait: 300, action: h.js("__saiDemo.send('사정은 모르겠어요. 물어보진 않았어요')"), window: h.w },
    { name: 'chat-4', wait: 300, action: h.js("__saiDemo.send('제 다음 작업이 하루 밀렸어요')"), window: h.w },
    { name: 'chat-5', wait: 300, action: h.js("__saiDemo.send('미리 알려주기만 했어도 괜찮았을 것 같아요')"), window: h.w, settle: 800 },
    { name: 'chat-share', wait: 300, action: h.click('.card .share'), window: h.w, settle: 800 },
  ];
}

const SCENARIOS = {
  basic(ctx) {
    const h = helpers(ctx);
    return [
      ...['sit', 'walk', 'loaf', 'sleep', 'drag'].map((state) => ({ name: `cat-${state}`, wait: 1000, action: h.force(state), window: h.c })),
      { name: 'cat-greet', wait: 600, action: () => (h.force('sit')(), ctx.advisor.greet()), window: h.c },
      { name: 'cat-advice', wait: 600, action: () => (ctx.cat.hideBubble(), ctx.advisor.speakNow()), window: h.c },
      ...complaintSteps(h, ctx.openChat),
      { name: 'chat-saved', wait: 300, action: h.click('.share-panel .card-actions .primary'), window: h.w, settle: 800 },
      { name: 'notes', wait: 300, action: h.js("__saiDemo.showTab('notes')"), window: h.w },
      { name: 'settings', wait: 300, action: h.js("__saiDemo.showTab('settings')"), window: h.w },
    ];
  },

  // A: 팀 만들기
  'team-create'(ctx) {
    const h = helpers(ctx);
    return [
      { name: 'open', wait: 300, action: () => ctx.openChat('settings'), window: h.w, settle: 1500 },
      {
        name: 'created',
        wait: 300,
        action: async () => {
          await h.setVal('#displayName', '유빈')();
          await h.setVal('#teamName', 'BION 홍보팀')();
          await h.click('#teamCreate')();
        },
        window: h.w,
        settle: 1200,
      },
    ];
  },

  // B: 참여 코드로 참여
  'team-join'(ctx) {
    const h = helpers(ctx);
    const hub = JSON.parse(fs.readFileSync(process.env.SAI_TEAM_FILE, 'utf8'));
    const code = Object.keys(hub.joinCodes)[0];
    return [
      { name: 'open', wait: 300, action: () => ctx.openChat('settings'), window: h.w, settle: 1500 },
      {
        name: 'joined',
        wait: 300,
        action: async () => {
          await h.setVal('#displayName', '민지')();
          await h.setVal('#joinCode', code.toLowerCase())();
          await h.click('#teamJoinBtn')();
        },
        window: h.w,
        settle: 1200,
      },
    ];
  },

  // A: 고민 정리 → 민지에게 자동 반영, 팀 약속은 검토 후 공유
  'team-share'(ctx) {
    const h = helpers(ctx);
    return [
      ...complaintSteps(h, ctx.openChat),
      {
        name: 'pick-recipient',
        wait: 300,
        action: h.js(`
          for (const sel of document.querySelectorAll('.share-panel select')) {
            sel.value = sel.options[1].value; sel.dispatchEvent(new Event('change'));
          }
          const radios = document.querySelectorAll('.share-panel input[value="team"]');
          radios[radios.length - 1].click();
          const rv = document.querySelectorAll('.share-panel input[value="review"]');
          rv[rv.length - 1].click();
          document.querySelector('.share-panel .option').scrollIntoView();
        `),
        window: h.w,
      },
      { name: 'saved', wait: 300, action: h.click('.share-panel .card-actions .primary'), window: h.w, settle: 1500 },
      { name: 'notes', wait: 300, action: async () => { await ctx.team.syncNow(); await h.js("__saiDemo.showTab('notes')")(); }, window: h.w, settle: 1200 },
      { name: 'confirmed', wait: 300, action: async () => { await h.click('#outbox .item .controls .primary')(); await ctx.team.syncNow(); await h.js("__saiDemo.showTab('notes')")(); }, window: h.w, settle: 1200 },
    ];
  },

  // B: 받은 주제가 평소 조언에 섞이고, "내 사정도 있어요"로 이야기
  'team-receive'(ctx) {
    const h = helpers(ctx);
    const boosted = () => {
      const { LIBRARY } = require('../core/advice-library');
      const topic = Object.keys(ctx.team.sync.boosts())[0];
      return LIBRARY.find((e) => e.topic === topic);
    };
    return [
      { name: 'sync', wait: 500, action: () => ctx.team.syncNow(), capture: false },
      { name: 'proposal-bubble', wait: 500, action: () => (h.force('sit')(), ctx.advisor.maybeShowProposal()), window: h.c },
      { name: 'agree', wait: 300, action: () => { const b = ctx.cat.bubble; ctx.cat.hideBubble(); ctx.advisor.handleAction(b.id, 'agree'); }, capture: false },
      { name: 'advice-bubble', wait: 800, action: () => (h.force('sit')(), ctx.advisor.show(boosted())), window: h.c },
      {
        name: 'explain-start',
        wait: 300,
        action: () => { const b = ctx.cat.bubble; ctx.cat.hideBubble(); ctx.advisor.handleAction(b.id, 'mine'); },
        window: h.w,
        settle: 1800,
      },
      { name: 'explain-1', wait: 300, action: h.js("__saiDemo.send('그날 갑자기 실험 수업이 연장돼서 못 갔어요')"), window: h.w },
      { name: 'explain-2', wait: 300, action: h.js("__saiDemo.send('폰을 못 봐서 연락도 못 했어요. 일정이 확정된 줄도 몰랐어요')"), window: h.w },
      { name: 'explain-3', wait: 300, action: h.js("__saiDemo.send('다음엔 일정 잡을 때 참석 확인을 남기면 좋겠어요')"), window: h.w, settle: 800 },
      { name: 'explain-hold', wait: 300, action: h.js("document.querySelector('.card .card-actions:last-child button').click()"), window: h.w, settle: 1200 },
      { name: 'notes', wait: 300, action: async () => { await ctx.team.syncNow(); await h.js("__saiDemo.showTab('notes')")(); }, window: h.w, settle: 1200 },
    ];
  },

  // 4단계: A가 자기소개·프로젝트·작업 입력
  'collab-setup'(ctx) {
    const h = helpers(ctx);
    const c = ctx.team.collab;
    return [
      { name: 'sync', wait: 500, action: () => ctx.team.syncNow(), capture: false },
      { name: 'empty', wait: 300, action: () => ctx.openChat('collab'), window: h.w, settle: 1500 },
      {
        name: 'filled',
        wait: 300,
        action: async () => {
          await c.saveProfile({ pr: '영상 편집과 촬영을 해 봤어요', skills: [{ name: '영상편집', level: 3 }, { name: '촬영', level: 2 }], learn: '', weeklyHours: 10 });
          const pid = await c.createProject({ title: '학생회 홍보 영상', goal: '1분 홍보 영상과 포스터', deliverable: 'mp4, A2 포스터', deadline: '2026-11-28' });
          const shoot = await c.addTask(pid, { title: '촬영', estimateHours: 3, difficulty: 2, skills: '촬영', doneCriteria: '인터뷰 3명 촬영본', prep: '카메라 대여, 장소 섭외' });
          await c.addTask(pid, { title: '컷 편집', estimateHours: 5, difficulty: 3, skills: '영상편집', dependsOn: [shoot], doneCriteria: '1분 이내 편집본' });
          await c.addTask(pid, { title: '포스터 디자인', estimateHours: 4, difficulty: 2, skills: '디자인', doneCriteria: 'A2 인쇄용 PDF' });
          await c.addTask(pid, { title: '자막·소개 글', estimateHours: 2, difficulty: 1, skills: '글쓰기' });
          await ctx.team.syncNow();
          await h.js("__saiDemo.showTab('collab')")();
        },
        window: h.w,
        settle: 1500,
      },
    ];
  },

  // 4단계: B가 자기소개 입력
  'collab-profile'(ctx) {
    const h = helpers(ctx);
    return [
      { name: 'sync', wait: 500, action: () => ctx.team.syncNow(), capture: false },
      {
        name: 'profile',
        wait: 300,
        action: async () => {
          await ctx.team.collab.saveProfile({ pr: '포스터·카드뉴스 만들기를 좋아해요', skills: [{ name: '디자인', level: 3 }, { name: '글쓰기', level: 2 }], learn: '영상편집', weeklyHours: 6 });
          ctx.openChat('collab');
        },
        window: h.w,
        settle: 3500,
      },
    ];
  },

  // 4단계: A가 배분 제안을 받고 적용
  'collab-plan'(ctx) {
    const h = helpers(ctx);
    return [
      { name: 'open', wait: 500, action: () => ctx.openChat('collab'), capture: false, window: h.w, settle: 1500 },
      {
        name: 'plan',
        wait: 1500,
        action: h.js(`(async () => {
          [...document.querySelectorAll('.plan button')].find(b => b.textContent.includes('배분 제안'))?.click();
          await new Promise(r => setTimeout(r, 800));
          document.querySelectorAll('.plan-row').forEach(d => d.open = true);
          document.querySelector('.plan').scrollIntoView();
        })()`),
        window: h.w,
        settle: 1500,
      },
      {
        name: 'applied',
        wait: 300,
        action: h.js(`(async () => {
          [...document.querySelectorAll('.plan button')].find(b => b.textContent.includes('이대로'))?.click();
          await new Promise(r => setTimeout(r, 800));
          document.querySelectorAll('.block')[2]?.scrollIntoView();
        })()`),
        window: h.w,
        settle: 1500,
      },
    ];
  },

  // 4단계: B의 고양이가 배정 확인 → 수락 → 시작 안내
  'collab-nudge'(ctx) {
    const h = helpers(ctx);
    const show = () => {
      h.force('sit')();
      ctx.advisor.maybeShowProgress();
    };
    return [
      { name: 'sync', wait: 500, action: () => ctx.team.syncNow(), capture: false },
      { name: 'accept-bubble', wait: 300, action: show, window: h.c },
      {
        name: 'accepted',
        wait: 300,
        action: async () => {
          const b = ctx.cat.bubble;
          ctx.cat.hideBubble();
          ctx.advisor.handleAction(b.id, 'task-accept');
          await new Promise((r) => setTimeout(r, 1000));
          ctx.cat.hideBubble();
          // 두 번째 배정도 수락
          const n = ctx.team.collab.nudge();
          if (n && n.key.startsWith('accept:')) await ctx.team.collab.accept(n.taskId);
          ctx.team.collab.markNudged('plan-tight');
        },
        capture: false,
      },
      { name: 'next-bubble', wait: 500, action: show, window: h.c },
      { name: 'tab', wait: 300, action: () => (ctx.cat.hideBubble(), ctx.openChat('collab')), window: h.w, settle: 1800 },
    ];
  },

  // 5단계: 외부 연결 설정 화면
  'ext-settings'(ctx) {
    const h = helpers(ctx);
    return [
      {
        name: 'settings',
        wait: 800,
        action: async () => {
          await ctx.team.syncNow();
          ctx.openChat('settings');
        },
        window: h.w,
        settle: 2500,
      },
      { name: 'ext', wait: 300, action: h.js("document.getElementById('extBox').scrollIntoView()"), window: h.w, settle: 800 },
    ];
  },

  // 여러 팀: A 가 팀 두 개를 만든다
  'multi-create'(ctx) {
    const h = helpers(ctx);
    const create = (name) => async () => {
      await h.setVal('#displayName', '유빈')();
      await h.setVal('#teamName', name)();
      await h.click('#teamCreate')();
    };
    return [
      { name: 'open', wait: 300, action: () => ctx.openChat('settings'), capture: false, window: h.w, settle: 1500 },
      { name: 'first', wait: 1500, action: create('BION 홍보팀'), window: h.w, settle: 1500 },
      { name: 'second', wait: 300, action: create('안동소주 팀플'), window: h.w, settle: 1500 },
    ];
  },

  // 여러 팀: B 는 두 번째 팀에만 참여
  'multi-join'(ctx) {
    const h = helpers(ctx);
    const hub = JSON.parse(fs.readFileSync(process.env.SAI_TEAM_FILE, 'utf8'));
    const code = Object.entries(hub.joinCodes).find(([, v]) => v.teamName === '안동소주 팀플')[0];
    return [
      { name: 'open', wait: 300, action: () => ctx.openChat('settings'), capture: false, window: h.w, settle: 1500 },
      {
        name: 'joined',
        wait: 1500,
        action: async () => {
          await h.setVal('#displayName', '민지')();
          await h.setVal('#joinCode', code)();
          await h.click('#teamJoinBtn')();
        },
        window: h.w,
        settle: 1500,
      },
    ];
  },

  // 여러 팀: A 가 고민을 "안동소주 팀플"로 민지에게 공유 (보고 있는 팀은 BION 으로 바꿔 둔 상태에서)
  'multi-share'(ctx) {
    const h = helpers(ctx);
    return [
      {
        name: 'switch',
        wait: 500,
        action: async () => {
          const bion = ctx.team.teams().find((t) => t.name === 'BION 홍보팀');
          ctx.team.setActive(bion.teamId);
          await ctx.team.syncNow();
        },
        capture: false,
      },
      ...complaintSteps(h, ctx.openChat),
      {
        name: 'pick-team',
        wait: 300,
        action: h.js(`(async () => {
          const pick = document.querySelector('.share-panel select.team-pick');
          pick.value = [...pick.options].find(o => o.text.startsWith('안동소주')).value;
          pick.dispatchEvent(new Event('change'));
          for (const sel of document.querySelectorAll('.share-panel select.recipient')) {
            sel.value = sel.options[1].value; sel.dispatchEvent(new Event('change'));
          }
          pick.scrollIntoView();
        })()`),
        window: h.w,
        settle: 800,
      },
      { name: 'saved', wait: 300, action: h.click('.share-panel .card-actions .primary'), window: h.w, settle: 1500 },
      { name: 'notes', wait: 300, action: async () => { await ctx.team.syncNow(); await h.js("__saiDemo.showTab('notes')")(); }, window: h.w, settle: 1500 },
    ];
  },

  // 여러 팀: B 가 받은 주제 확인
  'multi-receive'(ctx) {
    const h = helpers(ctx);
    return [
      {
        name: 'check',
        wait: 500,
        action: async () => {
          await ctx.team.syncNow();
          console.log('MULTI-BOOSTS', JSON.stringify(ctx.team.hub.teamFacade().boosts()), JSON.stringify(ctx.team.teams().map((t) => t.name)));
        },
        capture: false,
      },
    ];
  },

  // A: 다시 동기화 → 보류됨 표시, 팀 약속 현황
  'team-review'(ctx) {
    const h = helpers(ctx);
    return [
      { name: 'notes', wait: 300, action: async () => { await ctx.team.syncNow(); ctx.openChat('notes'); }, window: h.w, settle: 2000 },
      { name: 'settings', wait: 300, action: h.js("__saiDemo.showTab('settings')"), window: h.w, settle: 1000 },
    ];
  },
};

module.exports = { runSnapshots };
