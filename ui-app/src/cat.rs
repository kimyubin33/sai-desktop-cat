//! 고양이 창: 도트 고양이를 그리고, 말풍선을 띄우고, 클릭·끌기·오른쪽 클릭을 앱에 알린다.
//! 투명한 부분은 클릭이 아래 창으로 통과하고, 고양이 그림과 말풍선 위에서만 마우스를 받는다.
//! 마우스가 그 위에 있는지는 앱이 판단하므로, 여기서는 "마우스를 받을 영역"만 알려 준다.
use crate::api::{cat_event, listen};
use crate::dom::*;
use crate::sprite;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, MouseEvent};

struct St {
    state: String,
    facing: i32,
    frame: u32,
    last_frame_at: f64,
    blink_until: f64,
    bubble: Option<Value>,
    last_regions: String,
}

fn frame_ms(state: &str) -> f64 {
    match state {
        "walk" => 160.0,
        "sit" => 700.0,
        "loaf" => 1200.0,
        "sleep" => 900.0,
        "talk" => 220.0,
        "drag" => 200.0,
        _ => 500.0,
    }
}

pub fn start() {
    let canvas: HtmlCanvasElement = by_id("cat").unwrap().unchecked_into();
    let bubble = by_id("bubble").unwrap();
    let zz = by_id("zz").unwrap();
    let st = Rc::new(RefCell::new(St { state: "sit".into(), facing: 1, frame: 0, last_frame_at: 0.0, blink_until: 0.0, bubble: None, last_regions: String::new() }));

    // 그리기: 매 프레임
    {
        let st = st.clone();
        let canvas = canvas.clone();
        let f: Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>> = Rc::new(RefCell::new(None));
        let g = f.clone();
        *g.borrow_mut() = Some(Closure::new(move |t: f64| {
            {
                let mut s = st.borrow_mut();
                if t - s.last_frame_at > frame_ms(&s.state) {
                    s.frame += 1;
                    s.last_frame_at = t;
                    if (s.state == "sit" || s.state == "loaf") && js_sys::Math::random() < 0.08 {
                        s.blink_until = t + 160.0;
                    }
                }
                sprite::render(&canvas, &s.state, s.frame, s.facing, t >= s.blink_until);
            }
            let _ = window().request_animation_frame(f.borrow().as_ref().unwrap().as_ref().unchecked_ref());
        }));
        let _ = window().request_animation_frame(g.borrow().as_ref().unwrap().as_ref().unchecked_ref());
    }

    // 앱 → 상태
    {
        let st = st.clone();
        let zz = zz.clone();
        let canvas = canvas.clone();
        listen("cat:state", move |s| {
            let mut x = st.borrow_mut();
            let state = s["state"].as_str().unwrap_or("sit").to_string();
            if state != x.state {
                x.frame = 0;
            }
            x.state = state.clone();
            x.facing = s["facing"].as_i64().unwrap_or(1) as i32;
            toggle(&zz, "hidden", state != "sleep");
            toggle(&canvas, "dragging", state == "drag");
        });
    }
    {
        let st = st.clone();
        let bubble = bubble.clone();
        listen("cat:bubble", move |b| {
            q_in(&bubble, ".text").unwrap().set_text_content(Some(b["text"].as_str().unwrap_or("")));
            q_in(&bubble, ".source").unwrap().set_text_content(Some(b["sourceLabel"].as_str().unwrap_or("")));
            let actions = q_in(&bubble, ".actions").unwrap();
            clear(&actions);
            for a in b["actions"].as_array().cloned().unwrap_or_default() {
                let btn = el("button").attr("data-action", a["id"].as_str().unwrap_or("")).text(a["label"].as_str().unwrap_or(""));
                let btn = if a["primary"].as_bool().unwrap_or(false) { btn.cls("primary") } else { btn };
                let _ = actions.append_child(&btn.0);
            }
            toggle(&bubble, "hidden", false);
            st.borrow_mut().bubble = Some(b);
        });
    }
    {
        let st = st.clone();
        let bubble = bubble.clone();
        listen("cat:hide-bubble", move |_| {
            toggle(&bubble, "hidden", true);
            st.borrow_mut().bubble = None;
        });
    }

    // 마우스를 받을 영역 알리기 (0.15초마다, 바뀌었을 때만)
    {
        let st = st.clone();
        let canvas = canvas.clone();
        let bubble = bubble.clone();
        every(150, move || {
            let mut list = cat_rects(&canvas, &st.borrow());
            if !has_class(&bubble, "hidden") {
                let b = bubble.get_bounding_client_rect();
                list.push(json!({ "x": b.left(), "y": b.top(), "w": b.width(), "h": b.height() }));
            }
            let key: String = list.iter().map(|r| format!("{},{},{},{};", r["x"].as_f64().unwrap_or(0.0).round(), r["y"].as_f64().unwrap_or(0.0).round(), r["w"].as_f64().unwrap_or(0.0).round(), r["h"].as_f64().unwrap_or(0.0).round())).collect();
            let mut s = st.borrow_mut();
            if key != s.last_regions {
                s.last_regions = key;
                cat_event("regions", json!(list));
            }
        });
    }

    bubble.clone().dyn_into::<web_sys::EventTarget>().map(|t| {
        on(&t, "mouseenter", |_| cat_event("bubble-hover", json!(true)));
        on(&t, "mouseleave", |_| cat_event("bubble-hover", json!(false)));
        let st = st.clone();
        on(&t, "click", move |e| {
            let Some(target) = event_target(&e) else { return };
            let Ok(Some(btn)) = target.closest("button[data-action]") else { return };
            let s = st.borrow();
            let Some(b) = &s.bubble else { return };
            cat_event("bubble-action", json!({ "id": b["id"], "action": btn.get_attribute("data-action").unwrap_or_default() }));
        });
    }).ok();

    {
        let st = st.clone();
        let c2 = canvas.clone();
        on(&canvas, "mousedown", move |e| {
            let me: &MouseEvent = e.unchecked_ref();
            if !hit_cat(&c2, &st.borrow(), me) {
                return;
            }
            // 오른쪽 버튼은 눌렀을 때 바로 메뉴 (contextmenu 이벤트는 투명 창에서 오지 않을 때가 있다)
            if me.button() == 2 {
                e.prevent_default();
                cat_event("context-menu", Value::Null);
                return;
            }
            if me.button() != 0 {
                return;
            }
            e.prevent_default();
            cat_event("drag-start", Value::Null);
        });
    }
    on(&window(), "mouseup", |e| {
        let me: &MouseEvent = e.unchecked_ref();
        if me.button() == 0 {
            cat_event("drag-end", Value::Null);
        }
    });
    on(&canvas, "contextmenu", |e| e.prevent_default());
    cat_event("ready", Value::Null);
}

fn pixels(s: &St) -> Vec<u8> {
    sprite::draw(&s.state, s.frame, true)
}

/// 32x32 도트에서 그려진 칸 (좌우 반전 반영)
fn opaque(px: &[u8], x: usize, y: usize, facing: i32) -> bool {
    let xx = if facing < 0 { sprite::SIZE - 1 - x } else { x };
    px[(y * sprite::SIZE + xx) * 4 + 3] > 30
}

/// 그려진 픽셀이 있는 가로줄 묶음 → 창 기준 사각형 몇 개
fn cat_rects(canvas: &HtmlCanvasElement, s: &St) -> Vec<Value> {
    let px = pixels(s);
    let r = canvas.get_bounding_client_rect();
    let cell_w = r.width() / sprite::SIZE as f64;
    let cell_h = r.height() / sprite::SIZE as f64;
    let mut out = vec![];
    for y0 in (0..sprite::SIZE).step_by(3) {
        let (mut min_x, mut max_x) = (usize::MAX, 0usize);
        for y in y0..(y0 + 3).min(sprite::SIZE) {
            for x in 0..sprite::SIZE {
                if opaque(&px, x, y, s.facing) {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                }
            }
        }
        if min_x != usize::MAX {
            out.push(json!({ "x": r.left() + (min_x as f64 - 0.5) * cell_w, "y": r.top() + y0 as f64 * cell_h, "w": (max_x - min_x + 2) as f64 * cell_w, "h": 3.0 * cell_h }));
        }
    }
    out
}

fn hit_cat(canvas: &HtmlCanvasElement, s: &St, e: &MouseEvent) -> bool {
    let r = canvas.get_bounding_client_rect();
    let x = ((e.client_x() as f64 - r.left()) / r.width() * sprite::SIZE as f64).floor() as i64;
    let y = ((e.client_y() as f64 - r.top()) / r.height() * sprite::SIZE as f64).floor() as i64;
    let px = pixels(s);
    // 가장자리 한 칸 여유
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (xx, yy) = (x + dx, y + dy);
            if (0..sprite::SIZE as i64).contains(&xx) && (0..sprite::SIZE as i64).contains(&yy) && opaque(&px, xx as usize, yy as usize, s.facing) {
                return true;
            }
        }
    }
    false
}
