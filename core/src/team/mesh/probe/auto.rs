//! LAN 발견은 후보만 만들고, 암호화된 새 도전과 장치 서명으로 연결 정보를 갱신한다.
use super::*;
use crate::team::mesh::identity::automatic::{AutoRoutes, AutoScope, BootAad, Capsule, Stage};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MAGIC: &[u8; 8] = b"SAIAUTO1";
const MAX_FRAME: usize = 16384;

#[derive(Clone)]
pub(super) struct Options {
    pub courier: bool,
    pub host: super::host::Options,
    pub mdns: bool,
    pub tick: Duration,
    pub idle: Duration,
    pub retry: Duration,
    pub retry_max: Duration,
    pub gate_gap: Duration,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            courier: false,
            host: super::host::Options::default(),
            mdns: true,
            tick: Duration::from_millis(500),
            idle: Duration::from_secs(5),
            retry: Duration::from_secs(1),
            retry_max: Duration::from_secs(30),
            gate_gap: Duration::from_millis(250),
        }
    }
}
#[cfg(test)]
impl Options {
    pub fn testing() -> Self {
        Self {
            courier: false,
            host: super::host::Options::testing(),
            mdns: false,
            tick: Duration::from_millis(20),
            idle: Duration::from_millis(150),
            retry: Duration::from_millis(50),
            retry_max: Duration::from_millis(250),
            gate_gap: Duration::from_millis(5),
        }
    }
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomaticStatus {
    pub enabled: bool,
    pub phase: &'static str,
    pub discovered: usize,
    pub paired: usize,
    pub successes: u64,
    pub last_success: i64,
    pub last_error: Option<String>,
    pub retry_in_seconds: u64,
}

#[derive(Clone, Serialize)]
pub(super) struct Advertisement {
    pub version: u8,
    pub session: String,
    pub tags: Vec<String>,
    pub addresses: Vec<SocketAddrV4>,
}
#[derive(Clone)]
struct Candidate {
    session: String,
    tag: String,
    addresses: Vec<SocketAddrV4>,
}

struct Network {
    address: Ipv4Addr,
    mask: Ipv4Addr,
}
impl Network {
    fn new(address: Ipv4Addr) -> Result<Self> {
        #[cfg(test)]
        if address.is_loopback() {
            return Ok(Self {
                address,
                mask: Ipv4Addr::new(255, 0, 0, 0),
            });
        }
        for interface in if_addrs::get_if_addrs().map_err(|_| "LAN 범위를 확인하지 못했어요.")?
        {
            if let if_addrs::IfAddr::V4(v4) = interface.addr {
                if v4.ip == address {
                    return Ok(Self {
                        address,
                        mask: v4.netmask,
                    });
                }
            }
        }
        err("선택한 LAN의 주소와 범위를 확인하지 못했어요.")
    }
    fn permits(&self, address: SocketAddrV4) -> bool {
        if address.port() == 0 {
            return false;
        }
        let ip = *address.ip();
        let private = ip.is_private();
        #[cfg(test)]
        let private = private || (self.address.is_loopback() && ip.is_loopback());
        let (ip, local, mask) = (u32::from(ip), u32::from(self.address), u32::from(self.mask));
        let host = !mask;
        private
            && (ip & mask) == (local & mask)
            && (host <= 1 || (ip & host != 0 && ip & host != host))
    }
}

pub(super) struct State {
    routes: AutoRoutes,
    own_session: String,
    local: SocketAddrV4,
    network: Network,
    candidates: Mutex<BTreeMap<String, Candidate>>,
    stats: Mutex<AutomaticStatus>,
    gate: tokio::sync::Semaphore,
    next_accept: Mutex<Instant>,
    options: Options,
    pub wake: tokio::sync::Notify,
}
impl State {
    pub async fn prepare(
        context: &approved::Context,
        local: SocketAddrV4,
        options: Options,
    ) -> Result<Self> {
        let b = &context.own.proof.body;
        let scope = AutoScope {
            team: context.team.clone(),
            own: b.device_id.clone(),
            revision: b.revision,
            head: b.membership_head.clone(),
        };
        let vault = context.vault.clone();
        let routes = blocking(move || vault.auto_routes(&scope)).await?;
        Ok(Self {
            routes,
            own_session: context.own.card.session.clone(),
            local,
            network: Network::new(*local.ip())?,
            candidates: Mutex::new(BTreeMap::new()),
            stats: Mutex::new(AutomaticStatus {
                enabled: true,
                phase: "discovering",
                ..Default::default()
            }),
            gate: tokio::sync::Semaphore::new(1),
            next_accept: Mutex::new(Instant::now()),
            options,
            wake: tokio::sync::Notify::new(),
        })
    }
    pub fn advertisement(&self) -> Result<Advertisement> {
        Ok(Advertisement {
            version: 1,
            session: self.own_session.clone(),
            tags: self.routes.offers(&self.own_session)?,
            addresses: vec![self.local],
        })
    }
    pub fn permits(&self, address: SocketAddrV4) -> bool {
        self.network.permits(address)
    }
    pub fn ingest(&self, advertisement: Advertisement) -> bool {
        if advertisement.version != 1
            || advertisement.session == self.own_session
            || !card::valid_session(&advertisement.session)
            || advertisement.tags.is_empty()
            || advertisement.tags.len() > 8
            || advertisement.addresses.is_empty()
            || advertisement.addresses.len() > 4
            || advertisement
                .addresses
                .iter()
                .any(|a| !self.network.permits(*a))
        {
            return false;
        }
        let mut matches = BTreeMap::new();
        for tag in advertisement.tags {
            if let Some(peer) = self.routes.match_offer(&advertisement.session, &tag) {
                matches.insert(peer, tag);
            }
        }
        if matches.len() != 1 {
            return false;
        }
        let (peer, tag) = matches.into_iter().next().unwrap();
        self.candidates.lock().unwrap().insert(
            peer,
            Candidate {
                session: advertisement.session,
                tag,
                addresses: advertisement.addresses,
            },
        );
        // 광고만으로 실패 횟수·재시도 시각을 초기화하지 않는다.
        true
    }
    pub fn forget(&self, session: &str) {
        self.candidates
            .lock()
            .unwrap()
            .retain(|_, c| c.session != session);
    }
    pub fn status(&self, running: bool) -> AutomaticStatus {
        let mut status = self.stats.lock().unwrap().clone();
        status.enabled = running;
        status.discovered = self.candidates.lock().unwrap().len();
        status
    }
    pub fn note_success(&self) {
        let mut stats = self.stats.lock().unwrap();
        stats.successes = stats.successes.saturating_add(1);
        stats.last_success = crate::time::now_ms();
        stats.last_error = None;
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    version: u8,
    session: String,
    tag: String,
    client_nonce: String,
}
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    deny_unknown_fields,
    rename_all = "camelCase"
)]
enum Frame {
    Hello(Hello),
    Challenge { nonce: String },
    Request(Capsule),
    Reply(Capsule),
}

async fn read(stream: &mut (impl AsyncRead + Unpin)) -> Result<Frame> {
    let n = stream
        .read_u32()
        .await
        .map_err(|_| "자동 연결 응답이 닫혔어요.")? as usize;
    if n == 0 || n > MAX_FRAME {
        return err("자동 연결 메시지 크기 한도를 넘었어요.");
    }
    let mut bytes = vec![0; n];
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(|_| "자동 연결 응답이 끊겼어요.")?;
    serde_json::from_slice(&bytes).map_err(|_| "자동 연결 메시지 형식이 아니에요.".into())
}
async fn write(stream: &mut (impl AsyncWrite + Unpin), frame: &Frame) -> Result<()> {
    let bytes = serde_json::to_vec(frame).map_err(|_| "자동 연결 정보를 만들지 못했어요.")?;
    if bytes.len() > MAX_FRAME {
        return err("자동 연결 메시지 크기 한도를 넘었어요.");
    }
    stream
        .write_u32(bytes.len() as u32)
        .await
        .map_err(|_| "자동 연결 응답을 보내지 못했어요.")?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|_| "자동 연결 응답이 끊겼어요.")?;
    stream
        .flush()
        .await
        .map_err(|_| "자동 연결 응답을 보내지 못했어요.".into())
}
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| "자동 연결 정보를 확인하지 못했어요.")?
}

async fn parse_peer(inner: &Inner, expected: &str, raw: String) -> Result<ApprovedCard> {
    let context = inner
        .approved
        .as_ref()
        .ok_or("승인된 연결이 아니에요.")?
        .clone();
    let peer = blocking(move || context.parse_peer(&raw)).await?;
    if !peer.automatic
        || !peer.exchange
        || peer.proof.body.device_id != expected
        || peer.card.addresses.len() != 1
    {
        return err("상대의 자동 연결 동의와 장치 정보가 일치하지 않아요.");
    }
    Ok(peer)
}

async fn register(inner: Arc<Inner>, peer: ApprovedCard) -> Result<()> {
    blocking(move || register_inner(&inner, peer)).await
}
fn register_inner(inner: &Arc<Inner>, peer: ApprovedCard) -> Result<()> {
    inner.check_active()?;
    let id = &peer.proof.body.device_id;
    if id == &inner.approved.as_ref().unwrap().own.proof.body.device_id {
        return err("내 장치의 연결 정보예요.");
    }
    // 이 함수는 새 서버 도전·장치 서명 검증을 마친 경로에서만 호출한다.
    let raw = serde_json::to_string(&peer).map_err(|_| "자동 연결 정보를 준비하지 못했어요.")?;
    {
        let mut peers = inner.peers.lock().unwrap();
        peers.retain(|session, old| {
            session == &peer.card.session
                || old
                    .approval
                    .as_ref()
                    .is_none_or(|p| &p.proof.body.device_id != id)
        });
    }
    add_peer(inner, &raw)?;
    if let Some(registered) = inner.peers.lock().unwrap().get_mut(&peer.card.session) {
        if registered.card.certificate == peer.card.certificate
            && registered.approval.as_ref().is_some_and(|p| {
                p.proof.signature == peer.proof.signature
                    && p.proof.body.binding == peer.proof.body.binding
            })
        {
            registered.bootstrapped = true;
        }
    }
    Ok(())
}

pub(super) async fn respond(mut socket: TcpStream, inner: Arc<Inner>) -> Result<()> {
    inner.check_active()?;
    let state = inner
        .automatic
        .as_ref()
        .ok_or("자동 연결을 허용하지 않았어요.")?;
    let _permit = state
        .gate
        .try_acquire()
        .map_err(|_| "다른 자동 연결을 확인 중이에요.")?;
    let source = match socket
        .peer_addr()
        .map_err(|_| "접속 주소를 확인하지 못했어요.")?
    {
        std::net::SocketAddr::V4(a) => a,
        _ => return err("LAN IPv4 연결만 지원해요."),
    };
    if !state.permits(source) {
        return err("선택한 LAN 범위의 접속이 아니에요.");
    }
    let mut magic = [0u8; 8];
    socket
        .read_exact(&mut magic)
        .await
        .map_err(|_| "자동 연결 요청이 끊겼어요.")?;
    if &magic != MAGIC {
        return err("자동 연결 요청이 아니에요.");
    }
    let hello = match read(&mut socket).await? {
        Frame::Hello(h) => h,
        _ => return err("자동 연결 시작 정보가 필요해요."),
    };
    if hello.version != 1
        || hello.session != state.own_session
        || !card::valid_session(&hello.client_nonce)
    {
        return err("자동 연결 실행 정보가 달라요.");
    }
    let peer_id = state
        .routes
        .receiver(&hello.session, &hello.tag)
        .ok_or("승인 장치의 발견 표식이 아니에요.")?;
    {
        let mut next = state.next_accept.lock().unwrap();
        let now = Instant::now();
        if now < *next {
            return err("잠시 후 자동 연결을 다시 확인해요.");
        }
        *next = now + state.options.gate_gap;
    }
    let nonce = random_id();
    write(
        &mut socket,
        &Frame::Challenge {
            nonce: nonce.clone(),
        },
    )
    .await?;
    let capsule = match read(&mut socket).await? {
        Frame::Request(c) => c,
        _ => return err("암호화된 자동 연결 증명이 필요해요."),
    };
    let mut aad = BootAad {
        stage: Stage::Request,
        session: hello.session,
        tag: hello.tag,
        server_nonce: nonce,
        client_nonce: hello.client_nonce,
    };
    let context = inner.approved.as_ref().unwrap();
    let (vault, scope, id, a) = (
        context.vault.clone(),
        state.routes.scope.clone(),
        peer_id.clone(),
        aad.clone(),
    );
    let raw = blocking(move || {
        vault
            .open_auto(&scope, &id, &a, &capsule)
            .map(|s| s.to_string())
    })
    .await?;
    let peer = parse_peer(&inner, &peer_id, raw).await?;
    if peer.card.addresses[0].ip() != source.ip() || !state.permits(peer.card.addresses[0]) {
        return err("서명된 자동 연결 주소와 접속한 LAN 주소가 달라요.");
    }
    aad.stage = Stage::Reply;
    let own =
        serde_json::to_string(&context.own).map_err(|_| "내 연결 정보를 준비하지 못했어요.")?;
    let (vault, scope, id, a) = (
        context.vault.clone(),
        state.routes.scope.clone(),
        peer_id,
        aad,
    );
    let reply = blocking(move || vault.seal_auto(&scope, &id, &a, &own)).await?;
    inner.check_active()?;
    register(inner.clone(), peer).await?;
    write(&mut socket, &Frame::Reply(reply)).await
}

async fn bootstrap(inner: Arc<Inner>, peer_id: String, candidate: Candidate) -> Result<()> {
    let state = inner.automatic.as_ref().unwrap();
    let context = inner.approved.as_ref().unwrap();
    for address in candidate.addresses {
        inner.check_active()?;
        let Ok(Ok(mut socket)) =
            tokio::time::timeout(IO_TIMEOUT, inner.connect_socket(address)).await
        else {
            continue;
        };
        socket
            .write_all(MAGIC)
            .await
            .map_err(|_| "자동 연결 시작 요청을 보내지 못했어요.")?;
        let client_nonce = random_id();
        write(
            &mut socket,
            &Frame::Hello(Hello {
                version: 1,
                session: candidate.session.clone(),
                tag: candidate.tag.clone(),
                client_nonce: client_nonce.clone(),
            }),
        )
        .await?;
        let server_nonce = match read(&mut socket).await? {
            Frame::Challenge { nonce } if card::valid_session(&nonce) => nonce,
            _ => return err("자동 연결의 새 도전이 필요해요."),
        };
        let mut aad = BootAad {
            stage: Stage::Request,
            session: candidate.session.clone(),
            tag: candidate.tag.clone(),
            server_nonce,
            client_nonce,
        };
        let own =
            serde_json::to_string(&context.own).map_err(|_| "내 연결 정보를 준비하지 못했어요.")?;
        let (vault, scope, id, a) = (
            context.vault.clone(),
            state.routes.scope.clone(),
            peer_id.clone(),
            aad.clone(),
        );
        let request = blocking(move || vault.seal_auto(&scope, &id, &a, &own)).await?;
        inner.check_active()?;
        write(&mut socket, &Frame::Request(request)).await?;
        let capsule = match read(&mut socket).await? {
            Frame::Reply(c) => c,
            _ => return err("상대의 암호화된 자동 연결 증명이 필요해요."),
        };
        aad.stage = Stage::Reply;
        let (vault, scope, id, a) = (
            context.vault.clone(),
            state.routes.scope.clone(),
            peer_id.clone(),
            aad,
        );
        let raw = blocking(move || {
            vault
                .open_auto(&scope, &id, &a, &capsule)
                .map(|s| s.to_string())
        })
        .await?;
        let peer = parse_peer(&inner, &peer_id, raw).await?;
        if peer.card.session != candidate.session || peer.card.addresses != vec![address] {
            return err("발견한 주소와 상대의 서명된 자동 연결 주소가 달라요.");
        }
        inner.check_active()?;
        register(inner.clone(), peer).await?;
        return Ok(());
    }
    err("발견한 승인 장치에 연결하지 못했어요. 대기 기록을 유지하고 다시 시도해요.")
}

pub(super) struct Runtime {
    task: JoinHandle<()>,
    discovery: Option<super::auto_discovery::Discovery>,
}
impl Runtime {
    pub fn start(inner: Arc<Inner>, address: SocketAddrV4, options: Options) -> Result<Self> {
        let discovery =
            if options.mdns && !inner.automatic.as_ref().unwrap().routes.peers().is_empty() {
                Some(super::auto_discovery::Discovery::start(
                    inner.clone(),
                    address,
                )?)
            } else {
                None
            };
        let task = tokio::spawn(run(inner, options));
        Ok(Self { task, discovery })
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.task.abort();
        self.discovery.take();
    }
}

struct Plan {
    session: Option<String>,
    boot_fail: u32,
    exchange_fail: u32,
    boot_due: Instant,
    exchange_due: Instant,
}
impl Plan {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            session: None,
            boot_fail: 0,
            exchange_fail: 0,
            boot_due: now,
            exchange_due: now,
        }
    }
}
fn retry_delay(options: &Options, failures: u32, jitter: u8) -> Duration {
    let base = options
        .retry
        .saturating_mul(1u32 << failures.saturating_sub(1).min(16))
        .min(options.retry_max);
    base.saturating_add(base.mul_f64(f64::from(jitter) / 1020.0))
        .min(options.retry_max)
}
enum Action {
    Bootstrap(String, Candidate),
    Exchange(String, String),
}

async fn run(inner: Arc<Inner>, options: Options) {
    let state = inner.automatic.as_ref().unwrap();
    let mut plans: BTreeMap<_, _> = state
        .routes
        .peers()
        .into_iter()
        .map(|id| (id, Plan::new()))
        .collect();
    let ids: Vec<_> = plans.keys().cloned().collect();
    let mut turn = 0usize;
    let mut favor_host = true;
    let mut interval = tokio::time::interval(options.tick);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut stopped = inner.stopped.subscribe();
    let mut validated = Instant::now() - Duration::from_secs(2);
    loop {
        let woke = tokio::select! { _=stopped.changed()=>break,_=state.wake.notified()=>true,_=interval.tick()=>false };
        if !inner.running() {
            break;
        }
        let now = Instant::now();
        if validated.elapsed() >= Duration::from_secs(1) {
            let context = inner.approved.as_ref().unwrap();
            let (vault, scope) = (context.vault.clone(), state.routes.scope.clone());
            if let Err(error) = blocking(move || vault.validate_auto_scope(&scope)).await {
                *inner.notice.lock().unwrap() = Some(error.clone());
                state.stats.lock().unwrap().last_error = Some(error);
                inner.active.store(false, Ordering::Release);
                inner.stopped.send_replace(true);
                break;
            }
            validated = Instant::now();
            if !inner.running() {
                break;
            }
        }
        let peers: BTreeMap<_, _> = inner
            .peers
            .lock()
            .unwrap()
            .values()
            .filter_map(|p| {
                p.approval
                    .as_ref()
                    .filter(|a| a.automatic && a.exchange)
                    .map(|a| (a.proof.body.device_id.clone(), p.card.session.clone()))
            })
            .collect();
        for (id, plan) in &mut plans {
            let session = peers.get(id).cloned();
            if session != plan.session {
                plan.session = session;
                plan.exchange_fail = 0;
                plan.exchange_due = now;
            }
            if woke && plan.exchange_fail == 0 {
                plan.exchange_due = now;
            }
        }
        state.stats.lock().unwrap().paired = peers.len();
        if inner.checking.load(Ordering::Acquire) {
            continue;
        }
        let candidates = state.candidates.lock().unwrap().clone();
        let verified: BTreeMap<_, _> = inner
            .peers
            .lock()
            .unwrap()
            .values()
            .filter_map(|p| {
                p.approval
                    .as_ref()
                    .filter(|_| p.bootstrapped)
                    .map(|a| (a.proof.body.device_id.clone(), p.card.session.clone()))
            })
            .collect();
        let mut action = None;
        let preferred = inner.host.as_ref().and_then(|h| h.preferred());
        if favor_host {
            if let Some(id) = preferred
                .as_ref()
                .filter(|id| state.routes.scope.own < **id)
            {
                if let Some(plan) = plans.get(id) {
                    if now >= plan.exchange_due {
                        if let Some(session) = &plan.session {
                            action = Some(Action::Exchange(id.clone(), session.clone()));
                        }
                    }
                }
            }
        }
        for step in 0..if action.is_none() { ids.len() } else { 0 } {
            let index = (turn + step) % ids.len();
            let id = &ids[index];
            let plan = &plans[id];
            if let Some(candidate) = candidates.get(id) {
                if verified.get(id).map(String::as_str) != Some(candidate.session.as_str())
                    && now >= plan.boot_due
                {
                    action = Some(Action::Bootstrap(id.clone(), candidate.clone()));
                    turn = (index + 1) % ids.len();
                    break;
                }
            }
            if state.routes.scope.own < *id && now >= plan.exchange_due {
                if let Some(session) = &plan.session {
                    action = Some(Action::Exchange(id.clone(), session.clone()));
                    turn = (index + 1) % ids.len();
                    break;
                }
            }
        }
        if action.is_some() {
            favor_host = !favor_host;
        }
        match action {
            Some(Action::Bootstrap(id, candidate)) => {
                state.stats.lock().unwrap().phase = "connecting";
                let result = tokio::select! { _=stopped.changed()=>break,result=tokio::time::timeout(Duration::from_secs(8),bootstrap(inner.clone(),id.clone(),candidate))=>result.unwrap_or_else(|_|Err("자동 연결 확인 시간이 지났어요.".into())) };
                let plan = plans.get_mut(&id).unwrap();
                match result {
                    Ok(()) => {
                        plan.boot_fail = 0;
                        plan.boot_due = Instant::now() + options.idle;
                    }
                    Err(error) => {
                        plan.boot_fail = plan.boot_fail.saturating_add(1);
                        plan.boot_due =
                            Instant::now() + retry_delay(&options, plan.boot_fail, rand::random());
                        state.stats.lock().unwrap().last_error = Some(error);
                    }
                }
            }
            Some(Action::Exchange(id, session)) => {
                state.stats.lock().unwrap().phase = "exchanging";
                let result = Checker(inner.clone()).exchange(&session).await;
                let plan = plans.get_mut(&id).unwrap();
                match result {
                    Ok(report) => {
                        plan.exchange_fail = 0;
                        plan.exchange_due = Instant::now()
                            + if report.more {
                                options.tick
                            } else {
                                let host = inner.host.as_ref().and_then(|h| h.preferred());
                                super::host::exchange_idle(
                                    options.idle,
                                    &state.routes.scope.own,
                                    &id,
                                    host.as_deref(),
                                )
                            };
                        state.note_success();
                    }
                    Err(error) => {
                        plan.exchange_fail = plan.exchange_fail.saturating_add(1);
                        plan.exchange_due = Instant::now()
                            + retry_delay(&options, plan.exchange_fail, rand::random());
                        state.stats.lock().unwrap().last_error = Some(error);
                    }
                }
            }
            None => {
                let mut stats = state.stats.lock().unwrap();
                stats.phase = if ids.is_empty() {
                    "no-peers"
                } else if peers.is_empty() {
                    "discovering"
                } else {
                    "waiting"
                };
            }
        }
        let now = Instant::now();
        let next = plans
            .iter()
            .flat_map(|(id, p)| {
                [
                    candidates
                        .get(id)
                        .filter(|c| {
                            verified.get(id).map(String::as_str) != Some(c.session.as_str())
                        })
                        .map(|_| p.boot_due),
                    p.session
                        .as_ref()
                        .filter(|_| state.routes.scope.own < *id)
                        .map(|_| p.exchange_due),
                ]
            })
            .flatten()
            .min();
        state.stats.lock().unwrap().retry_in_seconds = next
            .map(|t| {
                let d = t.saturating_duration_since(now);
                d.as_secs() + u64::from(d.subsec_nanos() > 0)
            })
            .unwrap_or(0);
    }
    state.stats.lock().unwrap().phase = "stopped";
}

impl Checker {
    pub fn wake(&self) {
        if let Some(state) = &self.0.automatic {
            state.wake.notify_one();
        }
    }
}

#[cfg(test)]
#[path = "auto_tests.rs"]
mod tests;
