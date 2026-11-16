//! OS 전원 알림에서는 메모리의 취소 신호만 변경한다. DB·소켓 생성은 감독 작업이 맡는다.
use sai_core::team::mesh::runtime::{Controller, Event};

#[cfg(windows)]
mod windows {
    use super::*;
    use std::sync::Mutex;
    use windows_sys::Win32::{System::Power::*, UI::WindowsAndMessaging::*};
    static TARGET: Mutex<Option<Controller>> = Mutex::new(None);
    pub struct PowerGuard(usize);
    unsafe extern "system" fn notified(
        _: *const core::ffi::c_void,
        kind: u32,
        _: *const core::ffi::c_void,
    ) -> u32 {
        // 정적 슬롯은 해제되지 않는다. 콜백이 실행 중이어도 복제한 Controller는 유효하다.
        let target = TARGET.lock().ok().and_then(|s| s.clone());
        if let Some(target) = target {
            match kind {
                PBT_APMSUSPEND => target.event(Event::Suspend),
                PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => target.event(Event::Resume),
                _ => {}
            }
        }
        0
    }
    impl PowerGuard {
        pub fn register(controller: Controller) -> Result<Self, String> {
            {
                let mut target = TARGET
                    .lock()
                    .map_err(|_| "전원 알림 상태를 열지 못했어요.")?;
                if target.is_some() {
                    return Err("전원 알림이 이미 등록돼 있어요.".into());
                }
                *target = Some(controller);
            }
            let mut parameters = DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
                Callback: Some(notified),
                Context: std::ptr::null_mut(),
            };
            let mut handle = std::ptr::null_mut();
            let result = unsafe {
                PowerRegisterSuspendResumeNotification(
                    DEVICE_NOTIFY_CALLBACK,
                    &mut parameters as *mut _ as _,
                    &mut handle,
                )
            };
            if result != 0 {
                *TARGET.lock().unwrap() = None;
                return Err(
                    "Windows 절전 알림을 등록하지 못했어요. 실행 간격 감지로 복구를 확인해요."
                        .into(),
                );
            }
            Ok(Self(handle as usize))
        }
    }
    impl Drop for PowerGuard {
        fn drop(&mut self) {
            *TARGET.lock().unwrap() = None;
            unsafe {
                PowerUnregisterSuspendResumeNotification(self.0 as _);
            }
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[tokio::test]
        async fn power_registration_and_callback_do_not_create_a_listener_without_consent() {
            let path =
                std::env::temp_dir().join(format!("sai-power-{}.sqlite3", rand::random::<u64>()));
            let controller = Controller::new(sai_core::team::mesh::identity::Vault::new(&path));
            let guard = PowerGuard::register(controller.clone()).unwrap();
            unsafe {
                notified(std::ptr::null(), PBT_APMSUSPEND, std::ptr::null());
            }
            let config = sai_core::team::mesh::runtime::Config {
                team: "unregistered".into(),
                address: "127.0.0.1".parse().unwrap(),
                mode: sai_core::team::mesh::runtime::Mode::Courier,
                restore: true,
            };
            assert!(controller
                .start(config)
                .await
                .err()
                .unwrap()
                .contains("절전"));
            unsafe {
                notified(std::ptr::null(), PBT_APMRESUMEAUTOMATIC, std::ptr::null());
                notified(std::ptr::null(), PBT_APMRESUMESUSPEND, std::ptr::null());
            }
            controller.tick().await;
            assert!(!controller.status().await.session.running);
            assert!(!path.exists());
            drop(guard);
        }
    }
}
#[cfg(windows)]
pub use windows::PowerGuard;
#[cfg(not(windows))]
pub struct PowerGuard;
#[cfg(not(windows))]
impl PowerGuard {
    pub fn register(_: Controller) -> Result<Self, String> {
        Err("Windows 절전 알림을 사용할 수 없어요.".into())
    }
}
