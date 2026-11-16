//! v1은 고정 필드 순서의 구조체 JSON과 용도별 접두어에 서명한다. 임의 JSON에 서명하지 않는다.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use x25519_dalek::{PublicKey, StaticSecret};

use super::keys::{digest, random_id, signed_bytes, unhex, verify, Material};
use crate::{err, Result};

pub const MAX_PACKET: usize = 1024 * 1024;
pub const MAX_EVENTS: usize = 256;
const MANIFEST: &str = "sai-mesh/manifest/1";
const REQUEST: &str = "sai-mesh/enrollment/1";
const EVENT: &str = "sai-mesh/membership/1";

fn parse<T: serde::de::DeserializeOwned>(text: &str, limit: usize) -> Result<T> {
    if text.len() > limit {
        return err("등록 자료가 너무 커요.");
    }
    serde_json::from_str(text).map_err(|_| "호환되는 장치 등록 자료가 아니에요.".into())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevicePublic {
    pub version: u8,
    pub device_id: String,
    pub signing_key: String,
    pub encryption_key: String,
}

impl DevicePublic {
    pub fn validate(&self) -> Result<()> {
        unhex::<32>(&self.signing_key)?;
        let encryption = PublicKey::from(unhex::<32>(&self.encryption_key)?);
        if self.version != 1
            || !StaticSecret::from([0x5a; 32])
                .diffie_hellman(&encryption)
                .was_contributory()
        {
            return err("호환되는 장치 공개키가 아니에요.");
        }
        let expected = digest(
            "sai-mesh/device-id/1",
            format!("{}:{}", self.signing_key, self.encryption_key).as_bytes(),
        );
        if self.device_id != expected {
            return err("장치 ID와 공개키가 일치하지 않아요.");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestBody {
    pub version: u8,
    pub team_id: String,
    pub nonce: String,
    pub name: String,
    pub authority_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamManifest {
    pub body: ManifestBody,
    pub signature: String,
}

impl TeamManifest {
    pub(super) fn create(authority: &Material, name: &str) -> Result<Self> {
        let name = name.trim().to_string();
        if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
            return err("PC 팀 이름을 1~60자로 입력해 주세요.");
        }
        let nonce = random_id()?;
        let authority_key = authority.signing_public()?;
        let team_id = digest(
            "sai-mesh/team-id/1",
            format!("{authority_key}:{nonce}").as_bytes(),
        );
        let body = ManifestBody {
            version: 1,
            team_id,
            nonce,
            name,
            authority_key,
        };
        Ok(Self {
            signature: authority.sign(MANIFEST, &body)?,
            body,
        })
    }

    pub fn fingerprint(&self) -> Result<String> {
        Ok(digest(
            "sai-mesh/authority/1",
            &unhex::<32>(&self.body.authority_key)?,
        ))
    }

    pub fn validate(&self) -> Result<()> {
        unhex::<16>(&self.body.nonce)?;
        unhex::<32>(&self.body.authority_key)?;
        let expected = digest(
            "sai-mesh/team-id/1",
            format!("{}:{}", self.body.authority_key, self.body.nonce).as_bytes(),
        );
        if self.body.version != 1
            || self.body.team_id != expected
            || self.body.name.trim().is_empty()
            || self.body.name.chars().count() > 60
            || self.body.name.chars().any(char::is_control)
        {
            return err("PC 팀의 신뢰 기준을 확인하지 못했어요.");
        }
        verify(
            &self.body.authority_key,
            MANIFEST,
            &self.body,
            &self.signature,
        )
    }

    fn head(&self) -> Result<String> {
        Ok(digest(
            "sai-mesh/manifest-hash/1",
            &signed_bytes(MANIFEST, self)?,
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestBody {
    pub version: u8,
    pub team_id: String,
    pub authority_fingerprint: String,
    pub nonce: String,
    pub device: DevicePublic,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentRequest {
    pub body: RequestBody,
    pub signature: String,
}

impl EnrollmentRequest {
    pub(super) fn create(device: &Material, manifest: &TeamManifest) -> Result<Self> {
        manifest.validate()?;
        let body = RequestBody {
            version: 1,
            team_id: manifest.body.team_id.clone(),
            authority_fingerprint: manifest.fingerprint()?,
            nonce: random_id()?,
            device: device.device()?,
        };
        Ok(Self {
            signature: device.sign(REQUEST, &body)?,
            body,
        })
    }

    pub fn parse(text: &str) -> Result<Self> {
        parse(text, 8192)
    }

    pub fn validate(&self, manifest: &TeamManifest) -> Result<()> {
        manifest.validate()?;
        self.body.device.validate()?;
        unhex::<16>(&self.body.nonce)?;
        if self.body.version != 1
            || self.body.team_id != manifest.body.team_id
            || self.body.authority_fingerprint != manifest.fingerprint()?
        {
            return err("이 PC 팀에 보낸 등록 요청이 아니에요.");
        }
        verify(
            &self.body.device.signing_key,
            REQUEST,
            &self.body,
            &self.signature,
        )
    }

    fn id(&self) -> Result<String> {
        Ok(digest(
            "sai-mesh/request-hash/1",
            &signed_bytes(REQUEST, self)?,
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields, rename_all = "camelCase")]
pub enum MembershipAction {
    Approve {
        request: EnrollmentRequest,
        member_id: String,
    },
    Revoke {
        device_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventBody {
    pub version: u8,
    pub team_id: String,
    pub revision: u64,
    pub parent: String,
    pub action: MembershipAction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MembershipEvent {
    pub body: EventBody,
    pub signature: String,
}

impl MembershipEvent {
    pub(super) fn create(
        authority: &Material,
        manifest: &TeamManifest,
        state: &MembershipState,
        action: MembershipAction,
    ) -> Result<Self> {
        if authority.is_device() || authority.signing_public()? != manifest.body.authority_key {
            return err("이 팀의 운영 관리자 키가 아니에요.");
        }
        let body = EventBody {
            version: 1,
            team_id: manifest.body.team_id.clone(),
            revision: state.revision + 1,
            parent: state.head.clone(),
            action,
        };
        Ok(Self {
            signature: authority.sign(EVENT, &body)?,
            body,
        })
    }

    pub fn hash(&self) -> Result<String> {
        Ok(digest("sai-mesh/event-hash/1", &signed_bytes(EVENT, self)?))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalPacket {
    pub version: u8,
    pub manifest: TeamManifest,
    pub events: Vec<MembershipEvent>,
}

#[derive(Clone, Serialize)]
pub struct MemberDevice {
    pub device: DevicePublic,
    pub member_id: String,
    pub active: bool,
}

pub struct MembershipState {
    pub revision: u64,
    pub head: String,
    pub devices: BTreeMap<String, MemberDevice>,
    used_requests: BTreeSet<String>,
}

impl ApprovalPacket {
    pub fn parse(text: &str) -> Result<Self> {
        parse(text, MAX_PACKET)
    }

    pub fn state(&self) -> Result<MembershipState> {
        self.manifest.validate()?;
        if self.version != 1 || self.events.len() > MAX_EVENTS {
            return err("지원 범위를 넘는 승인 이력이에요.");
        }
        let mut state = MembershipState {
            revision: 0,
            head: self.manifest.head()?,
            devices: BTreeMap::new(),
            used_requests: BTreeSet::new(),
        };
        for event in &self.events {
            if event.body.version != 1
                || event.body.team_id != self.manifest.body.team_id
                || event.body.revision != state.revision + 1
                || event.body.parent != state.head
            {
                return err("승인 이력의 순서 또는 이전 기록이 맞지 않아요.");
            }
            verify(
                &self.manifest.body.authority_key,
                EVENT,
                &event.body,
                &event.signature,
            )?;
            match &event.body.action {
                MembershipAction::Approve { request, member_id } => {
                    request.validate(&self.manifest)?;
                    unhex::<16>(member_id)?;
                    let id = &request.body.device.device_id;
                    if state.devices.get(id).is_some_and(|d| d.active)
                        || !state.used_requests.insert(request.id()?)
                    {
                        return err("이미 사용한 장치 등록 요청이에요.");
                    }
                    state.devices.insert(
                        id.clone(),
                        MemberDevice {
                            device: request.body.device.clone(),
                            member_id: member_id.clone(),
                            active: true,
                        },
                    );
                }
                MembershipAction::Revoke { device_id } => {
                    let device = state
                        .devices
                        .get_mut(device_id)
                        .filter(|d| d.active)
                        .ok_or("승인된 장치의 해제 기록이 아니에요.")?;
                    device.active = false;
                }
            }
            state.revision += 1;
            state.head = event.hash()?;
        }
        Ok(state)
    }

    pub(super) fn append(&mut self, authority: &Material, action: MembershipAction) -> Result<()> {
        if self.events.len() >= MAX_EVENTS {
            return err("실험용 승인 이력 한도에 도달했어요.");
        }
        let event = MembershipEvent::create(authority, &self.manifest, &self.state()?, action)?;
        self.events.push(event);
        if let Err(error) = self.state() {
            self.events.pop();
            return Err(error);
        }
        Ok(())
    }
}
