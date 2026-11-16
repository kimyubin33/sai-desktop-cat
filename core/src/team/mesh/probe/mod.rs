//! 임시 인증서의 상호 TLS 확인과 별도로 동의한 승인 장치의 준비 기록 교환.
mod approved;
mod auto;
mod auto_discovery;
mod card;
mod courier;
mod discovery;
mod exchange;
mod host;
mod tls;
mod wire;
pub use auto::AutomaticStatus;
pub use exchange::ExchangeReport;
pub use host::HostStatus;

pub use approved::ApprovedCard;
pub use card::PeerCard;

use base64::{engine::general_purpose::STANDARD, Engine};
use rand::RngCore;
use serde::Serialize;
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::{err, Result};
use card::{fingerprint, MAX_PEERS, NAME, VERSION};
use wire::Signal;

const LIFETIME: Duration = Duration::from_secs(15 * 60);
const IO_TIMEOUT: Duration = Duration::from_secs(3);

fn random_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 화면에서 선택한 로컬 LAN 주소만 수신 대상으로 사용할 수 있다.
pub fn interfaces() -> Result<Vec<Ipv4Addr>> {
    let mut addresses: Vec<_> = interface_options()?
        .into_iter()
        .map(|interface| interface.address)
        .collect();
    addresses.sort();
    addresses.dedup();
    Ok(addresses)
}

#[derive(Clone, Serialize)]
pub struct LanInterface {
    pub address: Ipv4Addr,
    pub name: String,
}

pub fn interface_options() -> Result<Vec<LanInterface>> {
    let mut addresses: Vec<_> = if_addrs::get_if_addrs()
        .map_err(|_| "이 PC의 네트워크 주소를 확인하지 못했어요.")?
        .into_iter()
        .filter_map(|interface| match interface.ip() {
            std::net::IpAddr::V4(ip) if ip.is_private() => Some(LanInterface {
                address: ip,
                name: interface.name,
            }),
            _ => None,
        })
        .collect();
    addresses.sort_by(|a, b| a.address.cmp(&b.address).then(a.name.cmp(&b.name)));
    addresses.dedup_by(|a, b| a.address == b.address);
    Ok(addresses)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerStatus {
    pub session: String,
    pub fingerprint: String,
    pub last_checked: i64,
    pub last_success: i64,
    pub last_error: Option<String>,
    pub exchange_enabled: bool,
    pub automatic_enabled: bool,
    pub courier_enabled: bool,
    pub host_enabled: bool,
    pub device_id: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct Discovered {
    pub session: String,
    pub fingerprint: String,
    pub addresses: Vec<SocketAddrV4>,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    pub session: Option<String>,
    pub card: Option<String>,
    pub fingerprint: Option<String>,
    pub expires_at: i64,
    pub peers: Vec<PeerStatus>,
    pub discovered: Vec<Discovered>,
    pub discovery_error: Option<String>,
    pub team_id: Option<String>,
    pub device_id: Option<String>,
    pub exchange_enabled: bool,
    pub automatic: AutomaticStatus,
    pub courier_enabled: bool,
    pub notice: Option<String>,
    pub host: HostStatus,
}

#[derive(Clone)]
struct Peer {
    card: PeerCard,
    status: PeerStatus,
    approval: Option<ApprovedCard>,
    bootstrapped: bool,
}

struct Inner {
    identity: tls::Identity,
    card: PeerCard,
    active: AtomicBool,
    deadline: Instant,
    expires_at: i64,
    peers: Mutex<BTreeMap<String, Peer>>,
    discovered: Mutex<BTreeMap<String, Discovered>>,
    checking: AtomicBool,
    stopped: tokio::sync::watch::Sender<bool>,
    approved: Option<approved::Context>,
    automatic: Option<Arc<auto::State>>,
    host: Option<host::State>,
    notice: Mutex<Option<String>>,
}

impl Inner {
    async fn connect_socket(&self, address: SocketAddrV4) -> Result<TcpStream> {
        if let Some(state) = &self.automatic {
            if !state.permits(address) {
                return err("선택한 LAN 범위 밖에는 자동 연결하지 않아요.");
            }
            let socket =
                tokio::net::TcpSocket::new_v4().map_err(|_| "자동 연결 소켓을 열지 못했어요.")?;
            socket
                .bind(std::net::SocketAddr::V4(SocketAddrV4::new(
                    *self.card.addresses[0].ip(),
                    0,
                )))
                .map_err(|_| "선택한 LAN 주소를 사용할 수 없어요.")?;
            socket
                .connect(std::net::SocketAddr::V4(address))
                .await
                .map_err(|_| "상대 LAN 장치에 접속하지 못했어요.".into())
        } else {
            TcpStream::connect(address)
                .await
                .map_err(|_| "상대 PC에 접속하지 못했어요.".into())
        }
    }
    fn running(&self) -> bool {
        self.active.load(Ordering::Acquire) && Instant::now() < self.deadline
    }

    fn check_active(&self) -> Result<()> {
        if self.running() {
            Ok(())
        } else {
            err("연결 실험이 끝났어요. 다시 시작하면 새 연결 정보를 교환해 주세요.")
        }
    }
}

pub struct Probe {
    inner: Arc<Inner>,
    listener: Option<JoinHandle<()>>,
    discovery: Option<discovery::Discovery>,
    discovery_error: Option<String>,
    automatic: Option<auto::Runtime>,
    host: Option<host::Runtime>,
}

/// 확인 요청은 런타임 소유권을 갖지 않는다. 중지 버튼이 요청 완료를 기다릴 필요가 없다.
#[derive(Clone)]
pub struct Checker(Arc<Inner>);

impl Probe {
    pub async fn start(address: Ipv4Addr) -> Result<Self> {
        if !interfaces()?.contains(&address) {
            return err("이 PC에서 사용할 LAN 주소를 골라 주세요.");
        }
        Self::start_inner(address, true, LIFETIME).await
    }

    async fn start_inner(address: Ipv4Addr, advertise: bool, lifetime: Duration) -> Result<Self> {
        Self::start_mode(address, advertise, lifetime, None).await
    }

    async fn start_mode(
        address: Ipv4Addr,
        advertise: bool,
        lifetime: Duration,
        authority: Option<(crate::team::mesh::identity::Vault, String)>,
    ) -> Result<Self> {
        Self::start_mode_exchange(address, advertise, lifetime, authority, false).await
    }

    async fn start_mode_exchange(
        address: Ipv4Addr,
        advertise: bool,
        lifetime: Duration,
        authority: Option<(crate::team::mesh::identity::Vault, String)>,
        exchange: bool,
    ) -> Result<Self> {
        Self::start_mode_auto(address, advertise, lifetime, authority, exchange, None).await
    }

    async fn start_mode_auto(
        address: Ipv4Addr,
        advertise: bool,
        lifetime: Duration,
        authority: Option<(crate::team::mesh::identity::Vault, String)>,
        exchange: bool,
        options: Option<auto::Options>,
    ) -> Result<Self> {
        if options.is_some() && (!exchange || authority.is_none()) {
            return err("자동 연결에는 승인 장치와 기록 교환 동의가 필요해요.");
        }
        let identity = tls::Identity::new()?;
        let listener = TcpListener::bind((address, 0))
            .await
            .map_err(|_| "선택한 주소에서 연결 실험을 열지 못했어요.")?;
        let local = match listener
            .local_addr()
            .map_err(|_| "수신 주소를 읽지 못했어요.")?
        {
            std::net::SocketAddr::V4(address) => address,
            _ => return err("IPv4 주소를 선택해 주세요."),
        };
        let card = PeerCard {
            protocol: VERSION,
            session: random_id(),
            certificate: STANDARD.encode(&identity.certificate),
            addresses: vec![local],
        };
        let approved = match authority {
            Some((vault, team)) => Some(
                approved::Context::new(
                    vault,
                    team,
                    card.clone(),
                    exchange,
                    options.is_some(),
                    options.as_ref().is_some_and(|o| o.courier),
                )
                .await?,
            ),
            None => None,
        };
        let automatic = match &options {
            Some(options) => Some(Arc::new(
                auto::State::prepare(approved.as_ref().unwrap(), local, options.clone()).await?,
            )),
            None => None,
        };
        let inner = Arc::new(Inner {
            identity,
            card,
            active: AtomicBool::new(true),
            deadline: Instant::now() + lifetime,
            expires_at: crate::time::now_ms() + lifetime.as_millis() as i64,
            peers: Mutex::new(BTreeMap::new()),
            discovered: Mutex::new(BTreeMap::new()),
            checking: AtomicBool::new(false),
            stopped: tokio::sync::watch::channel(false).0,
            host: approved
                .as_ref()
                .filter(|c| c.own.host)
                .map(|c| host::State::new(c, options.as_ref().unwrap().host.clone())),
            approved,
            automatic,
            notice: Mutex::new(None),
        });
        let host_options = options.as_ref().map(|o| o.host.clone());
        let automatic = match options {
            Some(options) => Some(auto::Runtime::start(inner.clone(), local, options)?),
            None => None,
        };
        let host = inner
            .host
            .as_ref()
            .map(|_| host::Runtime::start(inner.clone(), host_options.unwrap()));
        let worker = inner.clone();
        let listener = tokio::spawn(async move {
            serve(listener, worker).await;
        });
        let (discovery, discovery_error) = if advertise {
            match discovery::Discovery::start(inner.clone(), local) {
                Ok(d) => (Some(d), None),
                Err(e) => (None, Some(e)),
            }
        } else {
            (None, None)
        };
        Ok(Self {
            inner,
            listener: Some(listener),
            discovery,
            discovery_error,
            automatic,
            host,
        })
    }

    pub fn checker(&self) -> Checker {
        Checker(self.inner.clone())
    }

    pub fn status(&self) -> Status {
        if !self.inner.running() {
            return Status {
                notice: self.inner.notice.lock().unwrap().clone(),
                automatic: self
                    .inner
                    .automatic
                    .as_ref()
                    .map(|a| a.status(false))
                    .unwrap_or_default(),
                ..Status::default()
            };
        }
        Status {
            running: true,
            session: Some(self.inner.card.session.clone()),
            card: match &self.inner.approved {
                Some(context) => serde_json::to_string(&context.own).ok(),
                None => serde_json::to_string(&self.inner.card).ok(),
            },
            fingerprint: Some(fingerprint(&self.inner.identity.certificate)),
            expires_at: self.inner.expires_at,
            peers: self
                .inner
                .peers
                .lock()
                .unwrap()
                .values()
                .map(|p| p.status.clone())
                .collect(),
            discovered: self
                .inner
                .discovered
                .lock()
                .unwrap()
                .values()
                .cloned()
                .collect(),
            discovery_error: self.discovery_error.clone(),
            team_id: self.inner.approved.as_ref().map(|c| c.team.clone()),
            device_id: self
                .inner
                .approved
                .as_ref()
                .map(|c| c.own.proof.body.device_id.clone()),
            exchange_enabled: self.inner.approved.as_ref().is_some_and(|c| c.own.exchange),
            courier_enabled: self.inner.approved.as_ref().is_some_and(|c| c.own.courier),
            automatic: self
                .inner
                .automatic
                .as_ref()
                .map(|a| a.status(true))
                .unwrap_or_default(),
            notice: self.inner.notice.lock().unwrap().clone(),
            host: self
                .inner
                .host
                .as_ref()
                .map(|h| h.status())
                .unwrap_or_default(),
        }
    }

    pub fn add_peer(&self, input: &str) -> Result<String> {
        add_peer(&self.inner, input)
    }
}

fn add_peer(inner: &Arc<Inner>, input: &str) -> Result<String> {
    inner.check_active()?;
    let approval = inner
        .approved
        .as_ref()
        .map(|context| context.parse_peer(input))
        .transpose()?;
    let card = match &approval {
        Some(card) => card.card.clone(),
        None => PeerCard::parse(input)?,
    };
    if card.session == inner.card.session {
        return err("이 PC의 정보예요. 상대 PC의 연결 정보를 등록해 주세요.");
    }
    // 등록 시에도 인증서 문법을 검사한다. 인증서 체인과 소유 증명은 TLS가 확인한다.
    inner.identity.client(&card)?;
    let fingerprint = card.fingerprint()?;
    let mut peers = inner.peers.lock().unwrap();
    if let Some(old) = peers.get(&card.session) {
        if old.status.fingerprint != fingerprint {
            return err("같은 실험 ID의 인증서가 달라요. 상대 PC에서 다시 확인해 주세요.");
        }
    } else if peers.len() >= MAX_PEERS {
        return err("연결 실험에는 최대 8대까지 등록할 수 있어요.");
    }
    let session = card.session.clone();
    let mut status = peers
        .get(&session)
        .map(|p| p.status.clone())
        .unwrap_or(PeerStatus {
            session: session.clone(),
            fingerprint,
            last_checked: 0,
            last_success: 0,
            last_error: None,
            exchange_enabled: approval.as_ref().is_some_and(|a| a.exchange),
            automatic_enabled: approval.as_ref().is_some_and(|a| a.automatic),
            courier_enabled: approval.as_ref().is_some_and(|a| a.courier),
            host_enabled: approval.as_ref().is_some_and(|a| a.host),
            device_id: approval.as_ref().map(|a| a.proof.body.device_id.clone()),
        });
    status.exchange_enabled = approval.as_ref().is_some_and(|a| a.exchange);
    status.automatic_enabled = approval.as_ref().is_some_and(|a| a.automatic);
    status.courier_enabled = approval.as_ref().is_some_and(|a| a.courier);
    status.host_enabled = approval.as_ref().is_some_and(|a| a.host);
    peers.insert(
        session.clone(),
        Peer {
            card,
            status,
            approval,
            bootstrapped: false,
        },
    );
    Ok(session)
}

impl Probe {
    pub async fn shutdown(mut self) {
        self.inner.active.store(false, Ordering::Release);
        self.inner.stopped.send_replace(true);
        self.discovery.take();
        self.automatic.take();
        self.host.take();
        if let Some(task) = self.listener.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.inner.active.store(false, Ordering::Release);
        self.inner.stopped.send_replace(true);
        if let Some(task) = self.listener.take() {
            task.abort();
        }
        self.discovery.take();
        self.automatic.take();
        self.host.take();
    }
}

impl Checker {
    /// 소유자의 비동기 정리를 기다리지 않고 새 작업과 수신 연결을 취소한다.
    pub fn cancel(&self) {
        self.0.active.store(false, Ordering::Release);
        self.0.stopped.send_replace(true);
    }
    pub async fn add_peer(&self, input: String) -> Result<String> {
        let inner = self.0.clone();
        tokio::task::spawn_blocking(move || add_peer(&inner, &input))
            .await
            .map_err(|_| "연결 정보를 등록하지 못했어요.")?
    }
    pub async fn check(&self, session: &str) -> Result<()> {
        self.0.check_active()?;
        let peer = self
            .0
            .peers
            .lock()
            .unwrap()
            .get(session)
            .cloned()
            .ok_or("먼저 상대 PC의 연결 정보를 등록해 주세요.")?;
        if self.0.checking.swap(true, Ordering::AcqRel) {
            return err("앞의 연결 확인이 끝날 때까지 기다려 주세요.");
        }
        struct Checking<'a>(&'a AtomicBool);
        impl Drop for Checking<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _checking = Checking(&self.0.checking);
        let mut stopped = self.0.stopped.subscribe();
        if *stopped.borrow() {
            return err("연결 실험이 중지됐어요.");
        }
        let result = tokio::select! {
            biased;
            _ = stopped.changed() => return err("연결 실험이 중지됐어요."),
            result = tokio::time::timeout(Duration::from_secs(8), self.connect(&peer)) => result
                .unwrap_or_else(|_| Err("연결 확인 시간이 지났어요. 양쪽의 실험 실행·정보 등록·방화벽을 확인해 주세요.".into())),
        };
        self.0.check_active()?;
        let now = crate::time::now_ms();
        if let Some(peer) = self.0.peers.lock().unwrap().get_mut(session) {
            peer.status.last_checked = now;
            peer.status.last_error = result.as_ref().err().cloned();
            if result.is_ok() {
                peer.status.last_success = now;
            }
        }
        result
    }

    async fn connect(&self, peer: &Peer) -> Result<()> {
        self.0.validate_peer(peer).await?;
        let card = &peer.card;
        let mut config = self.0.identity.client(card)?;
        config.alpn_protocols = vec![self.0.alpn().to_vec()];
        let connector = TlsConnector::from(Arc::new(config));
        let mut addresses = card.addresses.clone();
        if let Some(found) = self.0.discovered.lock().unwrap().get(&card.session) {
            if found.fingerprint == card.fingerprint()? {
                for address in &found.addresses {
                    if !addresses.contains(address) {
                        addresses.push(*address);
                    }
                }
            }
        }
        for address in addresses.into_iter().take(4) {
            let attempt = async {
                let socket = self.0.connect_socket(address).await?;
                let mut stream = connector.connect(NAME.try_into().unwrap(), socket).await
                    .map_err(|_| "장치 인증에 실패했어요. 양쪽에서 최신 연결 정보를 등록했는지 확인해 주세요.")?;
                let remote = stream
                    .get_ref()
                    .1
                    .peer_certificates()
                    .and_then(|c| c.first())
                    .ok_or("상대 인증서가 없어요.")?;
                if remote.as_ref() != card.der()?.as_slice() {
                    return err("등록한 상대 PC의 인증서가 아니에요.");
                }
                if stream.get_ref().1.alpn_protocol() != Some(self.0.alpn()) {
                    return err("연결 인증 방식이 달라요.");
                }
                let nonce = random_id();
                wire::write(
                    &mut stream,
                    &Signal {
                        protocol: VERSION,
                        session: self.0.card.session.clone(),
                        nonce: nonce.clone(),
                    },
                )
                .await?;
                let reply = wire::read(&mut stream).await?;
                if reply.session != card.session || reply.nonce != nonce {
                    return err("상대 PC의 확인 응답이 맞지 않아요.");
                }
                self.0.validate_peer(peer).await?;
                Ok(())
            };
            if let Ok(Ok(())) = tokio::time::timeout(IO_TIMEOUT, attempt).await {
                return Ok(());
            }
        }
        err("상대 PC 연결을 확인하지 못했어요. 같은 LAN인지, 양쪽 정보 등록·실험 실행·통신 허용 상태를 확인해 주세요.")
    }
}

async fn serve(listener: TcpListener, inner: Arc<Inner>) {
    let mut tasks = JoinSet::new();
    let expiry = tokio::time::sleep_until(tokio::time::Instant::from_std(inner.deadline));
    tokio::pin!(expiry);
    let mut stopped = inner.stopped.subscribe();
    if *stopped.borrow() || !inner.running() {
        return;
    }
    loop {
        tokio::select! {
            _=stopped.changed()=>break,
            _ = &mut expiry => {
                if inner.automatic.is_some() { *inner.notice.lock().unwrap()=Some("자동 연결 시간이 끝났어요. 다시 동의하고 시작해 주세요.".into()); }
                break;
            },
            _ = tasks.join_next(), if !tasks.is_empty() => {},
            accepted = listener.accept() => {
                let Ok((socket, _)) = accepted else { break };
                if !inner.running() { break; }
                // 인증 전 연결로 작업 수가 무한히 늘어나지 않게 한다.
                if tasks.len() >= 4 { continue; }
                let peers: Vec<_> = inner.peers.lock().unwrap().values().map(|p| p.card.clone()).collect();
                let mut config=inner.identity.server(&peers).ok();
                if config.is_none() && inner.automatic.is_none() { continue; }
                if let Some(config)=config.as_mut() { config.alpn_protocols=inner.alpns(); }
                let worker = inner.clone();
                tasks.spawn(async move {
                    let _ = tokio::time::timeout(Duration::from_secs(30), respond(socket, config, worker)).await;
                });
            }
        }
    }
    inner.active.store(false, Ordering::Release);
    inner.stopped.send_replace(true);
    tasks.abort_all();
}

async fn respond(
    socket: TcpStream,
    config: Option<rustls::ServerConfig>,
    inner: Arc<Inner>,
) -> Result<()> {
    let mut first = [0u8; 1];
    let n = tokio::time::timeout(IO_TIMEOUT, socket.peek(&mut first))
        .await
        .map_err(|_| "접속 시간이 지났어요.")?
        .map_err(|_| "접속을 읽지 못했어요.")?;
    if n == 0 {
        return err("접속이 닫혔어요.");
    }
    if first[0] == b'S' {
        return tokio::time::timeout(Duration::from_secs(8), auto::respond(socket, inner))
            .await
            .map_err(|_| "자동 연결 시간이 지났어요.")?;
    }
    let config = config.ok_or("등록한 상대 인증서가 없어요.")?;
    let initial = async {
        let mut stream = TlsAcceptor::from(Arc::new(config))
            .accept(socket)
            .await
            .map_err(|_| "장치 인증 실패")?;
        let request = wire::read(&mut stream).await?;
        Ok::<_, String>((stream, request))
    };
    let (mut stream, request) = tokio::time::timeout(IO_TIMEOUT, initial)
        .await
        .map_err(|_| "접속 인증 시간이 지났어요.")??;
    inner.check_active()?;
    let expected = inner
        .peers
        .lock()
        .unwrap()
        .get(&request.session)
        .cloned()
        .ok_or("등록하지 않은 장치")?;
    let remote = stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|c| c.first())
        .ok_or("인증서 없음")?;
    if remote.as_ref() != expected.card.der()?.as_slice() {
        return err("장치 정보가 인증서와 일치하지 않음");
    }
    let courier = stream.get_ref().1.alpn_protocol() == Some(exchange::COURIER_ALPN);
    let host = stream.get_ref().1.alpn_protocol() == Some(host::ALPN);
    if host
        && !(inner.approved.as_ref().is_some_and(|c| c.own.host)
            && expected
                .approval
                .as_ref()
                .is_some_and(|c| c.host && c.courier && c.automatic && c.exchange))
    {
        return err("대표 후보의 중계 동의가 없어요.");
    }
    let transfer = stream.get_ref().1.alpn_protocol() == Some(exchange::ALPN) || courier;
    if courier
        && !(inner.approved.as_ref().is_some_and(|c| c.own.courier)
            && expected.approval.as_ref().is_some_and(|c| c.courier))
    {
        return err("양쪽의 중계 동의가 필요해요.");
    }
    if stream.get_ref().1.alpn_protocol() != Some(inner.alpn())
        && !(transfer && inner.approved.as_ref().is_some_and(|c| c.own.exchange))
        && !host
    {
        return err("연결 인증 방식이 다름");
    }
    inner.validate_peer(&expected).await?;
    wire::write(
        &mut stream,
        &Signal {
            protocol: VERSION,
            session: inner.card.session.clone(),
            nonce: request.nonce.clone(),
        },
    )
    .await?;
    if transfer {
        exchange::respond(&mut stream, &inner, &expected, &request.nonce, courier).await?;
    }
    Ok(())
}

#[cfg(test)]
mod approved_tests;
#[cfg(test)]
mod tests;
