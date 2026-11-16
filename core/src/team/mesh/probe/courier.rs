//! 중계 동의를 서명한 두 피어만 v2 TLS 연결에서 제한된 보관 목록을 교환한다.
use super::*;
use crate::team::mesh::identity::{
    courier::{Parcel, BATCH, MAX_ITEMS},
    transfer::{Scope, MAX_FRAME},
    Vault,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    deny_unknown_fields,
    rename_all = "camelCase"
)]
enum Frame {
    Inventory(Vec<String>),
    Parcel(Parcel),
    Stored(String),
    End { more: bool },
}
async fn read(stream: &mut (impl AsyncRead + Unpin)) -> Result<Frame> {
    let n = stream
        .read_u32()
        .await
        .map_err(|_| "중계 연결이 닫혔어요.")? as usize;
    if n == 0 || n > MAX_FRAME {
        return err("중계 프레임 한도를 넘었어요.");
    }
    let mut bytes = vec![0; n];
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(|_| "중계 자료 수신이 끊겼어요.")?;
    serde_json::from_slice(&bytes).map_err(|_| "호환되는 중계 자료가 아니에요.".into())
}
async fn write(stream: &mut (impl AsyncWrite + Unpin), frame: &Frame) -> Result<()> {
    let bytes = serde_json::to_vec(frame).map_err(|_| "중계 프레임을 만들지 못했어요.")?;
    if bytes.len() > MAX_FRAME {
        return err("중계 프레임 한도를 넘었어요.");
    }
    stream
        .write_u32(bytes.len() as u32)
        .await
        .map_err(|_| "중계 연결이 끊겼어요.")?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|_| "중계 자료를 보내지 못했어요.")?;
    stream
        .flush()
        .await
        .map_err(|_| "중계 자료를 보내지 못했어요.".into())
}
async fn push(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    inner: &Inner,
    peer: &Peer,
    vault: &Vault,
    scope: &Scope,
    preparing_more: bool,
) -> Result<(usize, bool)> {
    let inventory = match read(stream).await? {
        Frame::Inventory(ids) if ids.len() <= MAX_ITEMS => ids,
        _ => return err("먼저 제한된 중계 보관 목록이 필요해요."),
    };
    let (v, s) = (vault.clone(), scope.clone());
    let (items, more) = super::exchange::blocking(move || v.courier_batch(&s, &inventory)).await?;
    let more = more || preparing_more;
    let count = items.len();
    for item in items {
        inner.check_active()?;
        inner.validate_peer(peer).await?;
        let key = item.key()?;
        write(stream, &Frame::Parcel(item)).await?;
        match read(stream).await? {
            Frame::Stored(id) if id == key => (),
            _ => return err("중계 보관 응답이 현재 자료와 달라요."),
        }
    }
    write(stream, &Frame::End { more }).await?;
    Ok((count, more))
}
async fn pull(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    inner: &Inner,
    peer: &Peer,
    vault: &Vault,
    scope: &Scope,
    challenge: &str,
) -> Result<(usize, bool)> {
    let (v, s) = (vault.clone(), scope.clone());
    let ids = super::exchange::blocking(move || v.courier_inventory(&s)).await?;
    write(stream, &Frame::Inventory(ids)).await?;
    for count in 0..=BATCH {
        match read(stream).await? {
            Frame::Parcel(item) if count < BATCH => {
                inner.check_active()?;
                inner.validate_peer(peer).await?;
                let key = item.key()?;
                let (v, s, n) = (vault.clone(), scope.clone(), challenge.to_string());
                super::exchange::blocking(move || v.store_courier(&s, &item, &n)).await?;
                inner.check_active()?;
                // 연결 내부의 보관 응답이며 최종 수신자의 서명된 Receipt와 다르다.
                write(stream, &Frame::Stored(key)).await?;
            }
            Frame::End { more } => return Ok((count, more)),
            _ => return err("중계 자료의 순서 또는 개수 한도를 넘었어요."),
        }
    }
    err("중계 자료 개수 한도를 넘었어요.")
}
pub(super) async fn sync(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    inner: &Inner,
    peer: &Peer,
    challenge: &str,
    initiator: bool,
) -> Result<(usize, usize, bool)> {
    if !(inner.approved.as_ref().is_some_and(|c| c.own.courier)
        && peer.approval.as_ref().is_some_and(|c| c.courier))
    {
        return err("양쪽의 중계 동의가 필요해요.");
    }
    inner.check_active()?;
    inner.validate_peer(peer).await?;
    let (vault, scope) = super::exchange::scope(inner, peer)?;
    let (v, s, n) = (vault.clone(), scope.clone(), challenge.to_string());
    let preparing_more = super::exchange::blocking(move || v.prepare_courier(&s, &n)).await?;
    if initiator {
        let (sent, a) = push(stream, inner, peer, &vault, &scope, preparing_more).await?;
        let (received, b) = pull(stream, inner, peer, &vault, &scope, challenge).await?;
        Ok((sent, received, a || b || preparing_more))
    } else {
        let (received, b) = pull(stream, inner, peer, &vault, &scope, challenge).await?;
        let (sent, a) = push(stream, inner, peer, &vault, &scope, preparing_more).await?;
        Ok((sent, received, a || b || preparing_more))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn oversized_courier_frame_is_rejected_before_allocating_payload() {
        let (mut a, mut b) = tokio::io::duplex(8);
        a.write_u32((MAX_FRAME + 1) as u32).await.unwrap();
        assert!(read(&mut b).await.is_err());
    }
}

#[cfg(all(test, windows))]
#[path = "courier_tests.rs"]
mod network_tests;
