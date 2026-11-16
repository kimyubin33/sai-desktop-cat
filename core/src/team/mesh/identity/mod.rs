//! 단계 1: 장치 신원·관리자 승인 이력과 Windows 사용자별 개인키 보관.
pub(crate) mod archive;
pub(crate) mod automatic;
pub(crate) mod consent;
pub(crate) mod courier;
mod crypt;
pub mod journal;
mod keys;
pub mod model;
pub mod session;
pub mod talk;
pub(crate) mod transfer;
mod vault;

pub use model::{ApprovalPacket, DevicePublic, EnrollmentRequest};
pub use vault::Vault;

#[cfg(test)]
mod tests;
