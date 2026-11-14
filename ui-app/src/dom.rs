//! 화면 요소를 만드는 작은 도우미. 예전 화면 코드의 el(tag, attrs, ...children) 와 같은 방식이다.
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Document, Element, Event, HtmlElement, HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement, Node, Window};

pub fn window() -> Window {
    web_sys::window().unwrap()
}

pub fn document() -> Document {
    window().document().unwrap()
}

/// 요소 만들기
pub struct El(pub HtmlElement);

pub fn el(tag: &str) -> El {
    El(document().create_element(tag).unwrap().unchecked_into())
}

impl El {
    pub fn cls(self, c: &str) -> Self {
        if !c.is_empty() {
            self.0.set_class_name(c);
        }
        self
    }
    pub fn text(self, t: &str) -> Self {
        self.0.set_text_content(Some(t));
        self
    }
    pub fn attr(self, k: &str, v: &str) -> Self {
        let _ = self.0.set_attribute(k, v);
        self
    }
    /// 참일 때만 속성을 단다 (checked, disabled, selected, open …)
    pub fn flag(self, k: &str, on: bool) -> Self {
        if on {
            let _ = self.0.set_attribute(k, "");
        }
        self
    }
    pub fn id(self, v: &str) -> Self {
        self.attr("id", v)
    }
    pub fn on(self, ev: &str, f: impl FnMut(Event) + 'static) -> Self {
        on(&self.0, ev, f);
        self
    }
    pub fn click(self, f: impl FnMut(Event) + 'static) -> Self {
        self.on("click", f)
    }
    pub fn child(self, c: impl Into<Node>) -> Self {
        let _ = self.0.append_child(&c.into());
        self
    }
    pub fn kid(self, c: El) -> Self {
        let _ = self.0.append_child(&c.0);
        self
    }
    pub fn maybe(self, c: Option<El>) -> Self {
        match c {
            Some(c) => self.kid(c),
            None => self,
        }
    }
    pub fn kids(self, list: impl IntoIterator<Item = El>) -> Self {
        for c in list {
            let _ = self.0.append_child(&c.0);
        }
        self
    }
    /// 글자 조각 붙이기
    pub fn txt(self, s: &str) -> Self {
        let _ = self.0.append_child(&document().create_text_node(s));
        self
    }
    pub fn node(&self) -> &HtmlElement {
        &self.0
    }
    pub fn build(self) -> HtmlElement {
        self.0
    }
}

impl From<El> for Node {
    fn from(e: El) -> Node {
        e.0.into()
    }
}

pub fn on(target: &web_sys::EventTarget, ev: &str, f: impl FnMut(Event) + 'static) {
    let c = Closure::<dyn FnMut(Event)>::new(f);
    let _ = target.add_event_listener_with_callback(ev, c.as_ref().unchecked_ref());
    // 창이 닫힐 때 함께 정리된다
    c.forget();
}

pub fn by_id(id: &str) -> Option<HtmlElement> {
    document().get_element_by_id(id).map(|e| e.unchecked_into())
}

pub fn q(sel: &str) -> Option<HtmlElement> {
    document().query_selector(sel).ok().flatten().map(|e| e.unchecked_into())
}

pub fn q_in(root: &Element, sel: &str) -> Option<HtmlElement> {
    root.query_selector(sel).ok().flatten().map(|e| e.unchecked_into())
}

pub fn q_all_in(root: &Element, sel: &str) -> Vec<HtmlElement> {
    let mut out = vec![];
    if let Ok(list) = root.query_selector_all(sel) {
        for i in 0..list.length() {
            if let Some(n) = list.item(i) {
                out.push(n.unchecked_into());
            }
        }
    }
    out
}

pub fn q_all(sel: &str) -> Vec<HtmlElement> {
    q_all_in(&document().document_element().unwrap(), sel)
}

pub fn clear(e: &Element) {
    e.set_text_content(None);
}

pub fn set_text(id: &str, t: &str) {
    if let Some(e) = by_id(id) {
        e.set_text_content(Some(t));
    }
}

pub fn toggle(e: &Element, class: &str, on: bool) {
    let _ = e.class_list().toggle_with_force(class, on);
}

pub fn has_class(e: &Element, class: &str) -> bool {
    e.class_list().contains(class)
}

pub fn confirm(msg: &str) -> bool {
    window().confirm_with_message(msg).unwrap_or(false)
}

/// 입력 칸의 값 (input / select / textarea 어느 것이든)
pub fn val(e: &Element) -> String {
    if let Some(i) = e.dyn_ref::<HtmlInputElement>() {
        return i.value();
    }
    if let Some(s) = e.dyn_ref::<HtmlSelectElement>() {
        return s.value();
    }
    if let Some(t) = e.dyn_ref::<HtmlTextAreaElement>() {
        return t.value();
    }
    e.text_content().unwrap_or_default()
}

pub fn set_val(e: &Element, v: &str) {
    if let Some(i) = e.dyn_ref::<HtmlInputElement>() {
        i.set_value(v);
    } else if let Some(s) = e.dyn_ref::<HtmlSelectElement>() {
        s.set_value(v);
    } else if let Some(t) = e.dyn_ref::<HtmlTextAreaElement>() {
        t.set_value(v);
    }
}

pub fn checked(e: &Element) -> bool {
    e.dyn_ref::<HtmlInputElement>().map(|i| i.checked()).unwrap_or(false)
}

pub fn set_checked(e: &Element, on: bool) {
    if let Some(i) = e.dyn_ref::<HtmlInputElement>() {
        i.set_checked(on);
    }
}

pub fn set_disabled(e: &Element, on: bool) {
    if on {
        let _ = e.set_attribute("disabled", "");
    } else {
        let _ = e.remove_attribute("disabled");
    }
}

/// 이름(name=)으로 폼 안의 칸 찾기
pub fn field(form: &Element, name: &str) -> Element {
    form.query_selector(&format!("[name=\"{name}\"]")).ok().flatten().unwrap_or_else(|| document().create_element("input").unwrap())
}

pub fn scroll_into_view(e: &Element, smooth: bool) {
    let o = web_sys::ScrollIntoViewOptions::new();
    if smooth {
        o.set_behavior(web_sys::ScrollBehavior::Smooth);
    }
    o.set_block(web_sys::ScrollLogicalPosition::Nearest);
    e.scroll_into_view_with_scroll_into_view_options(&o);
}

pub fn spawn(f: impl std::future::Future<Output = ()> + 'static) {
    wasm_bindgen_futures::spawn_local(f);
}

/// ms 뒤에 한 번
pub fn after(ms: i32, f: impl FnOnce() + 'static) {
    let c = Closure::once_into_js(f);
    let _ = window().set_timeout_with_callback_and_timeout_and_arguments_0(c.unchecked_ref(), ms);
}

/// ms 마다. 돌려주는 값으로 멈출 수 있다
pub fn every(ms: i32, f: impl FnMut() + 'static) -> i32 {
    let c = Closure::<dyn FnMut()>::new(f);
    let id = window().set_interval_with_callback_and_timeout_and_arguments_0(c.as_ref().unchecked_ref(), ms).unwrap_or(0);
    c.forget();
    id
}

pub fn stop(id: i32) {
    window().clear_interval_with_handle(id);
}

pub async fn sleep(ms: i32) {
    let p = js_sys::Promise::new(&mut |res, _| {
        let _ = window().set_timeout_with_callback_and_timeout_and_arguments_0(&res, ms);
    });
    let _ = wasm_bindgen_futures::JsFuture::from(p).await;
}

pub fn now() -> f64 {
    js_sys::Date::now()
}

/// 주소의 ?tab=… 값
pub fn query_param(k: &str) -> Option<String> {
    let s = window().location().search().ok()?;
    web_sys::UrlSearchParams::new_with_str(&s).ok()?.get(k)
}

pub fn event_target(e: &Event) -> Option<Element> {
    e.target().and_then(|t| t.dyn_into::<Element>().ok())
}

pub fn prevent(e: &Event) {
    e.prevent_default();
}
