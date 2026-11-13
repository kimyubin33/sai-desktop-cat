//! 사이 — 바탕화면 고양이 협업 도우미의 핵심 로직.
//! 창·트레이 같은 화면 요소와 무관해서 단독으로 테스트할 수 있다.
//! 화면(Tauri)은 `src-tauri` 크레이트가 이 로직을 감싼다.

pub mod util;
pub mod time;
pub mod store;
pub mod secret;
pub mod http;

pub mod topics;
pub mod safety;
pub mod language;
pub mod counseling;
pub mod privacy;
pub mod advice;
pub mod behavior;

pub mod team;
pub mod collab;
pub mod notify;
pub mod google;
pub mod schedule;
pub mod ai;
pub mod chat;

pub use serde_json::{json, Value};

/// 모든 오류는 사용자에게 그대로 보여 줄 수 있는 한국어 문장으로 다룬다.
pub type Result<T> = std::result::Result<T, String>;

/// 오류 문장을 만드는 짧은 도우미
pub fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(msg.into())
}
