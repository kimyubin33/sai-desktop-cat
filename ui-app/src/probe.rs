//! 휴대용 진단 창은 일반 대화창 초기화와 외부 계정 조회를 실행하지 않는다.
use crate::chat::settings::mesh;

pub fn start() {
    mesh::bind(true);
    crate::dom::spawn(mesh::load());
}
