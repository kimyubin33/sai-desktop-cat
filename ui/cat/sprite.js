// 사이의 도트 스프라이트를 코드로 그린다 (32x32 도트, 오른쪽을 바라보는 기준).
// 외부 그림 파일 없이 모양을 조절할 수 있도록 타원·삼각형 도형을 도트 단위로 찍는다.
(function () {
  const C = {
    line: '#3b2f2f',
    fur: '#f2a65a',
    stripe: '#d6803a',
    light: '#fbe3c4',
    pink: '#f7b2b7',
    eye: '#2b2b2b',
    shine: '#ffffff',
  };

  function px(g, x, y, w = 1, h = 1, c) {
    g.fillStyle = c;
    g.fillRect(Math.round(x), Math.round(y), w, h);
  }

  function ellipse(g, cx, cy, rx, ry, c) {
    for (let y = -ry; y <= ry; y++) {
      for (let x = -rx; x <= rx; x++) {
        if ((x * x) / (rx * rx + 0.3) + (y * y) / (ry * ry + 0.3) <= 1) px(g, cx + x, cy + y, 1, 1, c);
      }
    }
  }

  // 외곽선이 있는 타원
  function blob(g, cx, cy, rx, ry, fill) {
    ellipse(g, cx, cy, rx + 1, ry + 1, C.line);
    ellipse(g, cx, cy, rx, ry, fill);
  }

  // 귀: 꼭짓점(tx,ty) 아래로 넓어지는 삼각형
  function ear(g, tx, ty, h) {
    for (let i = 0; i <= h; i++) {
      px(g, tx - i * 0.6 - 1, ty + i, i * 1.2 + 2, 1, C.line);
    }
    for (let i = 1; i < h; i++) {
      px(g, tx - i * 0.6, ty + i, Math.max(1, i * 1.2), 1, i > 1 ? C.fur : C.line);
    }
    for (let i = 2; i < h - 1; i++) px(g, tx - i * 0.3 + 0.2, ty + i + 0.5, Math.max(1, i * 0.5), 1, C.pink);
  }

  function head(g, cx, cy, { eyes = 'open', mouth = false } = {}) {
    ear(g, cx - 3, cy - 8, 5);
    ear(g, cx + 3, cy - 8, 5);
    blob(g, cx, cy, 6, 5, C.fur);
    // 이마 줄무늬
    px(g, cx - 1, cy - 5, 1, 2, C.stripe);
    px(g, cx + 1, cy - 5, 1, 2, C.stripe);
    // 볼
    ellipse(g, cx + 1, cy + 3, 4, 1, C.light);
    if (eyes === 'open') {
      px(g, cx - 2, cy - 1, 2, 2, C.eye);
      px(g, cx + 3, cy - 1, 2, 2, C.eye);
      px(g, cx - 2, cy - 1, 1, 1, C.shine);
      px(g, cx + 3, cy - 1, 1, 1, C.shine);
    } else {
      // 감은 눈
      px(g, cx - 3, cy, 3, 1, C.eye);
      px(g, cx + 3, cy, 3, 1, C.eye);
    }
    px(g, cx + 1, cy + 1, 2, 1, C.pink); // 코
    if (mouth) {
      px(g, cx + 1, cy + 3, 2, 1, C.line);
      px(g, cx + 1, cy + 2, 2, 1, '#c96a6a');
    } else {
      px(g, cx, cy + 2, 1, 1, C.line);
      px(g, cx + 3, cy + 2, 1, 1, C.line);
    }
    // 수염
    px(g, cx - 8, cy + 1, 3, 1, C.line);
    px(g, cx + 7, cy + 1, 3, 1, C.line);
  }

  function tail(g, points) {
    for (const [x, y] of points) {
      px(g, x - 1, y - 1, 3, 3, C.line);
    }
    for (const [x, y] of points) px(g, x, y, 1, 1, C.fur);
  }

  const draw = {
    sit(g, f, opts) {
      const sway = f % 2 === 0 ? 0 : 1;
      tail(g, [[6, 29], [5, 28], [4, 27 - sway], [4, 26 - sway], [5, 25 - sway], [7, 29], [8, 29], [9, 29]]);
      blob(g, 14, 24, 7, 6, C.fur);
      px(g, 9, 21, 2, 1, C.stripe);
      px(g, 8, 24, 2, 1, C.stripe);
      ellipse(g, 16, 25, 3, 4, C.light);
      // 앞발
      px(g, 13, 29, 3, 2, C.line);
      px(g, 17, 29, 3, 2, C.line);
      px(g, 14, 29, 1, 1, C.light);
      px(g, 18, 29, 1, 1, C.light);
      head(g, 16, 13, opts);
    },

    walk(g, f) {
      const step = f % 2;
      tail(g, [[6, 19], [5, 18], [4, 16], [4, 14], [5, 12], [6, 11]]);
      // 다리 (뒤 → 앞)
      const legs = step ? [[8, 0], [12, 1], [18, 1], [22, 0]] : [[9, 1], [11, 0], [19, 0], [21, 1]];
      for (const [lx, up] of legs) {
        px(g, lx - 1, 25 - up, 3, 6 + up, C.line);
        px(g, lx, 25 - up, 1, 5 + up, C.fur);
      }
      blob(g, 15, 21, 9, 4, C.fur);
      px(g, 10, 18, 1, 2, C.stripe);
      px(g, 13, 18, 1, 2, C.stripe);
      px(g, 16, 18, 1, 2, C.stripe);
      ellipse(g, 16, 23, 6, 1, C.light);
      head(g, 24, 13 + (step ? 1 : 0), {});
    },

    loaf(g, f, opts) {
      tail(g, [[5, 29], [6, 29], [7, 29], [4, 28]]);
      blob(g, 15, 25, 10, 5, C.fur);
      px(g, 9, 22, 1, 2, C.stripe);
      px(g, 12, 21, 1, 2, C.stripe);
      px(g, 15, 21, 1, 2, C.stripe);
      px(g, 20, 29, 4, 2, C.line);
      px(g, 21, 29, 2, 1, C.light);
      head(g, 22, 18, opts);
    },

    sleep(g, f) {
      tail(g, [[23, 30], [22, 30], [21, 30], [20, 30], [19, 30], [18, 30]]);
      const breathe = f % 2;
      blob(g, 14, 26 - breathe, 10, 4 + breathe, C.fur);
      px(g, 8, 24, 1, 2, C.stripe);
      px(g, 11, 23, 1, 2, C.stripe);
      px(g, 14, 23, 1, 2, C.stripe);
      head(g, 21, 23, { eyes: 'closed' });
    },

    drag(g, f) {
      const swing = f % 2 ? 1 : -1;
      // 늘어진 몸과 다리
      px(g, 11 + swing, 26, 3, 5, C.line);
      px(g, 18 + swing, 26, 3, 5, C.line);
      px(g, 12 + swing, 26, 1, 4, C.fur);
      px(g, 19 + swing, 26, 1, 4, C.fur);
      tail(g, [[16 + swing * 2, 28], [16 + swing * 2, 29], [17 + swing * 2, 30]]);
      blob(g, 16, 21, 5, 6, C.fur);
      ellipse(g, 16, 22, 3, 4, C.light);
      px(g, 9, 15, 3, 3, C.line); // 앞발
      px(g, 21, 15, 3, 3, C.line);
      head(g, 16, 10, { eyes: 'open', mouth: true });
    },
  };
  draw.talk = (g, f) => draw.sit(g, 0, { mouth: f % 2 === 0 });

  /**
   * state, frame, facing(1|-1) 로 32x32 오프스크린 캔버스에 그린 뒤
   * 대상 캔버스에 도트가 뭉개지지 않게 확대해서 그린다.
   */
  function render(target, state, frame, facing, opts = {}) {
    const off = render.off || (render.off = document.createElement('canvas'));
    off.width = 32;
    off.height = 32;
    const g = off.getContext('2d');
    g.clearRect(0, 0, 32, 32);
    (draw[state] || draw.sit)(g, frame, opts);

    const t = target.getContext('2d');
    t.imageSmoothingEnabled = false;
    t.clearRect(0, 0, target.width, target.height);
    t.save();
    if (facing < 0) {
      t.translate(target.width, 0);
      t.scale(-1, 1);
    }
    t.drawImage(off, 0, 0, target.width, target.height);
    t.restore();
  }

  window.SaiSprite = { render, STATES: Object.keys(draw) };
})();
