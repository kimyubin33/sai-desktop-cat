//! 팀 Drive 탭: 톡과 파일 공유를 따로 보여 준다.
//! 보낼 파일은 이 컴퓨터에서 고르거나(끌어다 놓기), 내 Google 드라이브·공유 문서함에서 고른다.
use super::{arr, b, fmt_clock, n, s, show_tab};
use crate::api::call;
use crate::dom::*;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use wasm_bindgen::JsCast;
use web_sys::HtmlElement;

const MAX_MB: f64 = 25.0;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Room {
    #[default]
    Chat,
    Files,
}

impl Room {
    fn status_id(self) -> &'static str {
        match self {
            Self::Chat => "driveChatStatus",
            Self::Files => "driveFilesStatus",
        }
    }

    fn draft_id(self) -> &'static str {
        match self {
            Self::Chat => "driveText",
            Self::Files => "driveCaption",
        }
    }
}

#[derive(Default)]
struct D {
    feed: Option<Value>,
    room: Room,
    poll: i32,
    // 고르기 창
    mode: String,
    path: Vec<(String, String)>, // (폴더 id, 이름)
    q: String,
}

thread_local! {
    static S: RefCell<D> = RefCell::new(D::default());
    static BUSY: Cell<bool> = Cell::new(false);
}

fn ds<R>(f: impl FnOnce(&mut D) -> R) -> R {
    S.with(|s| f(&mut s.borrow_mut()))
}

fn status(room: Room, text: &str, bad: bool) {
    if let Some(m) = by_id(room.status_id()) {
        m.set_text_content(Some(text));
        toggle(&m, "bad", bad);
    }
}

fn kind_label(mime: &str, name: &str) -> String {
    let m = match mime {
        "application/vnd.google-apps.document" => "문서",
        "application/vnd.google-apps.spreadsheet" => "시트",
        "application/vnd.google-apps.presentation" => "슬라이드",
        "application/vnd.google-apps.form" => "설문",
        "application/vnd.google-apps.folder" => "폴더",
        "application/pdf" => "PDF",
        _ if mime.starts_with("image/") => "이미지",
        _ if mime.starts_with("video/") => "영상",
        _ if mime.starts_with("audio/") => "소리",
        _ => "",
    };
    if !m.is_empty() {
        return m.into();
    }
    name.rsplit_once('.').map(|(_, e)| e.to_uppercase()).filter(|e| e.len() <= 5).unwrap_or_else(|| "파일".into())
}

fn size_text(n: f64) -> String {
    if n <= 0.0 {
        String::new()
    } else if n < 1024.0 * 1024.0 {
        format!("{}KB", (n / 1024.0).ceil() as i64)
    } else {
        format!("{:.1}MB", n / 1024.0 / 1024.0)
    }
}

fn day_key(ms: f64) -> String {
    let d = js_sys::Date::new(&ms.into());
    format!("{}년 {}월 {}일", d.get_full_year(), d.get_month() + 1, d.get_date())
}

fn open_url(url: String) {
    spawn(async move {
        if let Err(e) = call("app:open-url", json!(url)).await {
            status(Room::Files, &e, true);
        }
    });
}

fn edit_on() -> bool {
    by_id("driveEdit").map(|e| checked(&e)).unwrap_or(true)
}

fn draft(room: Room) -> String {
    by_id(room.draft_id()).map(|input| val(&input)).unwrap_or_default()
}

fn clear_sent_draft(room: Room, sent: &str) {
    if let Some(input) = by_id(room.draft_id()) {
        if val(&input) == sent {
            set_val(&input, "");
        }
    }
}

fn begin_request(room: Room) -> bool {
    if BUSY.with(|busy| busy.replace(true)) {
        status(room, "앞의 요청을 처리하는 중이에요…", false);
        return false;
    }
    true
}

fn received(v: Value, room: Room) {
    ds(|d| d.feed = Some(v));
    draw_feed(ds(|d| d.room == room));
}

fn send(room: Room, channel: &'static str, args: Value, sent: Option<String>, busy_text: &'static str, ok: &'static str) {
    if !begin_request(room) {
        return;
    }
    status(room, busy_text, false);
    spawn(async move {
        match call(channel, args).await {
            Ok(v) => {
                if let Some(text) = sent {
                    clear_sent_draft(room, &text);
                }
                received(v, room);
                status(room, ok, false);
            }
            Err(e) => status(room, &e, true),
        }
        BUSY.with(|b| b.set(false));
    });
}

// ---------- 탭 열기 · 주기적으로 새로 읽기 ----------
pub fn render() {
    spawn(refresh(true));
    let old = ds(|d| d.poll);
    stop(old);
    let id = every(5000, || {
        if super::current_tab() != "drive" {
            let p = ds(|d| d.poll);
            stop(p);
            return;
        }
        spawn(refresh(false));
    });
    ds(|d| d.poll = id);
}

async fn refresh(scroll: bool) {
    let Ok(v) = call("drive:feed", Value::Null).await else { return };
    // 바뀐 게 없으면 다시 그리지 않는다 (읽는 중인 위치를 지키려고)
    let changed = ds(|d| {
        if d.feed.as_ref() == Some(&v) {
            return false;
        }
        d.feed = Some(v);
        true
    });
    if changed || scroll {
        draw_feed(scroll);
    }
}

fn select_room(room: Room) {
    if ds(|d| std::mem::replace(&mut d.room, room)) == room {
        return;
    }
    let files = room == Room::Files;
    for (id, selected) in [("driveChatMode", !files), ("driveFilesMode", files)] {
        let _ = by_id(id).unwrap().set_attribute("aria-pressed", if selected { "true" } else { "false" });
    }
    let feed = by_id("driveFeed").unwrap();
    let _ = feed.set_attribute("aria-labelledby", if files { "driveFilesMode" } else { "driveChatMode" });
    toggle(&feed, "over", false);
    toggle(&by_id("driveChatComposer").unwrap(), "hidden", files);
    toggle(&by_id("driveFilesComposer").unwrap(), "hidden", !files);
    close_picker();
    draw_feed(true);
}

pub fn bind() {
    on(&by_id("driveChatMode").unwrap(), "click", |_| select_room(Room::Chat));
    on(&by_id("driveFilesMode").unwrap(), "click", |_| select_room(Room::Files));

    let feed = by_id("driveFeed").unwrap();
    let target = feed.clone();
    on(&feed, "dragover", move |e| {
        e.prevent_default();
        let files = ds(|d| d.room == Room::Files);
        toggle(&target, "over", files);
        let event: &web_sys::DragEvent = e.unchecked_ref();
        if let Some(data) = event.data_transfer() {
            data.set_drop_effect(if files { "copy" } else { "none" });
        }
    });
    let target = feed.clone();
    on(&feed, "dragleave", move |_| toggle(&target, "over", false));
    let target = feed.clone();
    on(&feed, "drop", move |e| {
        e.prevent_default();
        toggle(&target, "over", false);
        if ds(|d| d.room == Room::Files) {
            let event: &web_sys::DragEvent = e.unchecked_ref();
            if let Some(files) = event.data_transfer().and_then(|data| data.files()) {
                upload(files);
            }
        }
    });

    let input = by_id("driveFileInput").unwrap();
    let chooser = input.clone();
    on(&by_id("driveChooseLocal").unwrap(), "click", move |_| {
        if drive_ready() {
            chooser.click();
        }
    });
    let chooser = input.clone();
    on(&input, "change", move |_| {
        let inp: &web_sys::HtmlInputElement = chooser.unchecked_ref();
        if let Some(files) = inp.files() {
            upload(files);
        }
        inp.set_value("");
    });
    on(&by_id("driveChooseGoogle").unwrap(), "click", |_| {
        if drive_ready() {
            open_picker();
        }
    });
    on(&by_id("driveSend").unwrap(), "click", |_| send_text());
    on(&by_id("driveText").unwrap(), "keydown", |e| {
        let k: &web_sys::KeyboardEvent = e.unchecked_ref();
        if k.key() == "Enter" && !k.shift_key() && !k.is_composing() {
            e.prevent_default();
            send_text();
        }
    });
}

fn drive_ready() -> bool {
    let f = ds(|d| d.feed.clone()).unwrap_or(Value::Null);
    if !b(&f, "connected") {
        status(Room::Files, "먼저 팀에 연결해 주세요.", true);
        return false;
    }
    if !b(&f["google"], "signedIn") {
        status(Room::Files, "Google 로그인이 필요해요. 설정 → 외부 연결에서 로그인해 주세요.", true);
        return false;
    }
    if !b(&f["google"], "drive") {
        status(Room::Files, "Drive 권한이 새로 필요해요. 설정 → 외부 연결에서 로그아웃 후 다시 로그인해 주세요.", true);
        return false;
    }
    true
}

fn send_text() {
    let text = draft(Room::Chat);
    if !text.trim().is_empty() {
        send(Room::Chat, "drive:send-text", json!({ "text": text.trim() }), Some(text), "보내는 중…", "");
    }
}

fn upload(files: web_sys::FileList) {
    if ds(|d| d.room != Room::Files) || !drive_ready() {
        return;
    }
    let list: Vec<web_sys::File> = (0..files.length()).filter_map(|i| files.get(i)).collect();
    if list.is_empty() || !begin_request(Room::Files) {
        return;
    }
    let edit = edit_on();
    let caption = draft(Room::Files);
    spawn(async move {
        let mut first = true;
        for f in list {
            let name = f.name();
            if f.size() > MAX_MB * 1024.0 * 1024.0 {
                status(Room::Files, &format!("{name}: {MAX_MB}MB 보다 커요. Drive에 먼저 올린 뒤 ‘Google Drive에서 선택’을 눌러 주세요."), true);
                continue;
            }
            status(Room::Files, &format!("{name} 을(를) 내 Drive 에 올리는 중…"), false);
            let Ok(buf) = wasm_bindgen_futures::JsFuture::from(f.array_buffer()).await else {
                status(Room::Files, &format!("{name} 을(를) 읽지 못했어요."), true);
                continue;
            };
            use base64::Engine;
            let data = base64::engine::general_purpose::STANDARD.encode(js_sys::Uint8Array::new(&buf).to_vec());
            let t = if first { caption.trim() } else { "" };
            match call("drive:send-local", json!({ "name": name, "mime": f.type_(), "data": data, "edit": edit, "text": t })).await {
                Ok(v) => {
                    if first {
                        clear_sent_draft(Room::Files, &caption);
                        first = false;
                    }
                    received(v, Room::Files);
                    status(Room::Files, "", false);
                }
                Err(e) => {
                    status(Room::Files, &e, true);
                    break;
                }
            }
        }
        BUSY.with(|b| b.set(false));
    });
}

// ---------- 대화방 ----------
fn file_card(x: &Value) -> El {
    let name = s(x, "name");
    let url = s(x, "url");
    let (pill_cls, pill) = match s(x, "access").as_str() {
        "edit" => ("status-pill sent", "함께 고치기"),
        "view" => ("status-pill", "보기만"),
        _ => ("status-pill", "원래 권한"),
    };
    let size = size_text(n(x, "size"));
    let u = url.clone();
    el("div")
        .cls("drive-file")
        .attr("title", &url)
        .kid(el("span").cls("drive-kind").text(&kind_label(&s(x, "mimeType"), &name)))
        .kid(el("div").cls("drive-file-main").kid(el("div").cls("drive-file-name").text(&name)).kid(el("div").cls("drive-file-sub").kid(el("span").cls(pill_cls).text(pill)).txt(&if size.is_empty() { String::new() } else { format!(" {size}") })))
        .on("click", move |_| open_url(u.clone()))
}

fn draw_feed(scroll: bool) {
    let Some(feed) = by_id("driveFeed") else { return };
    let (v, room) = ds(|d| (d.feed.clone().unwrap_or(Value::Null), d.room));
    let files = room == Room::Files;
    let head = by_id("driveHead").unwrap();
    clear(&head);
    if !b(&v, "connected") {
        clear(&feed);
        let _ = feed.append_child(&el("div").cls("empty-state").kid(el("p").text(if files { "팀에 연결하면 파일을 공유할 수 있어요." } else { "팀에 연결하면 팀원과 톡을 주고받을 수 있어요." })).kid(el("button").cls("primary").text("설정 열기").click(|_| show_tab("settings"))).0);
        return;
    }
    let _ = head.append_child(&el("div").kid(el("b").text(&s(&v, "teamName"))).kid(el("span").cls("hint").text(&format!(" · 팀원 {}명", s(&v, "memberCount")))).0);
    if files && !b(&v["google"], "signedIn") {
        let _ = head.append_child(&el("div").cls("warn").txt("파일을 보내려면 Google 로그인이 필요해요. ").kid(el("button").cls("link").text("설정 → 외부 연결").click(|_| show_tab("settings"))).0);
    } else if files && !b(&v["google"], "drive") {
        let _ = head.append_child(&el("div").cls("warn").txt("내 드라이브에서 고르려면 Google 로그인을 한 번 더 해 주세요 (Drive 권한이 새로 필요해요). ").kid(el("button").cls("link").text("설정 → 외부 연결").click(|_| show_tab("settings"))).0);
    }
    if !s(&v, "error").is_empty() {
        let _ = head.append_child(&el("div").cls("err").text(&s(&v, "error")).0);
    }

    let near_bottom = feed.scroll_height() - feed.scroll_top() - feed.client_height() < 120;
    clear(&feed);
    let items = arr(&v, "items");
    let mut visible = items.iter().filter(|x| x["url"].as_str().is_some_and(|url| !url.is_empty()) == files).peekable();
    if visible.peek().is_none() {
        let _ = feed.append_child(&el("div").cls("empty").text(if files { "아직 공유한 파일이 없어요. 아래에서 파일을 선택하거나 여기에 끌어다 놓으세요." } else { "아직 주고받은 톡이 없어요. 팀원에게 첫 메시지를 보내 보세요." }).0);
    }
    let mut last_day = String::new();
    for x in visible {
        let at = n(x, "at");
        let day = day_key(at);
        if day != last_day {
            let _ = feed.append_child(&el("div").cls("drive-day").text(&day).0);
            last_day = day;
        }
        let mine = b(x, "mine");
        let mut bubble = el("div").cls(if mine { "drive-msg mine" } else { "drive-msg" });
        if !mine {
            bubble = bubble.kid(el("div").cls("drive-who").text(&s(x, "byName")));
        }
        if files {
            bubble = bubble.kid(file_card(x));
        }
        if !s(x, "text").is_empty() {
            bubble = bubble.kid(el("div").cls("drive-text").text(&s(x, "text")));
        }
        let mut foot = el("div").cls("drive-foot");
        if files {
            let u = s(x, "url");
            foot = foot.kid(el("button").cls("link").text("열기").click(move |_| open_url(u.clone())));
            if !mine && !s(x, "driveId").is_empty() && s(x, "mimeType") != "application/vnd.google-apps.folder" {
                let id = s(x, "id");
                foot = foot.kid(el("button").cls("link").text("내 Drive 에 저장").attr("title", "내 드라이브의 \"사이에서 받은 파일\" 폴더에 사본을 만들어요").click(move |_| {
                    let id = id.clone();
                    status(Room::Files, "내 Drive 에 사본을 만드는 중…", false);
                    spawn(async move {
                        match call("drive:save", json!({ "id": id })).await {
                            Ok(r) => status(Room::Files, &format!("내 드라이브의 \"사이에서 받은 파일\" 폴더에 \"{}\"을(를) 저장했어요.", s(&r, "name")), false),
                            Err(e) => status(Room::Files, &e, true),
                        }
                    });
                }));
            }
        }
        if mine {
            let id = s(x, "id");
            foot = foot.kid(el("button").cls("link").text("지우기").attr("title", if files { "공유 목록에서 지우기" } else { "이 메시지 지우기" }).click(move |_| {
                if confirm(if files { "파일을 공유 목록에서 지울까요? Google Drive 원본은 유지돼요." } else { "팀 톡에서 이 메시지를 지울까요?" }) {
                    send(room, "drive:delete", json!({ "id": id }), None, "지우는 중…", "지웠어요.");
                }
            }));
        }
        foot = foot.kid(el("span").cls("drive-time").text(&fmt_clock(at)));
        bubble = bubble.kid(foot);
        let _ = feed.append_child(&bubble.0);
    }
    if scroll || near_bottom {
        feed.set_scroll_top(feed.scroll_height());
    }
}

// ---------- 내 드라이브 · 공유 문서함에서 고르기 ----------
fn open_picker() {
    ds(|d| {
        if d.mode.is_empty() {
            d.mode = "my".into();
        }
    });
    toggle(&by_id("drivePicker").unwrap(), "hidden", false);
    spawn(load_picker(String::new()));
}

fn close_picker() {
    toggle(&by_id("drivePicker").unwrap(), "hidden", true);
}

fn set_mode(m: &str) {
    ds(|d| {
        d.mode = m.into();
        if m != "my" {
            d.path.clear();
        }
    });
    spawn(load_picker(String::new()));
}

async fn load_picker(page: String) {
    let Some(p) = by_id("drivePicker") else { return };
    let (mode, path, q) = ds(|d| (d.mode.clone(), d.path.clone(), d.q.clone()));
    if page.is_empty() {
        draw_picker_frame(&p, &mode, &path, &q);
    }
    let list = by_id("pickList").unwrap();
    let more = by_id("pickMore").unwrap();
    clear(&more);
    if page.is_empty() {
        clear(&list);
        let _ = list.append_child(&el("div").cls("hint").text("불러오는 중…").0);
    }
    let folder = path.last().map(|x| x.0.clone()).unwrap_or_default();
    let mode_q = if mode == "search" && q.trim().is_empty() { "my".to_string() } else { mode.clone() };
    match call("drive:list", json!({ "mode": mode_q, "folder": folder, "q": q, "page": page })).await {
        Ok(r) => {
            if page.is_empty() {
                clear(&list);
            }
            let files = arr(&r, "files");
            if files.is_empty() && page.is_empty() {
                let _ = list.append_child(&el("div").cls("empty").text(match mode.as_str() {
                    "shared" => "공유 문서함이 비어 있어요.",
                    "search" => "찾는 이름의 파일이 없어요.",
                    _ => "이 폴더는 비어 있어요.",
                }).0);
            }
            for f in &files {
                let _ = list.append_child(&pick_row(f).0);
            }
            let next = s(&r, "nextPageToken");
            if !next.is_empty() {
                let _ = more.append_child(&el("button").cls("link").text("더 보기").click(move |_| spawn(load_picker(next.clone()))).0);
            }
        }
        Err(e) => {
            clear(&list);
            let _ = list.append_child(&el("div").cls("err").text(&e).0);
        }
    }
}

fn draw_picker_frame(p: &HtmlElement, mode: &str, path: &[(String, String)], q: &str) {
    clear(p);
    let tab = |m: &'static str, label: &str| el("button").cls(if mode == m { "chip on" } else { "chip" }).text(label).click(move |_| set_mode(m));
    let search = el("input").attr("type", "search").attr("placeholder", "이름으로 찾기").attr("value", q).build();
    {
        let s2 = search.clone();
        on(&search, "keydown", move |e| {
            let k: &web_sys::KeyboardEvent = e.unchecked_ref();
            if k.key() == "Enter" && !k.is_composing() {
                let v = val(&s2);
                ds(|d| d.q = v);
                set_mode("search");
            }
        });
    }
    let _ = p.append_child(
        &el("div")
            .cls("pick-head")
            .kid(el("b").text("Google 드라이브에서 보내기"))
            .kid(el("button").cls("link").text("닫기").click(|_| close_picker()))
            .0,
    );
    let _ = p.append_child(&el("div").cls("chips").kid(tab("my", "내 드라이브")).kid(tab("shared", "공유 문서함")).kid(tab("recent", "최근")).child(search).0);
    if mode == "my" {
        let mut crumbs = el("div").cls("pick-path").kid(el("button").cls("link").text("내 드라이브").click(|_| {
            ds(|d| d.path.clear());
            spawn(load_picker(String::new()));
        }));
        for (i, (_, name)) in path.iter().enumerate() {
            crumbs = crumbs.txt(" › ").kid(el("button").cls("link").text(name).click(move |_| {
                ds(|d| d.path.truncate(i + 1));
                spawn(load_picker(String::new()));
            }));
        }
        let _ = p.append_child(&crumbs.0);
    }
    let _ = p.append_child(&el("p").cls("hint").text("파일을 고르면 팀의 파일 공유에 올려요. 내가 공유 권한을 가진 파일은 아래의 \"받은 팀원도 고칠 수 있게\" 설정대로 링크 권한을 맞추고, 그렇지 않은 파일은 원래 권한 그대로 보내요.").0);
    let _ = p.append_child(&el("div").id("pickList").cls("pick-list").0);
    let _ = p.append_child(&el("div").id("pickMore").0);
}

fn pick_row(f: &Value) -> El {
    let name = s(f, "name");
    let mime = s(f, "mimeType");
    let folder = mime == "application/vnd.google-apps.folder";
    let who = if b(f, "ownedByMe") {
        "나".to_string()
    } else {
        let sharer = s(&f["sharingUser"], "displayName");
        if !sharer.is_empty() {
            format!("{sharer} 공유")
        } else {
            f["owners"].get(0).map(|o| s(o, "displayName")).unwrap_or_default()
        }
    };
    let modified = s(f, "modifiedTime");
    let when = js_sys::Date::new(&modified.as_str().into()).get_time();
    let sub = format!("{who}{}{}", if when.is_nan() { String::new() } else { format!(" · {}", super::fmt_date(when)) }, { let z = size_text(n(f, "size")); if z.is_empty() { z } else { format!(" · {z}") } });
    let row = el("div").cls("item").kid(el("span").cls("drive-kind").text(&kind_label(&mime, &name))).kid(el("div").cls("main").kid(el("div").cls("title").text(&name)).kid(el("div").cls("sub").text(&sub)));
    let (id, n2) = (s(f, "id"), name.clone());
    if folder {
        row.kid(el("button").text("열기").click(move |_| {
            let (id, n2) = (id.clone(), n2.clone());
            ds(|d| {
                if d.mode != "my" {
                    d.mode = "my".into();
                    d.path.clear();
                }
                d.path.push((id, n2));
            });
            spawn(load_picker(String::new()));
        }))
    } else {
        row.kid(el("button").cls("primary").text("보내기").click(move |_| {
            let caption = draft(Room::Files);
            close_picker();
            send(Room::Files, "drive:send-drive", json!({ "fileId": id, "edit": edit_on(), "text": caption.trim() }), Some(caption), "보내는 중…", "보냈어요.");
        }))
    }
}
