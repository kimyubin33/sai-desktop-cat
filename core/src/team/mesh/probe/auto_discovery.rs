//! 자동 연결 광고는 실행 ID와 장치 쌍별 불투명 표식만 포함한다.
use super::{auto::Advertisement, Inner};
use crate::{err, Result};
use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use std::{net::SocketAddrV4, sync::Arc, time::Duration};
use tokio::task::JoinHandle;

const SERVICE: &str = "_sai-sync._tcp.local.";
pub(super) struct Discovery {
    daemon: ServiceDaemon,
    fullname: String,
    task: JoinHandle<()>,
}
impl Discovery {
    pub fn start(inner: Arc<Inner>, address: SocketAddrV4) -> Result<Self> {
        let daemon = ServiceDaemon::new().map_err(|_| "LAN 자동 연결 찾기를 시작하지 못했어요.")?;
        let state = inner.automatic.as_ref().unwrap();
        let advertisement = match state.advertisement() {
            Ok(advertisement) => advertisement,
            Err(error) => {
                let _ = daemon.shutdown();
                return Err(error);
            }
        };
        let mut properties = vec![
            ("id".to_string(), advertisement.session.clone()),
            ("v".into(), "1".into()),
        ];
        for (i, tag) in advertisement.tags.iter().enumerate() {
            properties.push((format!("t{i}"), tag.clone()));
        }
        let setup = || -> std::result::Result<_, mdns_sd::Error> {
            daemon.disable_interface(IfKind::All)?;
            daemon.enable_interface(IfKind::Addr((*address.ip()).into()))?;
            let properties: Vec<_> = properties
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let info = ServiceInfo::new(
                SERVICE,
                &advertisement.session,
                &format!("{}.local.", advertisement.session),
                std::net::IpAddr::V4(*address.ip()),
                address.port(),
                &properties[..],
            )?;
            let fullname = info.get_fullname().to_string();
            daemon.register(info)?;
            Ok((fullname, daemon.browse(SERVICE)?))
        };
        let (fullname, events) = match setup() {
            Ok(value) => value,
            Err(_) => {
                let _ = daemon.shutdown();
                return err("선택한 LAN에서 자동 연결 광고를 시작하지 못했어요. 수동 연결을 사용할 수 있어요.");
            }
        };
        let worker = daemon.clone();
        let own = fullname.clone();
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(250));
            while inner.running() {
                interval.tick().await;
                for _ in 0..64 {
                    let Ok(event) = events.try_recv() else { break };
                    let state = inner.automatic.as_ref().unwrap();
                    match event {
                        ServiceEvent::ServiceResolved(info) => {
                            if info.get_property_val_str("v") != Some("1") {
                                continue;
                            }
                            let session = info
                                .get_property_val_str("id")
                                .unwrap_or_default()
                                .to_string();
                            let tags = (0..8)
                                .filter_map(|i| {
                                    info.get_property_val_str(&format!("t{i}"))
                                        .map(str::to_string)
                                })
                                .collect();
                            let addresses = info
                                .get_addresses_v4()
                                .into_iter()
                                .map(|ip| SocketAddrV4::new(ip, info.get_port()))
                                .filter(|a| state.permits(*a))
                                .take(4)
                                .collect();
                            state.ingest(Advertisement {
                                version: 1,
                                session,
                                tags,
                                addresses,
                            });
                        }
                        ServiceEvent::ServiceRemoved(_, name) => {
                            if let Some(session) = name.strip_suffix(&format!(".{SERVICE}")) {
                                state.forget(session);
                            }
                        }
                        _ => {}
                    }
                }
            }
            let _ = worker.unregister(&own);
            let _ = worker.shutdown();
        });
        Ok(Self {
            daemon,
            fullname,
            task,
        })
    }
}
impl Drop for Discovery {
    fn drop(&mut self) {
        self.task.abort();
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}
