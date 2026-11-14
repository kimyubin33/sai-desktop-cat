//! 사이의 화면 (고양이 창·대화창). Rust 로 쓰고 WebAssembly 로 만들어 창 안에서 돌린다.
//! 손으로 쓴 JavaScript 는 없다 — 빌드 도구가 만드는 연결 파일(ui/pkg)만 있다.
pub mod sprite;

#[cfg(target_arch = "wasm32")]
pub mod api;
#[cfg(target_arch = "wasm32")]
pub mod dom;
#[cfg(target_arch = "wasm32")]
mod cat;
#[cfg(target_arch = "wasm32")]
mod chat;
#[cfg(target_arch = "wasm32")]
mod peek;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn main() {
    let which = dom::document().document_element().and_then(|e| e.get_attribute("data-window")).unwrap_or_default();
    match which.as_str() {
        "cat" => cat::start(),
        "chat" => chat::start(),
        "peek" => peek::start(),
        _ => {}
    }
}
