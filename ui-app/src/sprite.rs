//! 사이의 도트 스프라이트를 코드로 그린다 (32x32 도트, 오른쪽을 바라보는 기준).
//! 외부 그림 파일 없이 모양을 조절할 수 있도록 타원·삼각형 도형을 도트 단위로 찍는다.
//! 그림 계산은 순수 Rust(32x32 RGBA)이고, 마지막에 캔버스로 크게 옮긴다.

pub const SIZE: usize = 32;

type Rgba = [u8; 4];

const LINE: Rgba = [0x3b, 0x2f, 0x2f, 255];
const FUR: Rgba = [0xf2, 0xa6, 0x5a, 255];
const STRIPE: Rgba = [0xd6, 0x80, 0x3a, 255];
const LIGHT: Rgba = [0xfb, 0xe3, 0xc4, 255];
const PINK: Rgba = [0xf7, 0xb2, 0xb7, 255];
const EYE: Rgba = [0x2b, 0x2b, 0x2b, 255];
const SHINE: Rgba = [0xff, 0xff, 0xff, 255];
const MOUTH: Rgba = [0xc9, 0x6a, 0x6a, 255];

pub struct Canvas(pub Vec<u8>);

impl Canvas {
    fn new() -> Canvas {
        Canvas(vec![0; SIZE * SIZE * 4])
    }

    /// 캔버스의 fillRect(Math.round(x), Math.round(y), w, h) 와 같게
    fn px(&mut self, x: f64, y: f64, w: f64, h: f64, c: Rgba) {
        let x0 = x.round() as i64;
        let y0 = y.round() as i64;
        let w = w.round().max(1.0) as i64;
        let h = h.round().max(1.0) as i64;
        for yy in y0..y0 + h {
            for xx in x0..x0 + w {
                if (0..SIZE as i64).contains(&xx) && (0..SIZE as i64).contains(&yy) {
                    let i = (yy as usize * SIZE + xx as usize) * 4;
                    self.0[i..i + 4].copy_from_slice(&c);
                }
            }
        }
    }

    fn ellipse(&mut self, cx: f64, cy: f64, rx: i64, ry: i64, c: Rgba) {
        for y in -ry..=ry {
            for x in -rx..=rx {
                let (xf, yf) = (x as f64, y as f64);
                if xf * xf / ((rx * rx) as f64 + 0.3) + yf * yf / ((ry * ry) as f64 + 0.3) <= 1.0 {
                    self.px(cx + xf, cy + yf, 1.0, 1.0, c);
                }
            }
        }
    }

    /// 외곽선이 있는 타원
    fn blob(&mut self, cx: f64, cy: f64, rx: i64, ry: i64, fill: Rgba) {
        self.ellipse(cx, cy, rx + 1, ry + 1, LINE);
        self.ellipse(cx, cy, rx, ry, fill);
    }

    /// 귀: 꼭짓점(tx,ty) 아래로 넓어지는 삼각형
    fn ear(&mut self, tx: f64, ty: f64, h: i64) {
        for i in 0..=h {
            let f = i as f64;
            self.px(tx - f * 0.6 - 1.0, ty + f, f * 1.2 + 2.0, 1.0, LINE);
        }
        for i in 1..h {
            let f = i as f64;
            self.px(tx - f * 0.6, ty + f, (f * 1.2).max(1.0), 1.0, if i > 1 { FUR } else { LINE });
        }
        for i in 2..h - 1 {
            let f = i as f64;
            self.px(tx - f * 0.3 + 0.2, ty + f + 0.5, (f * 0.5).max(1.0), 1.0, PINK);
        }
    }

    fn head(&mut self, cx: f64, cy: f64, eyes_open: bool, mouth: bool) {
        self.ear(cx - 3.0, cy - 8.0, 5);
        self.ear(cx + 3.0, cy - 8.0, 5);
        self.blob(cx, cy, 6, 5, FUR);
        // 이마 줄무늬
        self.px(cx - 1.0, cy - 5.0, 1.0, 2.0, STRIPE);
        self.px(cx + 1.0, cy - 5.0, 1.0, 2.0, STRIPE);
        // 볼
        self.ellipse(cx + 1.0, cy + 3.0, 4, 1, LIGHT);
        if eyes_open {
            self.px(cx - 2.0, cy - 1.0, 2.0, 2.0, EYE);
            self.px(cx + 3.0, cy - 1.0, 2.0, 2.0, EYE);
            self.px(cx - 2.0, cy - 1.0, 1.0, 1.0, SHINE);
            self.px(cx + 3.0, cy - 1.0, 1.0, 1.0, SHINE);
        } else {
            // 감은 눈
            self.px(cx - 3.0, cy, 3.0, 1.0, EYE);
            self.px(cx + 3.0, cy, 3.0, 1.0, EYE);
        }
        self.px(cx + 1.0, cy + 1.0, 2.0, 1.0, PINK); // 코
        if mouth {
            self.px(cx + 1.0, cy + 3.0, 2.0, 1.0, LINE);
            self.px(cx + 1.0, cy + 2.0, 2.0, 1.0, MOUTH);
        } else {
            self.px(cx, cy + 2.0, 1.0, 1.0, LINE);
            self.px(cx + 3.0, cy + 2.0, 1.0, 1.0, LINE);
        }
        // 수염
        self.px(cx - 8.0, cy + 1.0, 3.0, 1.0, LINE);
        self.px(cx + 7.0, cy + 1.0, 3.0, 1.0, LINE);
    }

    fn tail(&mut self, points: &[(f64, f64)]) {
        for (x, y) in points {
            self.px(x - 1.0, y - 1.0, 3.0, 3.0, LINE);
        }
        for (x, y) in points {
            self.px(*x, *y, 1.0, 1.0, FUR);
        }
    }

    fn sit(&mut self, f: u32, eyes_open: bool, mouth: bool) {
        let sway = if f % 2 == 0 { 0.0 } else { 1.0 };
        self.tail(&[(6.0, 29.0), (5.0, 28.0), (4.0, 27.0 - sway), (4.0, 26.0 - sway), (5.0, 25.0 - sway), (7.0, 29.0), (8.0, 29.0), (9.0, 29.0)]);
        self.blob(14.0, 24.0, 7, 6, FUR);
        self.px(9.0, 21.0, 2.0, 1.0, STRIPE);
        self.px(8.0, 24.0, 2.0, 1.0, STRIPE);
        self.ellipse(16.0, 25.0, 3, 4, LIGHT);
        // 앞발
        self.px(13.0, 29.0, 3.0, 2.0, LINE);
        self.px(17.0, 29.0, 3.0, 2.0, LINE);
        self.px(14.0, 29.0, 1.0, 1.0, LIGHT);
        self.px(18.0, 29.0, 1.0, 1.0, LIGHT);
        self.head(16.0, 13.0, eyes_open, mouth);
    }

    fn walk(&mut self, f: u32) {
        let step = f % 2 == 1;
        self.tail(&[(6.0, 19.0), (5.0, 18.0), (4.0, 16.0), (4.0, 14.0), (5.0, 12.0), (6.0, 11.0)]);
        // 다리 (뒤 → 앞)
        let legs: [(f64, f64); 4] = if step { [(8.0, 0.0), (12.0, 1.0), (18.0, 1.0), (22.0, 0.0)] } else { [(9.0, 1.0), (11.0, 0.0), (19.0, 0.0), (21.0, 1.0)] };
        for (lx, up) in legs {
            self.px(lx - 1.0, 25.0 - up, 3.0, 6.0 + up, LINE);
            self.px(lx, 25.0 - up, 1.0, 5.0 + up, FUR);
        }
        self.blob(15.0, 21.0, 9, 4, FUR);
        self.px(10.0, 18.0, 1.0, 2.0, STRIPE);
        self.px(13.0, 18.0, 1.0, 2.0, STRIPE);
        self.px(16.0, 18.0, 1.0, 2.0, STRIPE);
        self.ellipse(16.0, 23.0, 6, 1, LIGHT);
        self.head(24.0, 13.0 + if step { 1.0 } else { 0.0 }, true, false);
    }

    fn loaf(&mut self, eyes_open: bool) {
        self.tail(&[(5.0, 29.0), (6.0, 29.0), (7.0, 29.0), (4.0, 28.0)]);
        self.blob(15.0, 25.0, 10, 5, FUR);
        self.px(9.0, 22.0, 1.0, 2.0, STRIPE);
        self.px(12.0, 21.0, 1.0, 2.0, STRIPE);
        self.px(15.0, 21.0, 1.0, 2.0, STRIPE);
        self.px(20.0, 29.0, 4.0, 2.0, LINE);
        self.px(21.0, 29.0, 2.0, 1.0, LIGHT);
        self.head(22.0, 18.0, eyes_open, false);
    }

    fn sleep(&mut self, f: u32) {
        self.tail(&[(23.0, 30.0), (22.0, 30.0), (21.0, 30.0), (20.0, 30.0), (19.0, 30.0), (18.0, 30.0)]);
        let b = (f % 2) as i64;
        self.blob(14.0, 26.0 - b as f64, 10, 4 + b, FUR);
        self.px(8.0, 24.0, 1.0, 2.0, STRIPE);
        self.px(11.0, 23.0, 1.0, 2.0, STRIPE);
        self.px(14.0, 23.0, 1.0, 2.0, STRIPE);
        self.head(21.0, 23.0, false, false);
    }

    fn drag(&mut self, f: u32) {
        let swing = if f % 2 == 1 { 1.0 } else { -1.0 };
        // 늘어진 몸과 다리
        self.px(11.0 + swing, 26.0, 3.0, 5.0, LINE);
        self.px(18.0 + swing, 26.0, 3.0, 5.0, LINE);
        self.px(12.0 + swing, 26.0, 1.0, 4.0, FUR);
        self.px(19.0 + swing, 26.0, 1.0, 4.0, FUR);
        self.tail(&[(16.0 + swing * 2.0, 28.0), (16.0 + swing * 2.0, 29.0), (17.0 + swing * 2.0, 30.0)]);
        self.blob(16.0, 21.0, 5, 6, FUR);
        self.ellipse(16.0, 22.0, 3, 4, LIGHT);
        self.px(9.0, 15.0, 3.0, 3.0, LINE); // 앞발
        self.px(21.0, 15.0, 3.0, 3.0, LINE);
        self.head(16.0, 10.0, true, true);
    }
}

/// 상태·프레임으로 32x32 RGBA 그림을 만든다
pub fn draw(state: &str, frame: u32, eyes_open: bool) -> Vec<u8> {
    let mut c = Canvas::new();
    match state {
        "walk" => c.walk(frame),
        "loaf" => c.loaf(eyes_open),
        "sleep" => c.sleep(frame),
        "drag" => c.drag(frame),
        "talk" => c.sit(0, true, frame % 2 == 0),
        _ => c.sit(frame, eyes_open, false),
    }
    c.0
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
    #[test]
    fn every_state_draws_something() {
        for s in ["sit", "walk", "loaf", "sleep", "drag", "talk"] {
            let px = super::draw(s, 1, true);
            assert!(px.chunks(4).filter(|p| p[3] > 0).count() > 150, "{s}");
        }
    }
}
