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
  interactiveWhileHover(canvas);
  interactiveWhileHover(bubble);

  bubble.addEventListener('mouseenter', () => window.cat.bubbleHover(true));
  bubble.addEventListener('mouseleave', () => window.cat.bubbleHover(false));

  canvas.addEventListener('mousedown', (e) => {
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
