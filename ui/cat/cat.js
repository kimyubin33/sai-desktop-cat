(function () {
  const canvas = document.getElementById('cat');
  const bubble = document.getElementById('bubble');
  const zz = document.getElementById('zz');
  const textEl = bubble.querySelector('.text');
  const sourceEl = bubble.querySelector('.source');

  let state = 'sit';
  let facing = 1;
  let frame = 0;
  let lastFrameAt = 0;
  let blinkUntil = 0;
  let currentBubble = null;

  const FRAME_MS = { walk: 160, sit: 700, loaf: 1200, sleep: 900, talk: 220, drag: 200 };

  function loop(now) {
    if (now - lastFrameAt > (FRAME_MS[state] || 500)) {
      frame++;
      lastFrameAt = now;
      if ((state === 'sit' || state === 'loaf') && Math.random() < 0.08) blinkUntil = now + 160;
    }
    const blinking = now < blinkUntil;
    window.SaiSprite.render(canvas, state, frame, facing, { eyes: blinking ? 'closed' : 'open' });
    requestAnimationFrame(loop);
  }
  requestAnimationFrame(loop);

  window.cat.onState((s) => {
    if (s.state !== state) frame = 0;
    state = s.state;
    facing = s.facing;
    zz.classList.toggle('hidden', state !== 'sleep');
    canvas.classList.toggle('dragging', state === 'drag');
  });

  window.cat.onBubble((b) => {
    currentBubble = b;
    textEl.textContent = b.text;
    sourceEl.textContent = b.sourceLabel || '';
    const actionsEl = bubble.querySelector('.actions');
    actionsEl.textContent = '';
    for (const a of b.actions || []) {
      const btn = document.createElement('button');
      btn.dataset.action = a.id;
      btn.textContent = a.label;
      if (a.primary) btn.className = 'primary';
      actionsEl.append(btn);
    }
    bubble.classList.remove('hidden');
  });

  window.cat.onHideBubble(() => {
    bubble.classList.add('hidden');
    currentBubble = null;
  });

  // 투명한 부분은 클릭이 아래 창으로 통과하고, 고양이와 말풍선 위에서만 마우스를 받는다
  function interactiveWhileHover(el) {
    el.addEventListener('mouseenter', () => window.cat.setInteractive(true));
    el.addEventListener('mouseleave', () => {
      if (state !== 'drag') window.cat.setInteractive(false);
    });
  }
  interactiveWhileHover(bubble);

  // 고양이 그림은 캔버스의 일부일 뿐이라, 캔버스 전체가 아니라 그려진 픽셀 위에서만 마우스를 받는다.
  // (캔버스 전체로 받으면 고양이 근처의 다른 창을 눌러도 첫 클릭이 고양이 창에 먹힌다)
  let overCat = false;
  const ctx2d = canvas.getContext('2d', { willReadFrequently: true });
  function hitCat(e) {
    const r = canvas.getBoundingClientRect();
    const x = Math.floor(((e.clientX - r.left) / r.width) * canvas.width);
    const y = Math.floor(((e.clientY - r.top) / r.height) * canvas.height);
    if (x < 0 || y < 0 || x >= canvas.width || y >= canvas.height) return false;
    // 가장자리 몇 픽셀 여유를 둔다
    const pad = 3;
    const d = ctx2d.getImageData(Math.max(0, x - pad), Math.max(0, y - pad), pad * 2 + 1, pad * 2 + 1).data;
    for (let i = 3; i < d.length; i += 4) if (d[i] > 30) return true;
    return false;
  }
  function setOver(on) {
    if (on === overCat) return;
    overCat = on;
    if (on) window.cat.setInteractive(true);
    else if (state !== 'drag') window.cat.setInteractive(false);
  }
  canvas.addEventListener('mousemove', (e) => setOver(hitCat(e)));
  canvas.addEventListener('mouseleave', () => setOver(false));

  bubble.addEventListener('mouseenter', () => window.cat.bubbleHover(true));
  bubble.addEventListener('mouseleave', () => window.cat.bubbleHover(false));

  canvas.addEventListener('mousedown', (e) => {
    if (!hitCat(e)) return;
    // 오른쪽 버튼은 눌렀을 때 바로 메뉴 (contextmenu 이벤트는 투명 창에서 오지 않을 때가 있다)
    if (e.button === 2) {
      e.preventDefault();
      window.cat.contextMenu();
      return;
    }
    if (e.button !== 0) return;
    e.preventDefault();
    window.cat.dragStart();
  });
  window.addEventListener('mouseup', (e) => {
    if (e.button !== 0) return;
    window.cat.dragEnd();
  });
  canvas.addEventListener('contextmenu', (e) => e.preventDefault());

  bubble.addEventListener('click', (e) => {
    const btn = e.target.closest('button[data-action]');
    if (!btn || !currentBubble) return;
    window.cat.bubbleAction(currentBubble.id, btn.dataset.action);
  });
})();
