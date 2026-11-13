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

  // 투명한 부분은 클릭이 아래 창으로 통과하고, 고양이 그림과 말풍선 위에서만 마우스를 받는다.
  // 마우스가 그 위에 있는지는 앱이 판단하므로, 여기서는 "마우스를 받을 영역"만 알려 준다.
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
  /** 캔버스에서 그려진 픽셀이 있는 가로줄 묶음 → 창 기준 사각형 몇 개 */
  function catRects() {
    const w = canvas.width;
    const h = canvas.height;
    const data = ctx2d.getImageData(0, 0, w, h).data;
    const r = canvas.getBoundingClientRect();
    const sx = r.width / w;
    const sy = r.height / h;
    const rects = [];
    const BAND = 8;
    for (let y0 = 0; y0 < h; y0 += BAND) {
      let minX = w;
      let maxX = -1;
      for (let y = y0; y < Math.min(h, y0 + BAND); y++) {
        for (let x = 0; x < w; x++) {
          if (data[(y * w + x) * 4 + 3] > 30) {
            if (x < minX) minX = x;
            if (x > maxX) maxX = x;
          }
        }
      }
      if (maxX >= 0) rects.push({ x: r.left + (minX - 2) * sx, y: r.top + y0 * sy, w: (maxX - minX + 5) * sx, h: BAND * sy });
    }
    return rects;
  }
  let lastRegions = '';
  function reportRegions() {
    const list = catRects();
    if (!bubble.classList.contains('hidden')) {
      const b = bubble.getBoundingClientRect();
      list.push({ x: b.left, y: b.top, w: b.width, h: b.height });
    }
    const key = JSON.stringify(list.map((q) => [Math.round(q.x), Math.round(q.y), Math.round(q.w), Math.round(q.h)]));
    if (key === lastRegions) return;
    lastRegions = key;
    window.cat.regions(list);
  }
  setInterval(reportRegions, 150);
  window.cat.ready();

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
