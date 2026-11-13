// 고양이 행동 상태 기계 (Electron 의존성 없음 — 테스트 가능)
// 상태: sit(앉기) / walk(걷기) / loaf(식빵 자세로 쉬기) / sleep(잠자기) / talk(말하는 중) / drag(들려 있음)

const DURATIONS = {
  sit: [2500, 6000],
  loaf: [5000, 10000],
  sleep: [12000, 30000],
};

const WALK_SPEED = 55; // px/초

function randBetween(rand, [a, b]) {
  return a + rand() * (b - a);
}

class CatBehavior {
  constructor({ rand = Math.random, minX = 0, maxX = 800, x = 200 } = {}) {
    this.rand = rand;
    this.minX = minX;
    this.maxX = maxX;
    this.x = x;
    this.state = 'sit';
    this.facing = 1; // 1: 오른쪽, -1: 왼쪽
    this.timer = 3000;
    this.targetX = null;
    this.calm = false; // 조용한 모드일 때는 덜 돌아다닌다
  }

  setBounds(minX, maxX) {
    this.minX = minX;
    this.maxX = Math.max(minX, maxX);
    this.x = Math.min(Math.max(this.x, this.minX), this.maxX);
  }

  /** 다음 상태를 고른다. 오래 움직였으면 쉬고, 쉬었으면 움직인다. */
  pickNext() {
    const r = this.rand();
    const prev = this.state;
    let next;
    if (prev === 'walk') next = r < 0.6 ? 'sit' : r < 0.9 ? 'loaf' : 'sleep';
    else if (prev === 'sleep') next = r < 0.7 ? 'sit' : 'walk';
    else if (prev === 'loaf') next = r < 0.5 ? 'sleep' : r < 0.8 ? 'sit' : 'walk';
    else next = r < (this.calm ? 0.3 : 0.55) ? 'walk' : r < 0.85 ? 'loaf' : 'sleep';
    this.enter(next);
  }

  enter(state) {
    this.state = state;
    if (state === 'walk') {
      const span = this.maxX - this.minX;
      const dist = 80 + this.rand() * Math.min(500, Math.max(120, span * 0.5));
      let dir = this.rand() < 0.5 ? -1 : 1;
      if (this.x - dist < this.minX) dir = 1;
      if (this.x + dist > this.maxX) dir = -1;
      this.targetX = Math.min(Math.max(this.x + dir * dist, this.minX), this.maxX);
      this.facing = this.targetX >= this.x ? 1 : -1;
      this.timer = Infinity;
    } else if (DURATIONS[state]) {
      this.timer = randBetween(this.rand, DURATIONS[state]);
    } else {
      this.timer = Infinity; // talk, drag 는 외부에서 끝낸다
    }
  }

  /** 말풍선이 뜨면 걷던 것을 멈추고 앉아서 말한다 */
  startTalking() {
    if (this.state === 'drag') return;
    this.enter('talk');
  }

  stopTalking() {
    if (this.state === 'talk') this.enter('sit');
  }

  startDrag() {
    this.enter('drag');
  }

  endDrag(x) {
    this.x = Math.min(Math.max(x, this.minX), this.maxX);
    this.enter('sit');
  }

  /** dt(ms)만큼 진행. 위치가 바뀌었으면 true */
  tick(dt) {
    if (this.state === 'drag' || this.state === 'talk') return false;
    if (this.state === 'walk') {
      const step = (WALK_SPEED * dt) / 1000;
      const remain = this.targetX - this.x;
      if (Math.abs(remain) <= step) {
        this.x = this.targetX;
        this.pickNext();
      } else {
        this.x += Math.sign(remain) * step;
      }
      return true;
    }
    this.timer -= dt;
    if (this.timer <= 0) this.pickNext();
    return false;
  }

  snapshot() {
    return { state: this.state, facing: this.facing, x: Math.round(this.x) };
  }
}

module.exports = { CatBehavior, WALK_SPEED };
