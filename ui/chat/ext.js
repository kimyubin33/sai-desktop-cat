// 설정 → 외부 연결: Telegram 알림, Google 캘린더·할 일. 각 연결의 실제 상태와 마지막 결과를 그대로 보여 준다.
(function () {
  const root = document.getElementById('extRoot');
  const api = window.sai;
  let view = null;
  let linkTimer = null;

  function el(tag, attrs = {}, ...children) {
    const n = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (k === 'class') n.className = v;
      else if (k === 'text') n.textContent = v;
      else if (k.startsWith('on')) n.addEventListener(k.slice(2), v);
      else if (v !== false && v != null) n.setAttribute(k, v === true ? '' : v);
    }
    for (const c of children.flat()) if (c != null && c !== false) n.append(c);
    return n;
  }
  const errMsg = (e) => String((e && e.message) || e).replace(/^Error invoking remote method '[^']+': (Error: )?/, '');
  const when = (t) => (t ? new Date(t).toLocaleString('ko-KR', { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }) : '아직 없음');

  async function act(fn, okText) {
    const msg = document.getElementById('extMsg');
    if (msg) msg.textContent = '처리하는 중…';
    try {
      const r = await fn();
      if (r && r.teamConnected !== undefined) view = r;
      else if (r && r.view) view = r.view;
      draw();
      const m = document.getElementById('extMsg');
      if (m) m.textContent = okText || '';
      return r;
    } catch (e) {
      draw();
      const m = document.getElementById('extMsg');
      if (m) {
        m.textContent = errMsg(e);
        m.classList.add('bad');
      }
      return null;
    }
  }

  async function render() {
    try {
      view = await api.extStatus();
    } catch (e) {
      view = null;
    }
    draw();
  }

  function stateLine(ok, text) {
    return el('div', { class: `ext-state ${ok ? 'on' : 'off'}` }, el('span', { class: 'dot' }), text);
  }

  function telegramBox() {
    const t = view.telegram;
    const box = el('div', { class: 'ext-item' }, el('div', { class: 'ext-title', text: 'Telegram 알림' }));
    box.append(
      el('p', {
        class: 'hint',
        text: '배정 제안, 도움 요청, 팀 약속 제안, 그리고 자리를 비웠을 때의 내 진행 확인만 보내요. 개인 대화나 고민 내용은 보내지 않아요.',
      }),
    );
    if (!view.teamConnected) {
      box.append(stateLine(false, '먼저 위에서 팀에 연결해 주세요.'));
      return box;
    }
    if (!t.bot) {
      const input = el('input', { type: 'password', placeholder: '123456789:AA…', autocomplete: 'off' });
      box.append(
        stateLine(false, '팀 봇이 아직 없어요. 팀에서 한 명만 넣으면 돼요.'),
        el('p', { class: 'hint', text: 'Telegram 의 @BotFather 에서 /newbot 으로 만든 봇의 토큰을 넣어 주세요. 토큰은 팀원만 볼 수 있게 저장돼요.' }),
        el('div', { class: 'inline' }, input, el('button', { text: '봇 저장', onclick: () => act(() => api.tgSetBot(input.value), '봇을 저장했어요.') })),
      );
      return box;
    }
    box.append(stateLine(t.linked, t.linked ? `@${t.bot.username} 와 연결됨 · 팀에서 ${t.linkedCount}명 연결` : `팀 봇 @${t.bot.username} · 아직 내 Telegram 과 연결 안 됨`));
    if (!t.linked) {
      box.append(
        el('div', { class: 'inline' }, el('button', { class: 'primary', text: t.pending ? 'Telegram 다시 열기' : 'Telegram 연결하기', onclick: startLink })),
        t.pending ? el('p', { class: 'hint', text: 'Telegram 에서 봇 대화가 열리면 아래쪽 "시작(Start)"을 눌러 주세요. 눌렀다면 자동으로 연결돼요.' }) : null,
      );
    } else {
      box.append(
        el(
          'label',
          { class: 'check' },
          el('input', { type: 'checkbox', checked: t.teamAlerts, onchange: (e) => act(() => api.tgTeamAlerts(e.target.checked), '저장했어요.') }),
          ' 팀 알림 받기 (배정·도움 요청·팀 약속)',
        ),
        el(
          'label',
          { class: 'check' },
          el('input', { type: 'checkbox', checked: view.telegramAway, onchange: (e) => act(() => api.tgAway(e.target.checked), '저장했어요.') }),
          ' 자리를 비웠을 때(10분 이상 입력 없음) 진행 확인을 Telegram 으로',
        ),
        el(
          'div',
          { class: 'inline' },
          el('button', { text: '시험 메시지 보내기', onclick: () => act(() => api.tgTest(), '보냈어요. Telegram 을 확인해 주세요.') }),
          el('button', { class: 'link', text: '연결 끊기', onclick: () => act(() => api.tgUnlink(), '연결을 끊었어요.') }),
        ),
      );
    }
    if (t.lastResult) box.append(el('div', { class: `hint ${t.lastResult.ok ? '' : 'err'}`, text: `마지막 결과 (${when(t.lastResult.at)}): ${t.lastResult.text}` }));
    return box;
  }

  async function startLink() {
    await act(() => api.tgStartLink(), 'Telegram 이 열렸어요. 봇 대화에서 "시작"을 눌러 주세요.');
    clearInterval(linkTimer);
    let tries = 0;
    linkTimer = setInterval(async () => {
      tries++;
      try {
        const r = await api.tgCheckLink();
        if (r.linked) {
          clearInterval(linkTimer);
          view = r.view;
          draw();
          document.getElementById('extMsg').textContent = '연결됐어요. Telegram 에 확인 메시지를 보냈어요.';
        }
      } catch (e) {
        clearInterval(linkTimer);
        document.getElementById('extMsg').textContent = errMsg(e);
      }
      if (tries > 60) clearInterval(linkTimer);
    }, 4000);
  }

  function googleBox() {
    const g = view.google;
    const box = el('div', { class: 'ext-item' }, el('div', { class: 'ext-title', text: 'Google 캘린더 · 할 일' }));
    box.append(
      el('p', {
        class: 'hint',
        text: '캘린더는 바쁜 시간만 읽어요 (일정 제목·내용은 못 봐요). 계산한 빈 시간은 이 컴퓨터에서만 쓰고, 팀에는 내가 저장한 "주간 가능 시간"만 보여요. 할 일에는 내가 수락한 작업만 올라가요.',
      }),
    );
    if (!g.configured) {
      const ta = el('textarea', { rows: 3, placeholder: '클라이언트 ID(…apps.googleusercontent.com)와 보안 비밀번호(GOCSPX-…), 또는 내려받은 JSON' });
      box.append(
        stateLine(false, 'Google 연결 정보가 아직 없어요.'),
        ta,
        el('div', { class: 'inline' }, el('button', { text: '연결 정보 저장', onclick: () => act(() => api.gConfigure(ta.value), '저장했어요. 이제 로그인해 주세요.') })),
      );
      return box;
    }
    if (!g.signedIn) {
      box.append(
        stateLine(false, '로그인 안 됨'),
        el('div', { class: 'inline' }, el('button', { class: 'primary', text: 'Google 로 로그인', onclick: () => act(() => api.gLogin(), 'Google 과 연결됐어요.') })),
        el('p', { class: 'hint', text: '브라우저에서 Google 로그인 화면이 열려요. 로그인 후 권한을 허용하면 자동으로 돌아와요. 테스트 중인 앱이라 약 7일마다 다시 로그인해야 해요.' }),
      );
    } else {
      box.append(
        stateLine(true, `${g.email || 'Google 계정'} 연결됨`),
        el('div', { class: 'hint', text: `마지막 할 일 동기화: ${when(g.lastSync)}${g.lastResult ? ` — ${g.lastResult}` : ''}` }),
        el(
          'div',
          { class: 'inline' },
          el('button', { text: '할 일 지금 동기화', onclick: () => act(async () => { const r = await api.gSync(); return r; }, '동기화했어요.') }),
          el('button', { class: 'link', text: '로그아웃', onclick: () => act(() => api.gLogout(), '로그아웃했어요.') }),
        ),
      );
    }
    if (g.lastError) box.append(el('div', { class: 'hint err', text: `문제: ${g.lastError}` }));
    return box;
  }

  function draw() {
    root.textContent = '';
    if (!view) {
      root.append(el('p', { class: 'hint', text: '외부 연결 상태를 불러오지 못했어요.' }));
      return;
    }
    root.append(telegramBox(), googleBox(), el('div', { id: 'extMsg', class: 'status' }));
  }

  window.SaiExt = { render };
  // 대화창이 설정 탭으로 열렸을 때 탭 전환보다 이 파일이 늦게 준비될 수 있어 스스로 한 번 그린다
  render();
})();
