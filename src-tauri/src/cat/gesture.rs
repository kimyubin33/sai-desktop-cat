//! 최근 커서 경로로 흔들기와 던지기를 구분한다. 속도 기준은 배율에 무관한 논리 px/ms.
use std::collections::VecDeque;

type Point = (f64, f64);
const SHAKE_WINDOW: i64 = 620;
const FLICK_WINDOW: i64 = 100;
const THROW_SPEED: f64 = 0.85;
const IMPACT_SPEED: f64 = 0.60;

pub(super) struct Trail {
    points: VecDeque<(i64, Point)>,
    shook: bool,
}

impl Trail {
    pub(super) fn new(at: i64, pos: Point) -> Self {
        Self {
            points: VecDeque::from([(at, pos)]),
            shook: false,
        }
    }

    pub(super) fn sample(&mut self, at: i64, pos: Point, scale: f64) -> bool {
        self.points
            .retain(|(t, _)| *t >= at - SHAKE_WINDOW && *t < at);
        self.points.push_back((at, pos));
        if self.shook {
            return false;
        }
        let distance = 28.0 * scale;
        let shook = reversals(self.points.iter().map(|(_, p)| p.0), distance) >= 3
            || reversals(self.points.iter().map(|(_, p)| p.1), distance) >= 3;
        self.shook = shook;
        shook
    }

    pub(super) fn throw_velocity(&self, scale: f64) -> Point {
        let Some(&(now, end)) = self.points.back() else {
            return (0.0, 0.0);
        };
        let Some(&(at, start)) = self.points.iter().find(|(t, _)| *t >= now - FLICK_WINDOW) else {
            return (0.0, 0.0);
        };
        if now - at < 10 {
            return (0.0, 0.0);
        }
        let dt = (now - at) as f64;
        let (vx, vy) = ((end.0 - start.0) / dt, (end.1 - start.1) / dt);
        let speed = vx.hypot(vy);
        if speed < THROW_SPEED * scale {
            return (0.0, 0.0);
        }
        let limit = (3.2 * scale / speed).min(1.0);
        (vx * limit, vy * limit)
    }
}

fn reversals(mut coords: impl Iterator<Item = f64>, distance: f64) -> usize {
    let Some(mut extreme) = coords.next() else {
        return 0;
    };
    let mut sign = 0.0;
    let mut turns = 0;
    for p in coords {
        let delta = p - extreme;
        if sign == 0.0 {
            if delta.abs() < distance {
                continue;
            }
            sign = delta.signum();
        } else if delta * sign < 0.0 {
            if delta.abs() < distance {
                continue;
            }
            sign = -sign;
            turns += 1;
        }
        extreme = p;
    }
    turns
}

#[derive(Clone, Copy)]
pub(super) struct Bounds {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub floor: f64,
}

pub(super) struct Flight {
    pub x: f64,
    pub y: f64,
    vx: f64,
    vy: f64,
    // 손에서 놓은 속도가 있어야 충돌 반응을 한다. 평소의 낙하는 바닥에 조용히 앉는다.
    thrown: bool,
    reacted: bool,
}

impl Flight {
    pub(super) fn new(pos: Point, velocity: Point) -> Self {
        Self {
            x: pos.0,
            y: pos.1,
            vx: velocity.0,
            vy: velocity.1,
            thrown: velocity != (0.0, 0.0),
            reacted: false,
        }
    }

    /// 한 프레임 뒤 (처음 세게 부딪혔는지, 바닥에 멈췄는지).
    pub(super) fn step(&mut self, dt: f64, scale: f64, walls: Bounds) -> (bool, bool) {
        let mut impact = false;
        self.vy += 0.0025 * scale * dt;
        self.x += self.vx * dt;
        self.y += self.vy * dt;
        if (self.x <= walls.left && self.vx < 0.0) || (self.x >= walls.right && self.vx > 0.0) {
            impact = self.vx.abs() >= IMPACT_SPEED * scale;
            self.vx *= -0.64;
            if impact && self.y >= walls.floor - 10.0 * scale {
                self.vy = -0.48 * scale;
            }
        }
        self.x = self.x.clamp(walls.left, walls.right);
        if self.y < walls.top && self.vy < 0.0 {
            impact |= self.vy.abs() >= IMPACT_SPEED * scale;
            self.vy *= -0.55;
        }
        self.y = self.y.max(walls.top);
        if self.y >= walls.floor {
            self.y = walls.floor;
            if self.thrown && self.vy > 0.65 * scale {
                impact = true;
                self.vy *= -0.40;
            } else if self.vy >= 0.0 {
                self.vy = 0.0;
            }
            self.vx *= 0.82_f64.powf(dt / 33.0);
        } else {
            self.vx *= 0.995_f64.powf(dt / 33.0);
        }
        let hurt = self.thrown && impact && !self.reacted;
        self.reacted |= hurt;
        (
            hurt,
            self.y == walls.floor && self.vy == 0.0 && self.vx.abs() < 0.08 * scale,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shake_requires_repeated_large_reversals_and_fires_once() {
        for vertical in [true, false] {
            for scale in [1.0, 2.0] {
                let mut trail = Trail::new(0, (0.0, 0.0));
                let hits = [40.0, 0.0, 40.0, 0.0, 40.0, 0.0]
                    .into_iter()
                    .enumerate()
                    .filter(|(i, v)| {
                        let pos = if vertical {
                            (0.0, *v * scale)
                        } else {
                            (*v * scale, 0.0)
                        };
                        trail.sample((*i as i64 + 1) * 65, pos, scale)
                    })
                    .count();
                assert_eq!(hits, 1);
            }
        }
        let mut slow = Trail::new(0, (0.0, 0.0));
        for i in 1..30 {
            assert!(!slow.sample(i * 65, (i as f64 * 12.0, (i % 2) as f64 * 4.0), 1.0));
        }
    }

    #[test]
    fn release_uses_recent_motion_and_scales_with_dpi() {
        for scale in [1.0, 1.5, 2.0] {
            let mut trail = Trail::new(0, (0.0, 0.0));
            trail.sample(60, (30.0 * scale, 0.0), scale);
            assert_eq!(trail.throw_velocity(scale), (0.0, 0.0));
            trail.sample(96, (192.0 * scale, 0.0), scale);
            assert_eq!(trail.throw_velocity(scale), (2.0 * scale, 0.0));
            trail.sample(230, (192.0 * scale, 0.0), scale);
            assert_eq!(trail.throw_velocity(scale), (0.0, 0.0));
        }
    }

    #[test]
    fn strong_throw_bounces_inward_then_lands_and_normal_drop_is_quiet() {
        let walls = Bounds {
            left: -300.0,
            right: 300.0,
            top: -180.0,
            floor: 260.0,
        };
        for dir in [-1.0, 1.0] {
            let mut flight = Flight::new((285.0 * dir, 260.0), (1.7 * dir, 0.0));
            assert_eq!(flight.step(33.0, 1.0, walls), (true, false));
            assert_eq!(flight.x, dir * 300.0);
            let mut landed = false;
            for i in 0..200 {
                let (hurt, done) = flight.step(33.0, 1.0, walls);
                if i == 0 {
                    assert!(flight.x.abs() < 300.0 && flight.y < 260.0);
                }
                assert!(!hurt);
                assert!((-300.0..=300.0).contains(&flight.x));
                if done {
                    landed = true;
                    break;
                }
            }
            assert!(landed);
        }
        let mut drop = Flight::new((45.0, -150.0), (0.0, 0.0));
        assert!((0..100).any(|_| {
            let (hurt, landed) = drop.step(33.0, 1.0, walls);
            assert!(!hurt);
            landed
        }));
        assert_eq!((drop.x, drop.y), (45.0, 260.0));
    }
}
