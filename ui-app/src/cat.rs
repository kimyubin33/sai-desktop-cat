//! 바탕화면의 사이와 말풍선. 투명한 여백에서는 아래 창이 마우스를 받는다.
use crate::api::{cat_event, listen};
use crate::dom::*;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, HtmlImageElement, PointerEvent};

const MASK_SIZE: u32 = 32;
const HIT_ALPHA: u8 = 24;

#[derive(Default)]
struct Desktop {
    mask: Vec<Span>,
    bubble: Option<Value>,
    last_regions: Vec<[f64; 4]>,
}

struct Span {
    y: u32,
    start: u32,
    end: u32,
}

impl Span {
    fn rect(&self, [left, top, step_x, step_y]: [f64; 4]) -> [f64; 4] {
        [
            left + f64::from(self.start) * step_x,
            top + f64::from(self.y) * step_y,
            f64::from(self.end - self.start) * step_x,
            step_y,
        ]
    }
}

fn mask_grid(image: &HtmlImageElement) -> [f64; 4] {
    let bounds = image.get_bounding_client_rect();
    let size = f64::from(MASK_SIZE);
    [
        bounds.left(),
        bounds.top(),
        bounds.width() / size,
        bounds.height() / size,
    ]
}

/// Read the character image once so clicks pass through its transparent edges.
fn sample_mask(image: &HtmlImageElement) -> Option<Vec<Span>> {
    let canvas: HtmlCanvasElement = document().create_element("canvas").ok()?.dyn_into().ok()?;
    canvas.set_width(MASK_SIZE);
    canvas.set_height(MASK_SIZE);
    let ctx: CanvasRenderingContext2d = canvas.get_context("2d").ok()??.dyn_into().ok()?;
    let size = f64::from(MASK_SIZE);
    ctx.draw_image_with_html_image_element_and_dw_and_dh(image, 0.0, 0.0, size, size)
        .ok()?;
    let rgba = ctx.get_image_data(0.0, 0.0, size, size).ok()?.data().0;

    let mut spans = Vec::new();
    for y in 0..MASK_SIZE {
        let mut start = None;
        for x in 0..=MASK_SIZE {
            let visible =
                x < MASK_SIZE && rgba[((y * MASK_SIZE + x) * 4 + 3) as usize] >= HIT_ALPHA;
            if visible {
                start.get_or_insert(x);
            } else if let Some(start) = start.take() {
                spans.push(Span { y, start, end: x });
            }
        }
    }
    Some(spans)
}

pub fn start() {
    let cat = by_id("cat").unwrap();
    let mark: HtmlImageElement = by_id("companionMark").unwrap().unchecked_into();
    let bubble = by_id("bubble").unwrap();
    let desktop = Rc::new(RefCell::new(Desktop::default()));

    if mark.complete() {
        desktop.borrow_mut().mask = sample_mask(&mark).unwrap_or_default();
    } else {
        let (image, desktop) = (mark.clone(), desktop.clone());
        on(&mark, "load", move |_| {
            desktop.borrow_mut().mask = sample_mask(&image).unwrap_or_default();
        });
    }

    {
        let cat = cat.clone();
        listen("cat:state", move |event| {
            let state = event["state"].as_str().unwrap_or("sit");
            let _ = cat.set_attribute("data-state", state);
        });
    }
    {
        let (desktop, bubble) = (desktop.clone(), bubble.clone());
        listen("cat:bubble", move |b| {
            q_in(&bubble, ".text")
                .unwrap()
                .set_text_content(b["text"].as_str());
            q_in(&bubble, ".source")
                .unwrap()
                .set_text_content(b["sourceLabel"].as_str());
            let actions = q_in(&bubble, ".actions").unwrap();
            clear(&actions);
            for action in b["actions"].as_array().into_iter().flatten() {
                let button = el("button")
                    .attr("data-action", action["id"].as_str().unwrap_or(""))
                    .text(action["label"].as_str().unwrap_or(""));
                let button = if action["primary"].as_bool().unwrap_or(false) {
                    button.cls("primary")
                } else {
                    button
                };
                let _ = actions.append_child(&button.0);
            }
            toggle(&bubble, "hidden", false);
            desktop.borrow_mut().bubble = Some(b);
        });
    }
    {
        let (desktop, bubble) = (desktop.clone(), bubble.clone());
        listen("cat:hide-bubble", move |_| {
            toggle(&bubble, "hidden", true);
            desktop.borrow_mut().bubble = None;
        });
    }
    {
        let (desktop, mark, bubble) = (desktop.clone(), mark.clone(), bubble.clone());
        let mut regions = Vec::new();
        every(150, move || {
            let mut desktop = desktop.borrow_mut();
            let grid = mask_grid(&mark);
            regions.clear();
            regions.extend(desktop.mask.iter().map(|span| {
                let [x, y, w, h] = span.rect(grid);
                let (left, top) = (x.floor(), y.floor());
                [left, top, (x + w).ceil() - left, (y + h).ceil() - top]
            }));
            if !has_class(&bubble, "hidden") {
                let bounds = bubble.get_bounding_client_rect();
                regions.push([bounds.left(), bounds.top(), bounds.width(), bounds.height()]);
            }
            if regions != desktop.last_regions {
                cat_event(
                    "regions",
                    Value::Array(
                        regions
                            .iter()
                            .map(|&[x, y, w, h]| json!({ "x": x, "y": y, "w": w, "h": h }))
                            .collect(),
                    ),
                );
                std::mem::swap(&mut desktop.last_regions, &mut regions);
            }
        });
    }

    on(&bubble, "mouseenter", |_| {
        cat_event("bubble-hover", json!(true))
    });
    on(&bubble, "mouseleave", |_| {
        cat_event("bubble-hover", json!(false))
    });
    {
        let desktop = desktop.clone();
        on(&bubble, "click", move |event| {
            let Some(target) = event_target(&event) else {
                return;
            };
            let Ok(Some(button)) = target.closest("button[data-action]") else {
                return;
            };
            let desktop = desktop.borrow();
            let Some(bubble) = &desktop.bubble else {
                return;
            };
            cat_event(
                "bubble-action",
                json!({
                    "id": bubble["id"],
                    "action": button.get_attribute("data-action").unwrap_or_default(),
                }),
            );
        });
    }

    let holding = Rc::new(Cell::new(None));
    {
        let (target, holding) = (cat.clone(), holding.clone());
        on(&cat, "pointerdown", move |event| {
            let pointer: &PointerEvent = event.unchecked_ref();
            if !pointer.is_primary() || pointer.button() != 0 || holding.get().is_some() {
                return;
            }
            let desktop = desktop.borrow();
            let grid = mask_grid(&mark);
            let px = f64::from(pointer.client_x());
            let py = f64::from(pointer.client_y());
            if !desktop.mask.iter().any(|span| {
                let [x, y, w, h] = span.rect(grid);
                px >= x && px < x + w && py >= y && py < y + h
            }) {
                return;
            }
            event.prevent_default();
            let _ = target.set_pointer_capture(pointer.pointer_id());
            holding.set(Some(pointer.pointer_id()));
            cat_event("drag-start", Value::Null);
        });
    }
    for ev in ["pointerup", "pointercancel", "lostpointercapture"] {
        let holding = holding.clone();
        on(&window(), ev, move |event| {
            let pointer: &PointerEvent = event.unchecked_ref();
            if holding.get() == Some(pointer.pointer_id()) {
                holding.set(None);
                cat_event("drag-end", json!(ev != "pointerup"));
            }
        });
    }
    on(&document(), "contextmenu", |event| event.prevent_default());
    cat_event("ready", Value::Null);
}
