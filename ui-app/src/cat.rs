//! 바탕화면의 사이와 말풍선. 투명한 여백에서는 아래 창이 마우스를 받는다.
use crate::api::{cat_event, listen};
use crate::dom::*;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::{
    CanvasRenderingContext2d, DomRect, HtmlCanvasElement, HtmlImageElement, PointerEvent,
};

const MASK_SIZE: u32 = 32;
const HIT_ALPHA: u8 = 24;

#[derive(Default)]
struct Desktop {
    mask: Vec<Span>,
    left: bool,
    bubble: Option<Value>,
    reaction_until: f64,
    last_regions: Vec<[f64; 4]>,
}

struct Span {
    y: u32,
    start: u32,
    end: u32,
}

impl Span {
    fn rect(&self, bounds: &DomRect, left: bool) -> [f64; 4] {
        let step_x = bounds.width() / f64::from(MASK_SIZE);
        let step_y = bounds.height() / f64::from(MASK_SIZE);
        let start = if left {
            MASK_SIZE - self.end
        } else {
            self.start
        };
        [
            bounds.left() + f64::from(start) * step_x,
            bounds.top() + f64::from(self.y) * step_y,
            f64::from(self.end - self.start) * step_x,
            step_y,
        ]
    }
}

/// Read the bundled image once, using row spans to leave gaps between flame tips clickable through.
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
    let flame: HtmlImageElement = by_id("flame").unwrap().unchecked_into();
    let bubble = by_id("bubble").unwrap();
    let zz = by_id("zz").unwrap();
    let reaction = by_id("reaction").unwrap();
    let desktop = Rc::new(RefCell::new(Desktop::default()));

    if flame.complete() {
        desktop.borrow_mut().mask = sample_mask(&flame).unwrap_or_default();
    } else {
        let (image, desktop) = (flame.clone(), desktop.clone());
        on(&flame, "load", move |_| {
            desktop.borrow_mut().mask = sample_mask(&image).unwrap_or_default();
        });
    }

    {
        let (desktop, cat) = (desktop.clone(), cat.clone());
        listen("cat:reaction", move |event| {
            let (pose, line) = match event["pose"].as_str() {
                Some("dizzy") => ("dizzy", "어지러..."),
                Some("hurt") => ("hurt", "아야!"),
                _ => return,
            };
            let until = now() + 1300.0;
            desktop.borrow_mut().reaction_until = until;
            let _ = cat.set_attribute("data-reaction", pose);
            reaction.set_text_content(Some(line));
            toggle(
                &reaction,
                "below",
                event["below"].as_bool().unwrap_or(false),
            );
            toggle(&reaction, "left", event["left"].as_bool().unwrap_or(false));
            reaction.set_hidden(false);

            let (desktop, cat, reaction) = (desktop.clone(), cat.clone(), reaction.clone());
            after(1300, move || {
                if desktop.borrow().reaction_until == until {
                    let _ = cat.remove_attribute("data-reaction");
                    reaction.set_hidden(true);
                }
            });
        });
    }
    {
        let (desktop, cat) = (desktop.clone(), cat.clone());
        listen("cat:state", move |event| {
            let state = event["state"].as_str().unwrap_or("sit");
            let left = event["facing"].as_i64().unwrap_or(1) < 0;
            desktop.borrow_mut().left = left;
            let _ = cat.set_attribute("data-state", state);
            toggle(&cat, "facing-left", left);
            toggle(&zz, "hidden", state != "sleep");
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
        let (desktop, flame, bubble) = (desktop.clone(), flame.clone(), bubble.clone());
        every(150, move || {
            let mut desktop = desktop.borrow_mut();
            let bounds = flame.get_bounding_client_rect();
            let mut regions = desktop
                .mask
                .iter()
                .map(|span| {
                    let [x, y, w, h] = span.rect(&bounds, desktop.left);
                    [
                        x.floor(),
                        y.floor(),
                        (x + w).ceil() - x.floor(),
                        (y + h).ceil() - y.floor(),
                    ]
                })
                .collect::<Vec<_>>();
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
                desktop.last_regions = regions;
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
            if !pointer.is_primary() {
                return;
            }
            let desktop = desktop.borrow();
            let bounds = flame.get_bounding_client_rect();
            let px = f64::from(pointer.client_x());
            let py = f64::from(pointer.client_y());
            if !desktop.mask.iter().any(|span| {
                let [x, y, w, h] = span.rect(&bounds, desktop.left);
                px >= x && px < x + w && py >= y && py < y + h
            }) {
                return;
            }
            if pointer.button() == 2 {
                event.prevent_default();
                cat_event("context-menu", Value::Null);
            } else if pointer.button() == 0 && holding.get().is_none() {
                event.prevent_default();
                let _ = target.set_pointer_capture(pointer.pointer_id());
                holding.set(Some(pointer.pointer_id()));
                cat_event("drag-start", Value::Null);
            }
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
    on(&cat, "contextmenu", |event| event.prevent_default());
    cat_event("ready", Value::Null);
}
