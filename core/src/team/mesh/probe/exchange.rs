//! 인증 신호 이후 두 방향으로 제한된 수의 암호문과 커밋된 저장 확인을 교환한다.
use super::*;
use crate::team::mesh::identity::{
    transfer::{Delivery, Frontier, Receipt, Scope, MAX_FRAME},
    Vault,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub(super) const ALPN: &[u8] = b"sai-mesh-transfer/1";
pub(super) const COURIER_ALPN: &[u8] = b"sai-mesh-transfer/2";
const BATCH: usize = 8;

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeReport {
    pub sent: usize,
    pub received: usize,
    pub sent_headers: usize,
    pub received_headers: usize,
    pub more: bool,
    pub relay_sent: usize,
    pub relay_received: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    deny_unknown_fields,
    rename_all = "camelCase"
)]
enum Frame {
    Frontier(Frontier),
    Delivery(Delivery),
    Receipt(Receipt),
    End { more: bool },
}

async fn read(stream: &mut (impl AsyncRead + Unpin)) -> Result<Frame> {
    let n = stream
        .read_u32()
        .await
        .map_err(|_| "기록 교환 연결이 닫혔어요.")? as usize;
    if n == 0 || n > MAX_FRAME {
        return err("기록 교환 프레임 크기 한도를 넘었어요.");
    }
    let mut bytes = vec![0; n];
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(|_| "기록 교환 중 연결이 끊겼어요.")?;
    serde_json::from_slice(&bytes).map_err(|_| "호환되는 기록 교환 메시지가 아니에요.".into())
}

async fn write(stream: &mut (impl AsyncWrite + Unpin), frame: &Frame) -> Result<()> {
    let bytes = serde_json::to_vec(frame).map_err(|_| "기록 교환 메시지를 만들지 못했어요.")?;
    if bytes.len() > MAX_FRAME {
        return err("기록 교환 프레임 크기 한도를 넘었어요.");
    }
    stream
        .write_u32(bytes.len() as u32)
        .await
        .map_err(|_| "기록 교환 연결이 끊겼어요.")?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|_| "기록 교환 중 연결이 끊겼어요.")?;
    stream
        .flush()
        .await
        .map_err(|_| "기록 교환 내용을 보내지 못했어요.".into())
}

pub(super) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| "암호화된 기록을 처리하지 못했어요.")?
}

pub(super) fn scope(inner: &Inner, peer: &Peer) -> Result<(Vault, Scope)> {
    let context = inner
        .approved
        .as_ref()
        .filter(|c| c.own.exchange)
        .ok_or("먼저 기록 교환에 동의하고 연결을 시작해 주세요.")?;
    let remote = peer
        .approval
        .as_ref()
        .filter(|p| p.exchange)
        .ok_or("상대도 기록 교환에 동의한 연결 정보를 등록해야 해요.")?;
    Ok((
        context.vault.clone(),
        Scope {
            team: context.team.clone(),
            own: context.own.proof.body.device_id.clone(),
            peer: remote.proof.body.device_id.clone(),
            revision: context.own.proof.body.revision,
            head: context.own.proof.body.membership_head.clone(),
        },
    ))
}

async fn push(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    inner: &Inner,
    peer: &Peer,
    challenge: &str,
) -> Result<(usize, usize, bool)> {
    inner.check_active()?;
    inner.validate_peer(peer).await?;
    let (vault, scope) = scope(inner, peer)?;
    let frontier = match read(stream).await? {
        Frame::Frontier(f) => f,
        _ => return err("먼저 상대의 저장 경계가 필요해요."),
    };
    let (v, s, n) = (vault.clone(), scope.clone(), challenge.to_string());
    let mut cursor = blocking(move || v.begin_push(&s, &frontier, &n)).await?;
    let mut bodies = 0;
    let mut headers = 0;
    for _ in 0..BATCH {
        inner.check_active()?;
        inner.validate_peer(peer).await?;
        let (v, s, c) = (vault.clone(), scope.clone(), cursor.clone());
        let Some(delivery) = blocking(move || v.next_delivery(&s, &c)).await? else {
            break;
        };
        write(stream, &Frame::Delivery(delivery.clone())).await?;
        let receipt = match read(stream).await? {
            Frame::Receipt(r) => r,
            _ => return err("상대의 서명된 저장 확인이 필요해요."),
        };
        let (v, s, n, mut c) = (
            vault.clone(),
            scope.clone(),
            challenge.to_string(),
            cursor.clone(),
        );
        let (stored, next) = blocking(move || {
            let stored = v.accept_receipt(&s, &delivery, &receipt, &n, &mut c)?;
            Ok((stored, c))
        })
        .await?;
        cursor = next;
        if stored {
            bodies += 1;
        } else {
            headers += 1;
        }
    }
    let (v, s, c) = (vault, scope, cursor);
    let more = blocking(move || v.next_delivery(&s, &c).map(|d| d.is_some())).await?;
    write(stream, &Frame::End { more }).await?;
    Ok((bodies, headers, more))
}

async fn pull(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    inner: &Inner,
    peer: &Peer,
    challenge: &str,
) -> Result<(usize, usize, bool)> {
    inner.check_active()?;
    inner.validate_peer(peer).await?;
    let (vault, scope) = scope(inner, peer)?;
    let (v, s, n) = (vault.clone(), scope.clone(), challenge.to_string());
    let frontier = blocking(move || v.receive_frontier(&s, &n)).await?;
    write(stream, &Frame::Frontier(frontier)).await?;
    let mut bodies = 0;
    let mut headers = 0;
    for index in 0..=BATCH {
        match read(stream).await? {
            Frame::Delivery(delivery) if index < BATCH => {
                inner.check_active()?;
                inner.validate_peer(peer).await?;
                let (v, s, n) = (vault.clone(), scope.clone(), challenge.to_string());
                let receipt = blocking(move || v.store_delivery(&s, &delivery, &n)).await?;
                if receipt.has_body() {
                    bodies += 1;
                } else {
                    headers += 1;
                }
                inner.check_active()?;
                write(stream, &Frame::Receipt(receipt)).await?;
            }
            Frame::End { more } => return Ok((bodies, headers, more)),
            _ => return err("기록 교환의 순서 또는 개수 한도를 넘었어요."),
        }
    }
    err("기록 교환 개수 한도를 넘었어요.")
}

pub(super) async fn respond(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    inner: &Inner,
    peer: &Peer,
    challenge: &str,
    courier: bool,
) -> Result<()> {
    pull(stream, inner, peer, challenge).await?;
    push(stream, inner, peer, challenge).await?;
    if courier {
        super::courier::sync(stream, inner, peer, challenge, false).await?;
    }
    if let Some(state) = &inner.automatic {
        state.note_success();
    }
    Ok(())
}

impl Checker {
    pub async fn exchange(&self, session: &str) -> Result<ExchangeReport> {
        self.0.check_active()?;
        let peer = self
            .0
            .peers
            .lock()
            .unwrap()
            .get(session)
            .cloned()
            .ok_or("먼저 상대의 연결 정보를 등록해 주세요.")?;
        scope(&self.0, &peer)?;
        if self.0.checking.swap(true, Ordering::AcqRel) {
            return err("앞의 연결 작업이 끝날 때까지 기다려 주세요.");
        }
        struct Busy<'a>(&'a AtomicBool);
        impl Drop for Busy<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _busy = Busy(&self.0.checking);
        let mut stopped = self.0.stopped.subscribe();
        if *stopped.borrow() {
            return err("기록 교환이 중지됐어요.");
        }
        let result = tokio::select! {
            biased;
            _=stopped.changed()=>Err("기록 교환이 중지됐어요. 확인받지 못한 원본은 대기로 유지해요.".into()),
            result=tokio::time::timeout(Duration::from_secs(30),self.exchange_inner(&peer))=>result.unwrap_or_else(|_|Err("기록 교환 시간이 지났어요. 확인받지 못한 원본은 대기로 유지해요.".into())),
        };
        let now = crate::time::now_ms();
        if let Some(p) = self.0.peers.lock().unwrap().get_mut(session) {
            p.status.last_checked = now;
            p.status.last_error = result.as_ref().err().cloned();
            if result.is_ok() {
                p.status.last_success = now;
            }
        }
        result
    }

    async fn exchange_inner(&self, peer: &Peer) -> Result<ExchangeReport> {
        self.0.validate_peer(peer).await?;
        let mut config = self.0.identity.client(&peer.card)?;
        let courier = self.0.approved.as_ref().is_some_and(|c| c.own.courier)
            && peer.approval.as_ref().is_some_and(|c| c.courier);
        let alpn = if courier { COURIER_ALPN } else { ALPN };
        config.alpn_protocols = vec![alpn.to_vec()];
        let connector = TlsConnector::from(Arc::new(config));
        for address in peer.card.addresses.iter().take(4) {
            let connect = async {
                let socket = self.0.connect_socket(*address).await?;
                connector
                    .connect(NAME.try_into().unwrap(), socket)
                    .await
                    .map_err(|_| "기록 교환용 장치 인증에 실패했어요.".to_string())
            };
            let mut stream = match tokio::time::timeout(IO_TIMEOUT, connect).await {
                Ok(Ok(s)) => s,
                _ => continue,
            };
            if stream.get_ref().1.alpn_protocol() != Some(alpn)
                || stream
                    .get_ref()
                    .1
                    .peer_certificates()
                    .and_then(|c| c.first())
                    .map(|c| c.as_ref())
                    != Some(peer.card.der()?.as_slice())
            {
                return err("등록한 상대의 기록 교환 연결이 아니에요.");
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
            if reply.session != peer.card.session || reply.nonce != nonce {
                return err("기록 교환의 인증 응답이 달라요.");
            }
            let (sent, sent_headers, a) = push(&mut stream, &self.0, peer, &nonce).await?;
            let (received, received_headers, b) = pull(&mut stream, &self.0, peer, &nonce).await?;
            let (relay_sent, relay_received, c) = if courier {
                super::courier::sync(&mut stream, &self.0, peer, &nonce, true).await?
            } else {
                (0, 0, false)
            };
            self.0.check_active()?;
            return Ok(ExchangeReport {
                sent,
                received,
                sent_headers,
                received_headers,
                more: a || b || c,
                relay_sent,
                relay_received,
            });
        }
        err("기록 교환 연결을 열지 못했어요. 양쪽의 동의·최신 연결 정보·통신 허용 상태를 확인해 주세요.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn oversized_transfer_frame_is_rejected_before_reading_its_payload() {
        let (mut a, mut b) = tokio::io::duplex(16);
        a.write_u32((MAX_FRAME + 1) as u32).await.unwrap();
        assert!(read(&mut b).await.is_err());
    }
}
