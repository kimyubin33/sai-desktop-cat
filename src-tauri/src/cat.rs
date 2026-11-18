//! 바탕화면 고양이 창: 작은 투명 창 자체가 화면 아래쪽(작업 표시줄 위)을 따라 움직인다.
//! 투명한 부분은 클릭이 아래 창으로 통과하고, 고양이 그림과 말풍선 위에서만 마우스를 받는다.
//! (마우스가 그 위에 있는지는 앱이 커서 위치를 읽어 직접 판단한다)
use crate::app::App;
use sai_core::behavior::CatBehavior;
use sai_core::time::now_ms;
use sai_core::util::J;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub const WIN_W: f64 = 280.0;
pub const WIN_H: f64 = 280.0;
const TICK_MS: u64 = 33;
const BUBBLE_MS: i64 = 15_000;

#[derive(Clone, Copy, Debug, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

struct Drag {
    cursor: (f64, f64),
    win: (f64, f64),
    at: i64,
    moved: bool,
}

pub struct CatState {
    behavior: CatBehavior,
    /// 지금 모니터의 작업 영역 (물리 픽셀) 과 배율
    area: (f64, f64, f64, f64),
    scale: f64,
    floor_y: f64,
    drag: Option<Drag>,
    pub bubble: Option<Value>,
    bubble_until: i64,
    bubble_hovered: bool,
    last_sent: &'static str,
    interactive: bool,
    regions: Vec<Rect>,
    pub visible: bool,
    last_tick: i64,
}

impl CatState {
    pub fn new() -> CatState {
        CatState {
            behavior: CatBehavior::new(0.0, 800.0, 400.0),
            area: (0.0, 0.0, 1280.0, 720.0),
            scale: 1.0,
            floor_y: 0.0,
            drag: None,
            bubble: None,
            bubble_until: 0,
            bubble_hovered: false,
            last_sent: "",
            interactive: false,
            regions: vec![],
            visible: true,
            last_tick: now_ms(),
        }
    }

    fn win_size(&self) -> (f64, f64) {
        (WIN_W * self.scale, WIN_H * self.scale)
    }

    /// 고양이 창이 다닐 수 있는 가로 범위와 바닥 높이
    fn apply_area(&mut self, area: (f64, f64, f64, f64), scale: f64) {
        self.area = area;
        self.scale = scale;
        let (w, h) = self.win_size();
        self.floor_y = area.1 + area.3 - h;
        self.behavior.set_bounds(area.0, area.0 + area.2 - w);
    }

    pub fn is_busy(&self) -> bool {
        self.drag.is_some() || self.bubble.is_some()
    }
}

fn win(app: &tauri::AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("cat")
}

fn monitor_area(m: &tauri::Monitor) -> (f64, f64, f64, f64) {
    let wa = m.work_area();
    (wa.position.x as f64, wa.position.y as f64, wa.size.width as f64, wa.size.height as f64)
}

pub fn create(app: &tauri::AppHandle, a: &Arc<App>) -> tauri::Result<()> {
    let saved = a.stores.settings.read(|s| s["position"].clone());
    let monitors = app.available_monitors().unwrap_or_default();
    let mon = monitors
        .iter()
        .find(|m| m.name().map(|n| *n == saved.s("display")).unwrap_or(false))
        .cloned()
        .or_else(|| app.primary_monitor().ok().flatten())
        .or_else(|| monitors.first().cloned());
    {
        let mut c = a.cat.lock().unwrap();
        if let Some(m) = &mon {
            c.apply_area(monitor_area(m), m.scale_factor());
        }
        let (min_x, max_x) = (c.behavior.min_x, c.behavior.max_x);
        let x = saved.on("x").map(|x| x.clamp(min_x, max_x)).unwrap_or(min_x + (max_x - min_x) * 0.7);
        c.behavior = CatBehavior::new(min_x, max_x, x);
        c.behavior.calm = a.stores.settings.read(|s| s.i("quietUntil")) > now_ms();
    }
    let builder = WebviewWindowBuilder::new(app, "cat", WebviewUrl::App("cat/index.html".into()))
        .title("사이")
        .inner_size(WIN_W, WIN_H)
        .transparent(true)
        .decorations(false)
        .resizable(false)
        .skip_taskbar(true)
        .shadow(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .focusable(false)
        .focused(false)
        .visible(false);
    #[cfg(windows)]
    let builder = if let Some(dir) = std::env::var_os("SAI_PREVIEW_DIR") {
        builder.data_directory(std::path::PathBuf::from(dir).join("webview"))
    } else {
        builder
    };
    let w = builder.build()?;
    place(&w, &a.cat.lock().unwrap());
    let _ = w.show();
    // 창이 화면에 나온 뒤에 클릭 통과를 켠다 (Linux 에서는 그 전에 켜면 멈춘다)
    let _ = w.set_ignore_cursor_events(true);

    // 움직임과 마우스 판정은 33ms 마다
    let app2 = app.clone();
    let a2 = a.clone();
    tauri::async_runtime::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_millis(TICK_MS));
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            iv.tick().await;
            tick(&app2, &a2, &w);
        }
    });
    Ok(())
}

fn place(w: &WebviewWindow, c: &CatState) {
    place_at(w, c.behavior.x, c.floor_y);
}

fn place_at(w: &WebviewWindow, x: f64, y: f64) {
    let _ = w.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
}

fn tick(app: &tauri::AppHandle, a: &Arc<App>, w: &WebviewWindow) {
    let now = now_ms();
    let mut timeout_bubble: Option<String> = None;
    {
        let mut c = a.cat.lock().unwrap();
        let dt = (now - c.last_tick).clamp(0, 100) as f64;
        c.last_tick = now;
        if !c.visible {
            return;
        }
        let cursor = app.cursor_position().ok().map(|p| (p.x, p.y));

        let scale = c.scale;
        if let Some(drag) = &mut c.drag {
            if let Some((cx, cy)) = cursor {
                let (dx, dy) = (cx - drag.cursor.0, cy - drag.cursor.1);
                let position = (drag.win.0 + dx, drag.win.1 + dy);
                let started = !drag.moved && dx.hypot(dy) > 5.0 * scale;
                drag.moved |= started;
                let moved = drag.moved;
                if started {
                    hide_bubble_locked(app, &mut c);
                    c.behavior.start_drag();
                }
                if moved {
                    place_at(w, position.0, position.1);
                }
            }
        } else if c.behavior.tick(dt) {
            place(w, &c);
        }

        // 말풍선은 15초 뒤 저절로 닫힌다 (마우스를 올려 두면 기다린다)
        if c.bubble.is_some() && now > c.bubble_until {
            if c.bubble_hovered {
                c.bubble_until = now + 2000;
            } else {
                timeout_bubble = c.bubble.as_ref().map(|b| b.s("id"));
                hide_bubble_locked(app, &mut c);
            }
        }

        // 마우스가 고양이·말풍선 위에 있을 때만 클릭을 받는다
        if let Some((cx, cy)) = cursor {
            if let Ok(pos) = w.outer_position() {
                let (rx, ry) = ((cx - pos.x as f64) / scale, (cy - pos.y as f64) / scale);
                let over = c.regions.iter().any(|r| rx >= r.x && rx <= r.x + r.w && ry >= r.y && ry <= r.y + r.h);
                let want = over || c.drag.is_some();
                if want != c.interactive {
                    c.interactive = want;
                    let _ = w.set_ignore_cursor_events(!want);
                }
            }
        }
        send_state(app, &mut c, false);
    }
    if let Some(id) = timeout_bubble {
        a.advisor.handle_action(a, &id, "timeout");
    }
}

fn send_state(app: &tauri::AppHandle, c: &mut CatState, force: bool) {
    let state = c.behavior.state;
    if !force && state == c.last_sent {
        return;
    }
    c.last_sent = state;
    let _ = app.emit_to("cat", "cat:state", json!({ "state": state }));
}

fn save_position(a: &Arc<App>) {
    let (x, display) = {
        let c = a.cat.lock().unwrap();
        (c.behavior.x.round(), c.area)
    };
    let name = a.handle.monitor_from_point(display.0 + 1.0, display.1 + 1.0).ok().flatten().and_then(|m| m.name().cloned()).unwrap_or_default();
    a.stores.settings.update(|s| s["position"] = json!({ "x": x, "display": name }));
}

// ---------- 고양이 창에서 오는 신호 ----------
pub fn regions(a: &Arc<App>, list: &Value) {
    let rs: Vec<Rect> = list
        .as_array()
        .map(|v| v.iter().map(|r| Rect { x: r.n("x"), y: r.n("y"), w: r.n("w"), h: r.n("h") }).collect())
        .unwrap_or_default();
    a.cat.lock().unwrap().regions = rs;
}

pub fn drag_start(app: &tauri::AppHandle, a: &Arc<App>) {
    let Some(w) = win(app) else { return };
    let Ok(pos) = w.outer_position() else { return };
    let Ok(cur) = app.cursor_position() else { return };
    let mut c = a.cat.lock().unwrap();
    c.drag = Some(Drag {
        cursor: (cur.x, cur.y),
        win: (pos.x as f64, pos.y as f64),
        at: now_ms(),
        moved: false,
    });
}

pub fn drag_end(app: &tauri::AppHandle, a: &Arc<App>, cancelled: bool) {
    let window = win(app);
    let (clicked, moved) = {
        let mut c = a.cat.lock().unwrap();
        let Some(mut d) = c.drag.take() else { return };
        let cursor = app.cursor_position().ok();
        let released_x = if let Some(cur) = cursor.as_ref() {
            let (dx, dy) = (cur.x - d.cursor.0, cur.y - d.cursor.1);
            d.moved |= dx.hypot(dy) > 5.0 * c.scale;
            d.win.0 + dx
        } else {
            window.as_ref()
                .and_then(|w| w.outer_position().ok())
                .map_or(d.win.0, |pos| pos.x as f64)
        };
        let clicked = !cancelled && !d.moved && now_ms() - d.at < 600;
        if d.moved {
            // 커서가 있는 모니터로 옮겨간다
            if let Some(cur) = cursor {
                if let Ok(Some(m)) = app.monitor_from_point(cur.x, cur.y) {
                    c.apply_area(monitor_area(&m), m.scale_factor());
                }
            }
            hide_bubble_locked(app, &mut c);
            c.behavior.end_drag(released_x);
            if let Some(w) = &window {
                place(w, &c);
            }
        }
        (clicked, d.moved)
    };
    if moved {
        save_position(a);
    } else if clicked {
        crate::app::open_chat(app, a, "chat");
    }
}

pub fn bubble_hover(a: &Arc<App>, on: bool) {
    a.cat.lock().unwrap().bubble_hovered = on;
}

pub fn bubble_action(app: &tauri::AppHandle, a: &Arc<App>, id: &str, action: &str) {
    {
        let mut c = a.cat.lock().unwrap();
        if c.bubble.as_ref().map(|b| b.s("id") != id).unwrap_or(true) {
            return;
        }
        hide_bubble_locked(app, &mut c);
    }
    a.advisor.handle_action(a, id, action);
}

// ---------- 말풍선 ----------
pub fn show_bubble(app: &tauri::AppHandle, a: &Arc<App>, b: Value) -> bool {
    let mut c = a.cat.lock().unwrap();
    if c.drag.is_some() || !c.visible {
        return false;
    }
    c.bubble_until = now_ms() + BUBBLE_MS;
    c.bubble_hovered = false;
    c.behavior.start_talking();
    send_state(app, &mut c, true);
    let _ = app.emit_to("cat", "cat:bubble", &b);
    c.bubble = Some(b);
    true
}

fn hide_bubble_locked(app: &tauri::AppHandle, c: &mut CatState) {
    if c.bubble.is_none() {
        return;
    }
    c.bubble = None;
    c.bubble_hovered = false;
    c.behavior.stop_talking();
    let _ = app.emit_to("cat", "cat:hide-bubble", json!(null));
}

pub fn hide_bubble(app: &tauri::AppHandle, a: &Arc<App>) {
    let mut c = a.cat.lock().unwrap();
    hide_bubble_locked(app, &mut c);
}

pub fn set_calm(a: &Arc<App>, on: bool) {
    a.cat.lock().unwrap().behavior.calm = on;
}

pub fn set_visible(app: &tauri::AppHandle, a: &Arc<App>, on: bool) {
    let mut c = a.cat.lock().unwrap();
    c.visible = on;
    if let Some(w) = win(app) {
        if on {
            let _ = w.show();
        } else {
            hide_bubble_locked(app, &mut c);
            let _ = w.hide();
        }
    }
}

pub fn resend_state(app: &tauri::AppHandle, a: &Arc<App>) {
    let mut c = a.cat.lock().unwrap();
    send_state(app, &mut c, true);
    if let Some(b) = &c.bubble {
        let _ = app.emit_to("cat", "cat:bubble", b);
    }
}
