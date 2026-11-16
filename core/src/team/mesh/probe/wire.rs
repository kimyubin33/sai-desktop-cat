use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::card::{valid_session, VERSION};
use crate::{err, Result};

pub(super) const MAX_FRAME: usize = 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Signal {
    pub protocol: u8,
    pub session: String,
    pub nonce: String,
}

impl Signal {
    pub fn valid(&self) -> bool {
        self.protocol == VERSION && valid_session(&self.session) && valid_session(&self.nonce)
    }
}

pub(super) async fn read(stream: &mut (impl AsyncRead + Unpin)) -> Result<Signal> {
    let size = stream
        .read_u32()
        .await
        .map_err(|_| "연결 확인 응답이 끊겼어요.")? as usize;
    if size == 0 || size > MAX_FRAME {
        return err("연결 확인 메시지 크기가 올바르지 않아요.");
    }
    let mut bytes = vec![0; size];
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(|_| "연결 확인 응답이 끊겼어요.")?;
    let signal: Signal =
        serde_json::from_slice(&bytes).map_err(|_| "연결 확인 메시지 형식이 아니에요.")?;
    if !signal.valid() {
        return err("호환되는 연결 확인 메시지가 아니에요.");
    }
    Ok(signal)
}

pub(super) async fn write(stream: &mut (impl AsyncWrite + Unpin), signal: &Signal) -> Result<()> {
    let bytes = serde_json::to_vec(signal).map_err(|_| "연결 확인 메시지를 만들지 못했어요.")?;
    if bytes.len() > MAX_FRAME {
        return err("연결 확인 메시지가 너무 커요.");
    }
    stream
        .write_u32(bytes.len() as u32)
        .await
        .map_err(|_| "확인 신호를 보내지 못했어요.")?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|_| "확인 신호를 보내지 못했어요.")?;
    stream
        .flush()
        .await
        .map_err(|_| "확인 신호를 보내지 못했어요.")?;
    Ok(())
}
