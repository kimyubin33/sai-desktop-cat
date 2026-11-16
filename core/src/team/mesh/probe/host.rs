//! 최근 상호 TLS 응답을 바탕으로 고르는 로컬 대표. 합의·관리자·배타 쓰기 권한이 아니다.
use super::*;

pub(super) const ALPN: &[u8] = b"sai-mesh-host/1";

#[derive(Clone)]
pub(super) struct Options {
    pub tick: Duration,
    pub heartbeat: Duration,
    pub ttl: Duration,
    pub settle: Duration,
    pub hold: Duration,
    pub attempt: Duration,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            tick: Duration::from_millis(250),
            heartbeat: Duration::from_secs(5),
            ttl: Duration::from_secs(15),
            settle: Duration::from_secs(2),
            hold: Duration::from_secs(10),
            attempt: Duration::from_secs(5),
        }
    }
}
#[cfg(test)]
impl Options {
    pub fn testing() -> Self {
        Self {
            tick: Duration::from_millis(20),
            heartbeat: Duration::from_millis(500),
            ttl: Duration::from_secs(5),
            settle: Duration::from_millis(100),
            hold: Duration::from_millis(500),
            attempt: Duration::from_secs(3),
        }
    }
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    pub enabled: bool,
    pub phase: &'static str,
    pub device_id: Option<String>,
    pub session: Option<String>,
    pub is_self: bool,
    pub candidates: usize,
    pub changes: u64,
    pub confirmed_for_seconds: u64,
    pub last_error: Option<String>,
}

struct Seen {
    session: String,
    first: Instant,
    last: Instant,
}
struct Choice {
    own: String,
    session: String,
    started: Instant,
    peers: BTreeMap<String, Seen>,
    selected: Option<(String, String)>,
    selected_at: Instant,
    changes: u64,
    last_error: Option<String>,
    options: Options,
}
impl Choice {
    fn new(own: String, session: String, now: Instant, options: Options) -> Self {
        Self {
            own,
            session,
            started: now,
            peers: BTreeMap::new(),
            selected: None,
            selected_at: now,
            changes: 0,
            last_error: None,
            options,
        }
    }
    fn reconcile(&mut self, allowed: &BTreeMap<String, String>, now: Instant) {
        // mDNS의 ServiceRemoved는 권한 증명이 아니므로 이 목록에 사용하지 않는다.
        self.peers.retain(|id, p| {
            allowed.get(id) == Some(&p.session)
                && now.saturating_duration_since(p.last) < self.options.ttl
        });
    }
    fn confirmed(&mut self, id: String, session: String, now: Instant) {
        match self.peers.get_mut(&id) {
            Some(p)
                if p.session == session
                    && now.saturating_duration_since(p.last) < self.options.ttl =>
            {
                p.last = now
            }
            _ => {
                self.peers.insert(
                    id,
                    Seen {
                        session,
                        first: now,
                        last: now,
                    },
                );
            }
        }
        self.last_error = None;
    }
    fn update(&mut self, now: Instant) -> bool {
        self.peers
            .retain(|_, p| now.saturating_duration_since(p.last) < self.options.ttl);
        let mut candidates = BTreeMap::new();
        if now.saturating_duration_since(self.started) >= self.options.settle {
            candidates.insert(self.own.clone(), self.session.clone());
        }
        for (id, p) in &self.peers {
            if now.saturating_duration_since(p.first) >= self.options.settle {
                candidates.insert(id.clone(), p.session.clone());
            }
        }
        let best = candidates.into_iter().next();
        let selected_live = self.selected.as_ref().is_some_and(|(id, session)| {
            (id == &self.own && session == &self.session)
                || self.peers.get(id).is_some_and(|p| &p.session == session)
        });
        if selected_live && now.saturating_duration_since(self.selected_at) < self.options.hold {
            return false;
        }
        if self.selected != best {
            self.selected = best;
            self.selected_at = now;
            self.changes = self.changes.saturating_add(1);
            return true;
        }
        false
    }
    fn status(&self, now: Instant) -> HostStatus {
        let selected = self.selected.as_ref().filter(|(id, session)| {
            (id == &self.own && session == &self.session)
                || self.peers.get(id).is_some_and(|p| {
                    &p.session == session
                        && now.saturating_duration_since(p.last) < self.options.ttl
                })
        });
        let is_self = selected.is_some_and(|(id, _)| id == &self.own);
        let remaining = selected
            .and_then(|(id, _)| self.peers.get(id))
            .map(|p| {
                self.options
                    .ttl
                    .saturating_sub(now.saturating_duration_since(p.last))
                    .as_secs()
            })
            .unwrap_or(0);
        HostStatus {
            enabled: true,
            phase: if selected.is_none() {
                "settling"
            } else if is_self
                && self
                    .peers
                    .values()
                    .all(|p| now.saturating_duration_since(p.last) >= self.options.ttl)
            {
                "alone"
            } else if is_self {
                "representative"
            } else {
                "peer"
            },
            device_id: selected.map(|(id, _)| id.clone()),
            session: selected.map(|(_, s)| s.clone()),
            is_self,
            candidates: self
                .peers
                .values()
                .filter(|p| now.saturating_duration_since(p.last) < self.options.ttl)
                .count()
                + 1,
            changes: self.changes,
            confirmed_for_seconds: remaining,
            last_error: self.last_error.clone(),
        }
    }
}

pub(super) struct State(Mutex<Choice>);
impl State {
    pub fn new(context: &approved::Context, options: Options) -> Self {
        Self(Mutex::new(Choice::new(
            context.own.proof.body.device_id.clone(),
            context.own.card.session.clone(),
            Instant::now(),
            options,
        )))
    }
    pub fn status(&self) -> HostStatus {
        self.0.lock().unwrap().status(Instant::now())
    }
    pub fn preferred(&self) -> Option<String> {
        self.status().device_id
    }
}

fn eligible(inner: &Inner, peer: &Peer) -> bool {
    inner
        .approved
        .as_ref()
        .is_some_and(|c| c.own.host && c.own.courier)
        && peer
            .approval
            .as_ref()
            .is_some_and(|c| c.host && c.courier && c.automatic && c.exchange)
}
fn current(inner: &Inner, peer: &Peer) -> bool {
    let Some(proof) = peer.approval.as_ref().map(|a| &a.proof) else {
        return false;
    };
    let id = &proof.body.device_id;
    let peers = inner.peers.lock().unwrap();
    if peers
        .values()
        .filter(|p| {
            eligible(inner, p)
                && p.approval
                    .as_ref()
                    .is_some_and(|a| &a.proof.body.device_id == id)
        })
        .count()
        != 1
    {
        return false;
    }
    peers.get(&peer.card.session).is_some_and(|p| {
        p.card.certificate == peer.card.certificate
            && p.card.addresses == peer.card.addresses
            && eligible(inner, p)
            && p.approval.as_ref().is_some_and(|a| {
                &a.proof.body.device_id == id
                    && a.proof.signature == proof.signature
                    && a.proof.body.binding == proof.body.binding
            })
    })
}
async fn heartbeat(inner: &Inner, peer: &Peer) -> Result<()> {
    inner.check_active()?;
    if !eligible(inner, peer) {
        return err("대표 후보의 중계 동의가 없어요.");
    }
    inner.validate_peer(peer).await?;
    let mut config = inner.identity.client(&peer.card)?;
    config.alpn_protocols = vec![ALPN.to_vec()];
    let connector = TlsConnector::from(Arc::new(config));
    // 자동 연결 카드는 선택한 LAN의 단일 주소만 허용한다.
    if peer.card.addresses.len() != 1 {
        return err("대표 후보의 LAN 주소를 확인해 주세요.");
    }
    let socket = inner.connect_socket(peer.card.addresses[0]).await?;
    let mut stream = connector
        .connect(NAME.try_into().unwrap(), socket)
        .await
        .map_err(|_| "대표 후보의 장치 인증에 실패했어요.")?;
    if stream.get_ref().1.alpn_protocol() != Some(ALPN)
        || stream
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|c| c.first())
            .map(|c| c.as_ref())
            != Some(peer.card.der()?.as_slice())
    {
        return err("등록한 대표 후보의 연결이 아니에요.");
    }
    let nonce = random_id();
    wire::write(
        &mut stream,
        &Signal {
            protocol: VERSION,
            session: inner.card.session.clone(),
            nonce: nonce.clone(),
        },
    )
    .await?;
    let reply = wire::read(&mut stream).await?;
    if reply.session != peer.card.session || reply.nonce != nonce {
        return err("대표 후보의 새 응답이 아니에요.");
    }
    inner.check_active()?;
    inner.validate_peer(peer).await?;
    if !current(inner, peer) {
        return err("대표 후보의 실행 정보가 바뀌었어요.");
    }
    Ok(())
}

pub(super) struct Runtime(JoinHandle<()>);
impl Runtime {
    pub fn start(inner: Arc<Inner>, options: Options) -> Self {
        Self(tokio::spawn(run(inner, options)))
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn run(inner: Arc<Inner>, options: Options) {
    let state = inner.host.as_ref().unwrap();
    let mut interval = tokio::time::interval(options.tick);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut stopped = inner.stopped.subscribe();
    let mut tasks = JoinSet::<(Peer, Result<()>)>::new();
    let mut pending = std::collections::BTreeSet::new();
    let mut due = BTreeMap::<String, Instant>::new();
    let mut validated = Instant::now() - Duration::from_secs(2);
    if *stopped.borrow() {
        return;
    }
    loop {
        tokio::select! {
            biased;
            _=stopped.changed()=>break,
            result=tasks.join_next(),if !tasks.is_empty()=>{
                if let Some(Ok((peer,result)))=result {
                    let id=peer.approval.as_ref().unwrap().proof.body.device_id.clone();
                    pending.remove(&id);
                    due.insert(id.clone(),Instant::now()+options.heartbeat);
                    let accepted=result.is_ok() && inner.running() && current(&inner,&peer);
                    let mut choice=state.0.lock().unwrap();
                    if accepted {
                        choice.confirmed(id,peer.card.session,Instant::now());
                    } else if let Err(error)=result { choice.last_error=Some(error); }
                    if choice.update(Instant::now()) { if let Some(a)=&inner.automatic { a.wake.notify_one(); } }
                }
            },
            _=interval.tick()=>{
                if !inner.running() { break; }
                if validated.elapsed()>=Duration::from_secs(1) {
                    let context=inner.approved.as_ref().unwrap();
                    let proof=&context.own.proof.body;
                    let (v,s)=(context.vault.clone(),crate::team::mesh::identity::automatic::AutoScope{
                        team:context.team.clone(),own:proof.device_id.clone(),revision:proof.revision,head:proof.membership_head.clone()});
                    if let Err(error)=super::exchange::blocking(move||v.validate_auto_scope(&s)).await {
                        *inner.notice.lock().unwrap()=Some(error);
                        inner.active.store(false,Ordering::Release);
                        inner.stopped.send_replace(true);
                        break;
                    }
                    validated=Instant::now();
                }
                let mut grouped=BTreeMap::<String,Vec<Peer>>::new();
                for peer in inner.peers.lock().unwrap().values().filter(|p|eligible(&inner,p)) {
                    grouped.entry(peer.approval.as_ref().unwrap().proof.body.device_id.clone()).or_default().push(peer.clone());
                }
                // 같은 장치의 여러 실행을 임의로 최신이라고 고르거나 표를 늘리지 않는다.
                let peers:Vec<_>=grouped.into_values().filter_map(|mut group|if group.len()==1 { group.pop() } else { None }).collect();
                let allowed=peers.iter().map(|p|(p.approval.as_ref().unwrap().proof.body.device_id.clone(),p.card.session.clone())).collect();
                let now=Instant::now();
                {
                    let mut choice=state.0.lock().unwrap();
                    choice.reconcile(&allowed,now);
                    if choice.update(now) { if let Some(a)=&inner.automatic { a.wake.notify_one(); } }
                }
                due.retain(|id,_|allowed.contains_key(id));
                // 한 PC의 장애가 다른 후보 확인을 막지 않도록 최대 2개를 동시에 확인한다.
                let mut ready:Vec<_>=peers.into_iter().filter(|p|{
                    let id=&p.approval.as_ref().unwrap().proof.body.device_id;
                    !pending.contains(id) && due.get(id).is_none_or(|t|*t<=now)
                }).collect();
                ready.sort_by_key(|p|due.get(&p.approval.as_ref().unwrap().proof.body.device_id).copied());
                for peer in ready.into_iter().take(2usize.saturating_sub(tasks.len())) {
                    let id=peer.approval.as_ref().unwrap().proof.body.device_id.clone();
                    pending.insert(id);
                    let worker=inner.clone(); let timeout=options.attempt;
                    tasks.spawn(async move {
                        let result=tokio::time::timeout(timeout,heartbeat(&worker,&peer)).await
                            .unwrap_or_else(|_|Err("대표 후보의 응답 시간이 지났어요.".into()));
                        (peer,result)
                    });
                }
            }
        }
    }
    tasks.abort_all();
}

/// 대표 연결은 기존 주기, 나머지는 그 3배 주기로 교환하며 모두 계속 유지한다.
pub(super) fn exchange_idle(base: Duration, own: &str, peer: &str, host: Option<&str>) -> Duration {
    if host.is_some_and(|id| id != own && id != peer) {
        base.saturating_mul(3)
    } else {
        base
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
