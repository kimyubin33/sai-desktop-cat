//! 고양이 행동 상태 기계
//! 상태: sit(앉기) / walk(걷기) / loaf(식빵 자세로 쉬기) / sleep(잠자기) / talk(말하는 중) / drag(들려 있음)
use rand::Rng;

pub const WALK_SPEED: f64 = 55.0; // px/초

pub struct CatBehavior {
    pub min_x: f64,
    pub max_x: f64,
    pub x: f64,
    pub state: &'static str,
    pub facing: i32, // 1: 오른쪽, -1: 왼쪽
    timer: f64,
    target_x: f64,
    /// 조용한 모드일 때는 덜 돌아다닌다
    pub calm: bool,
}

fn between(a: f64, b: f64) -> f64 {
    a + rand::thread_rng().gen::<f64>() * (b - a)
}

impl CatBehavior {
    pub fn new(min_x: f64, max_x: f64, x: f64) -> Self {
        CatBehavior { min_x, max_x, x, state: "sit", facing: 1, timer: 3000.0, target_x: x, calm: false }
    }

    pub fn set_bounds(&mut self, min_x: f64, max_x: f64) {
        self.min_x = min_x;
        self.max_x = max_x.max(min_x);
        self.x = self.x.clamp(self.min_x, self.max_x);
    }

    /// 다음 상태를 고른다. 오래 움직였으면 쉬고, 쉬었으면 움직인다.
    fn pick_next(&mut self) {
        let r: f64 = rand::thread_rng().gen();
        let next = match self.state {
            "walk" => {
                if r < 0.6 {
                    "sit"
                } else if r < 0.9 {
                    "loaf"
                } else {
                    "sleep"
                }
            }
            "sleep" => {
                if r < 0.7 {
                    "sit"
                } else {
                    "walk"
                }
            }
            "loaf" => {
                if r < 0.5 {
                    "sleep"
                } else if r < 0.8 {
                    "sit"
                } else {
                    "walk"
                }
            }
            _ => {
                if r < if self.calm { 0.3 } else { 0.55 } {
                    "walk"
                } else if r < 0.85 {
                    "loaf"
                } else {
                    "sleep"
                }
            }
        };
        self.enter(next);
    }

    pub fn enter(&mut self, state: &'static str) {
        self.state = state;
        match state {
            "walk" => {
                let span = self.max_x - self.min_x;
                let mut rng = rand::thread_rng();
                let dist = 80.0 + rng.gen::<f64>() * (span * 0.5).clamp(120.0, 500.0);
                let mut dir = if rng.gen::<f64>() < 0.5 { -1.0 } else { 1.0 };
                if self.x - dist < self.min_x {
                    dir = 1.0;
                }
                if self.x + dist > self.max_x {
                    dir = -1.0;
                }
                self.target_x = (self.x + dir * dist).clamp(self.min_x, self.max_x);
                self.facing = if self.target_x >= self.x { 1 } else { -1 };
                self.timer = f64::INFINITY;
            }
            "sit" => self.timer = between(2500.0, 6000.0),
            "loaf" => self.timer = between(5000.0, 10000.0),
            "sleep" => self.timer = between(12000.0, 30000.0),
            _ => self.timer = f64::INFINITY, // talk, drag 는 바깥에서 끝낸다
        }
    }

    /// 말풍선이 뜨면 걷던 것을 멈추고 앉아서 말한다
    pub fn start_talking(&mut self) {
        if self.state != "drag" {
            self.enter("talk");
        }
    }

    pub fn stop_talking(&mut self) {
        if self.state == "talk" {
            self.enter("sit");
        }
    }

    pub fn start_drag(&mut self) {
        self.enter("drag");
    }

    pub fn end_drag(&mut self, x: f64) {
        self.x = x.clamp(self.min_x, self.max_x);
        self.enter("sit");
    }

    /// dt(ms)만큼 진행. 위치가 바뀌었으면 true
    pub fn tick(&mut self, dt: f64) -> bool {
        if self.state == "drag" || self.state == "talk" {
            return false;
        }
        if self.state == "walk" {
            let step = WALK_SPEED * dt / 1000.0;
            let remain = self.target_x - self.x;
            if remain.abs() <= step {
                self.x = self.target_x;
                self.pick_next();
            } else {
                self.x += remain.signum() * step;
            }
            return true;
        }
        self.timer -= dt;
        if self.timer <= 0.0 {
            self.pick_next();
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_within_bounds() {
        let mut b = CatBehavior::new(0.0, 500.0, 250.0);
        for _ in 0..20000 {
            b.tick(33.0);
            assert!(b.x >= 0.0 && b.x <= 500.0);
        }
        b.start_talking();
        assert_eq!(b.state, "talk");
        assert!(!b.tick(33.0));
        b.stop_talking();
        assert_eq!(b.state, "sit");
    }
}
