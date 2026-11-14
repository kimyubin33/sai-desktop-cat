//! 32×32 도트 고양이. 각 포즈가 같은 얼굴, 팔레트, 꼬리를 쓴다.
//! 캔버스 렌더링과 클릭 판정은 같은 RGBA 데이터를 사용한다.

pub const SIZE: usize = 32;

type Rgba = [u8; 4];
const INK: Rgba = [0x30, 0x2d, 0x33, 255];
const STRIPE: Rgba = [0x55, 0x54, 0x5c, 255];
const SHADOW: Rgba = [0x80, 0x7d, 0x83, 255];
const FUR: Rgba = [0xaa, 0xa9, 0xad, 255];
const LIGHT: Rgba = [0xc3, 0xc1, 0xc4, 255];
const BLUSH: Rgba = [0xf6, 0xa8, 0xbb, 255];
const TONGUE: Rgba = [0xc8, 0x7e, 0x92, 255];
const EYE: Rgba = [0x20, 0x1f, 0x28, 255];

const HEAD: &[&[u8]] = &[
    b"...XX...........XX...",
    b"..XggX.........XggX..",
    b"..XsssX.......XsssX..",
    b"..XpsssX.....XssspX..",
    b"..XsssggXXXXXggsssX..",
    b".XgggggddgdggdgggggX.",
    b".XgggggdggdggggggggX.",
    b"XggghhhgggggggggggggX",
    b"XggghgggggggggggggggX",
    b"XsssgggggggggggggsssX",
    b"XssgggggggggggggggssX",
    b".XpppgggggggggggpppX.",
    b".XssshhhggggggggsssX.",
    b"..XXgggghhhhhhgggXX..",
    b"....XXgggggggggXX....",
    b"......XXXXXXXXX......",
];

const SITTING: &[&[u8]] = &[
    b"........XXXXXXXX........",
    b".......XggggggggX.......",
    b"......XggggggggggX......",
    b".....XggggggggggggX.....",
    b"....XggggggggggggggX....",
    b"...XggggggggggggggggX...",
    b"...XgggggggggggggdddX...",
    b"..XddddgggggggggggdddX..",
    b"..XdddgggghhhhgggggggX..",
    b".XssssggghhhhhhgggggggX.",
    b".XggggggghhhhhhggggdddX.",
    b"XgdddggghhhhhhhhggggdddX",
    b"XgddgggghhhhhhhhgggggggX",
    b"XssssgggghhhhhhhgggggggX",
    b"XggggggggghhhhhggggggggX",
    b".XssssggggggggggssssssX.",
    b"..XggsssssggsssssssggX..",
    b"...XXggggggggggggggXX...",
    b".....XXXXXXXXXXXXXX.....",
];

const PAW: &[&[u8]] = &[
    b".XXX.",
    b"XgggX",
    b"XhhgX",
    b"XgggX",
    b".XXX.",
];

const WALKING: &[&[u8]] = &[
    b"....XXXXXXXXXXXX....",
    b"...XddggddggddggXX..",
    b".XXgddggddggdgggggX.",
    b"XgggdgggdggggggggggX",
    b"XggggggggggggggggggX",
    b"XggggggggggggggggggX",
    b"XgsssggggggggggggggX",
    b".XsssgggggggggggggX.",
    b"..XsssssghhhhhhhhXX.",
    b"...XXssggghhhhgXX...",
    b".....XXXXXXXXXX.....",
];

const LOAF: &[&[u8]] = &[
    b".....XXXXXXXXX........",
    b"...XXddggddgggXXX.....",
    b"..XggddggddggddggXX...",
    b".XgggdgggdgggggggggX..",
    b"XgggggggggggggggggggX.",
    b"XsssgggggggggggggggggX",
    b"XggggggggggggggggggggX",
    b".XssssgggggghhhhhhgggX",
    b"..XXssssgggggghhhhhXX.",
    b"....XXXXXXXXXXXXXXX...",
];

const HELD: &[&[u8]] = &[
    b".....XXXXXX.....",
    b"...XXggggggXX...",
    b"...XggggggggX...",
    b"..XggggggggggX..",
    b".XgggghhhhggggX.",
    b".XdddghhhhhgggX.",
    b".XddghhhhhhggggX",
    b"XggggghhhhhhdddX",
    b".XgddghhhhhggggX",
    b".XggggghhhhggggX",
    b".XsssgggggggsssX",
    b"..XgssssssssggX.",
    b"...XXXXXXXXXXX..",
];

const TAIL_UP: &[&[u8]] = &[
    b".....XXX..",
    b"....XdddX.",
    b"....XhhddX",
    b"....XXhggX",
    b"......XssX",
    b"......XggX",
    b"......XddX",
    b".....XgssX",
    b"....XgggX.",
    b".XXXgsssX.",
    b"XgsssgXX..",
    b"XgggXX....",
    b".XXX......",
];

const TAIL_DOWN: &[&[u8]] = &[
    b"....XXX...",
    b"...XdddX..",
    b"...XhhddX.",
    b"...XXXgggX",
    b"......XssX",
    b"......XggX",
    b"......XddX",
    b"......XssX",
    b".....XgggX",
    b"...XXgggX.",
    b".XXggssX..",
    b"XgsssXX...",
    b"XXXXX.....",
];


struct Canvas(Vec<u8>);

impl Canvas {
    fn new() -> Self {
        Self(vec![0; SIZE * SIZE * 4])
    }

    fn pixel(&mut self, x: i32, y: i32, rgba: Rgba) {
        if (0..SIZE as i32).contains(&x) && (0..SIZE as i32).contains(&y) {
            let i = (y as usize * SIZE + x as usize) * 4;
            self.0[i..i + 4].copy_from_slice(&rgba);
        }
    }

    fn stamp(&mut self, art: &[&[u8]], x: i32, y: i32, flip: bool) {
        for (row, pixels) in art.iter().enumerate() {
            for (column, &color) in pixels.iter().enumerate() {
                let rgba = match color {
                    b'X' => INK,
                    b'd' => STRIPE,
                    b's' => SHADOW,
                    b'g' => FUR,
                    b'h' => LIGHT,
                    b'p' => BLUSH,
                    _ => continue,
                };
                let dx = if flip { pixels.len() - 1 - column } else { column };
                self.pixel(x + dx as i32, y + row as i32, rgba);
            }
        }
    }

    fn head(&mut self, x: i32, y: i32, eyes_open: bool, mouth_open: bool, state: &str) {
        self.stamp(HEAD, x, y, false);
        let mut dot = |dx, dy, color| self.pixel(x + dx, y + dy, color);
        for eye_x in [5, 13] {
            let marks: &[(i32, i32)] = match state {
                "dizzy" => &[(1, 0), (2, 0), (0, 1), (2, 1), (0, 2), (1, 2)],
                "hurt" if eye_x == 5 => &[(0, 0), (1, 1), (0, 2)],
                "hurt" => &[(1, 0), (0, 1), (1, 2)],
                _ if eyes_open => &[(0, 0), (1, 0), (0, 1), (1, 1)],
                _ => &[(0, 1), (1, 1), (2, 1)],
            };
            for &(dx, dy) in marks {
                dot(eye_x + dx, 8 + dy, EYE);
            }
        }
        for (dx, dy) in [(9, 10), (10, 10), (9, 11), (10, 11), (8, 12), (11, 12)] {
            dot(dx, dy, EYE);
        }
        if mouth_open {
            dot(9, 12, TONGUE);
            dot(10, 12, TONGUE);
        }
    }

    fn face_only(&mut self, eyes_open: bool) {
        let mut portrait = Self::new();
        portrait.head(0, 0, eyes_open, false, "peek");
        const W: usize = 30;
        const H: usize = 23;
        for y in 0..H {
            let sy = (2 * y + 1) * HEAD.len() / (2 * H);
            for x in 0..W {
                let sx = (2 * x + 1) * HEAD[0].len() / (2 * W);
                let src = (sy * SIZE + sx) * 4;
                let dst = ((y + 4) * SIZE + x + 1) * 4;
                self.0[dst..dst + 4].copy_from_slice(&portrait.0[src..src + 4]);
            }
        }
    }

    fn pose(&mut self, state: &str, frame: u32, eyes_open: bool) {
        let step = (frame % 2) as i32;
        let tail = if step == 0 { TAIL_UP } else { TAIL_DOWN };
        match state {
            "peek" => self.face_only(eyes_open),
            "drag" | "dizzy" | "hurt" => {
                let sway = step * 2 - 1;
                self.stamp(TAIL_UP, 16 + sway, 15, false);
                self.stamp(HELD, 7 + sway, 13, false);
                for x in [8, 17] {
                    self.stamp(PAW, x + sway, 25, false);
                }
                for x in [4, 22] {
                    self.stamp(PAW, x + sway, 16, false);
                }
                self.head(5, 2, eyes_open, true, state);
            }
            "walk" => {
                self.stamp(tail, 0, 14, true);
                for (index, x) in [4, 9, 17, 22].iter().enumerate() {
                    let lifted = i32::from(matches!(index, 0 | 3) == (step == 1));
                    self.stamp(PAW, x + lifted, 26 - lifted, false);
                }
                self.stamp(WALKING, 3, 17, false);
                self.head(11, 8 + step, eyes_open, false, state);
            }
            "loaf" | "sleep" => {
                self.stamp(tail, 0, 17, true);
                self.stamp(LOAF, 3, 20, false);
                let asleep = state == "sleep";
                self.head(10, if asleep { 14 + step } else { 11 }, eyes_open && !asleep, false, state);
            }
            _ => {
                self.stamp(tail, 21, 16, false);
                self.stamp(SITTING, 1, 11, false);
                for x in [7, 16] {
                    self.stamp(PAW, x, 26, false);
                }
                self.head(3, 2, eyes_open, state == "talk" && step == 1, state);
            }
        }
    }
}

/// 상태·프레임으로 32×32 RGBA를 만든다.
pub fn draw(state: &str, frame: u32, eyes_open: bool) -> Vec<u8> {
    let mut canvas = Canvas::new();
    canvas.pose(state, frame, eyes_open);
    canvas.0
}

#[cfg(target_arch = "wasm32")]
mod canvas {
    use wasm_bindgen::{Clamped, JsCast};
    use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, ImageData};

    thread_local! {
        static OFF: HtmlCanvasElement = {
            let c: HtmlCanvasElement = crate::dom::document().create_element("canvas").unwrap().unchecked_into();
            c.set_width(32);
            c.set_height(32);
            c
        };
    }

    pub fn ctx(c: &HtmlCanvasElement) -> CanvasRenderingContext2d {
        c.get_context("2d").unwrap().unwrap().unchecked_into()
    }

    /// 32x32 그림을 대상 캔버스에 도트가 뭉개지지 않게 확대해서 그린다 (facing < 0 이면 좌우 반전)
    pub fn render(target: &HtmlCanvasElement, state: &str, frame: u32, facing: i32, eyes_open: bool) {
        let px = super::draw(state, frame, eyes_open);
        OFF.with(|off| {
            if let Ok(img) = ImageData::new_with_u8_clamped_array_and_sh(Clamped(&px), 32, 32) {
                let _ = ctx(off).put_image_data(&img, 0.0, 0.0);
            }
            let t = ctx(target);
            t.set_image_smoothing_enabled(false);
            let (w, h) = (target.width() as f64, target.height() as f64);
            t.clear_rect(0.0, 0.0, w, h);
            t.save();
            if facing < 0 {
                let _ = t.translate(w, 0.0);
                let _ = t.scale(-1.0, 1.0);
            }
            let _ = t.draw_image_with_html_canvas_element_and_dw_and_dh(off, 0.0, 0.0, w, h);
            t.restore();
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use canvas::{ctx, render};



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_draws_something() {
        for state in ["sit", "walk", "loaf", "sleep", "drag", "talk", "peek", "dizzy", "hurt"] {
            let px = draw(state, 1, true);
            assert_eq!(px.len(), SIZE * SIZE * 4);
            assert!(px.chunks_exact(4).filter(|p| p[3] > 0).count() > 150, "{state}");
        }
    }

    #[test]
    fn notification_and_gestures_have_their_own_sprites() {
        let peek = draw("peek", 0, true);
        assert!(peek[(28 * SIZE * 4)..].iter().all(|b| *b == 0));
        assert_ne!(peek, draw("peek", 0, false));
        assert_ne!(draw("drag", 1, true), draw("dizzy", 1, true));
        assert_ne!(draw("hurt", 1, true), draw("dizzy", 1, true));
        assert_ne!(draw("walk", 0, true), draw("walk", 1, true));
    }
}
