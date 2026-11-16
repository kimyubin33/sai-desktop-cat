use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use std::net::SocketAddrV4;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

use super::{card, Discovered, Inner};
use crate::Result;

const SERVICE: &str = "_sai-probe._tcp.local.";

pub(super) struct Discovery {
    daemon: ServiceDaemon,
    fullname: String,
    task: JoinHandle<()>,
}

impl Discovery {
    pub fn start(inner: Arc<Inner>, address: SocketAddrV4) -> Result<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|_| "LAN 자동 찾기를 시작하지 못했어요. 연결 정보로 직접 확인할 수 있어요.")?;
        let setup = || -> std::result::Result<_, mdns_sd::Error> {
            daemon.disable_interface(IfKind::All)?;
            daemon.enable_interface(IfKind::Addr((*address.ip()).into()))?;
            let fingerprint = card::fingerprint(&inner.identity.certificate);
            let properties = [
                ("id", inner.card.session.as_str()),
                ("fp", fingerprint.as_str()),
                ("v", "1"),
            ];
            let info = ServiceInfo::new(
                SERVICE,
                &inner.card.session,
                &format!("{}.local.", inner.card.session),
                std::net::IpAddr::V4(*address.ip()),
                address.port(),
                &properties[..],
            )?;
            let fullname = info.get_fullname().to_owned();
            daemon.register(info)?;
            Ok((fullname, daemon.browse(SERVICE)?))
        };
        let (fullname, events) = match setup() {
            Ok(result) => result,
            Err(_) => {
                let _ = daemon.shutdown();
                return Err("선택한 LAN에서 자동 찾기를 시작하지 못했어요. 연결 정보로 직접 확인할 수 있어요.".into());
            }
        };
        let worker = daemon.clone();
        let own = fullname.clone();
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(250));
            while inner.running() {
                interval.tick().await;
                // 이 광고는 신뢰·페어링이 아니다. 등록한 인증서만 TLS 연결에 사용한다.
                for _ in 0..64 {
                    let Ok(event) = events.try_recv() else { break };
                    match event {
                        ServiceEvent::ServiceResolved(info) => {
                            let id = info.get_property_val_str("id").unwrap_or_default();
                            let fp = info.get_property_val_str("fp").unwrap_or_default();
                            if id == inner.card.session
                                || !card::valid_session(id)
                                || fp.len() != 64
                                || !fp
                                    .bytes()
                                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                                || info.get_property_val_str("v") != Some("1")
                            {
                                continue;
                            }
                            let addresses: Vec<_> = info
                                .get_addresses_v4()
                                .into_iter()
                                .filter(|ip| ip.is_private())
                                .take(4)
                                .map(|ip| SocketAddrV4::new(ip, info.get_port()))
                                .collect();
                            if addresses.is_empty() || info.get_port() == 0 {
                                continue;
                            }
                            let mut peers = inner.discovered.lock().unwrap();
                            if peers.len() < 32 || peers.contains_key(id) {
                                peers.insert(
                                    id.into(),
                                    Discovered {
                                        session: id.into(),
                                        fingerprint: fp.into(),
                                        addresses,
                                    },
                                );
                            }
                        }
                        ServiceEvent::ServiceRemoved(_, name) => {
                            if let Some(id) = name.strip_suffix(&format!(".{SERVICE}")) {
                                inner.discovered.lock().unwrap().remove(id);
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
