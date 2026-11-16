use super::{Environment, NetworkScope};
use crate::{err, Result};
use std::net::Ipv4Addr;

pub struct SystemEnvironment;
impl Environment for SystemEnvironment {
    fn now(&self) -> u64 {
        #[cfg(windows)]
        {
            unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
        }
        #[cfg(not(windows))]
        {
            0
        }
    }
    fn inspect(&self, address: Ipv4Addr) -> Result<NetworkScope> {
        #[cfg(windows)]
        {
            inspect(address)
        }
        #[cfg(not(windows))]
        {
            let _ = address;
            err("자동 복구는 현재 Windows에서만 지원해요.")
        }
    }
    fn locate(&self, scope: &NetworkScope) -> Result<Ipv4Addr> {
        let mut matches = Vec::new();
        for interface in if_addrs::get_if_addrs().map_err(|_| "네트워크를 확인하지 못했어요.")?
        {
            if interface.oper_status != if_addrs::IfOperStatus::Up {
                continue;
            }
            let if_addrs::IfAddr::V4(v4) = interface.addr else {
                continue;
            };
            if !scope.contains(v4.ip) {
                continue;
            }
            if self.inspect(v4.ip).is_ok_and(|observed| &observed == scope) {
                matches.push(v4.ip);
            }
        }
        matches.sort();
        matches.dedup();
        if matches.len() != 1 {
            return err("동의한 Windows 네트워크의 단일 LAN 주소를 기다리고 있어요.");
        }
        Ok(matches[0])
    }
}

#[cfg(windows)]
fn inspect(address: Ipv4Addr) -> Result<NetworkScope> {
    use windows::{
        core::GUID,
        Win32::{Networking::NetworkListManager::*, System::Com::*},
    };
    let interfaces =
        if_addrs::get_if_addrs().map_err(|_| "네트워크 어댑터를 확인하지 못했어요.")?;
    let candidates: Vec<_> = interfaces
        .into_iter()
        .filter(|i| {
            i.oper_status == if_addrs::IfOperStatus::Up && i.ip() == std::net::IpAddr::V4(address)
        })
        .collect();
    if candidates.len() != 1 || !address.is_private() {
        return err("이 PC의 단일 사설 LAN 주소를 선택해 주세요.");
    }
    let interface = &candidates[0];
    let if_addrs::IfAddr::V4(v4) = &interface.addr else {
        return err("LAN IPv4 주소가 아니에요.");
    };
    let adapter = GUID::try_from(interface.adapter_name.trim_matches(['{', '}']))
        .map_err(|_| "어댑터 식별자를 확인하지 못했어요.")?;
    struct Com;
    impl Drop for Com {
        fn drop(&mut self) {
            unsafe {
                CoUninitialize();
            }
        }
    }
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|_| "Windows 네트워크 확인을 시작하지 못했어요.")?;
        let _com = Com;
        let manager: INetworkListManager = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)
            .map_err(|_| "Windows 네트워크 목록을 열지 못했어요.")?;
        let connections = manager
            .GetNetworkConnections()
            .map_err(|_| "Windows 네트워크를 열거하지 못했어요.")?;
        let mut found = Vec::new();
        let mut finished = false;
        for _ in 0..128 {
            let mut rows = [None];
            let mut count = 0;
            connections
                .Next(&mut rows, Some(&mut count))
                .map_err(|_| "Windows 네트워크를 확인하지 못했어요.")?;
            if count == 0 {
                finished = true;
                break;
            }
            let connection = rows[0].take().ok_or("Windows 네트워크 정보가 없어요.")?;
            if connection
                .GetAdapterId()
                .map_err(|_| "어댑터 ID를 확인하지 못했어요.")?
                != adapter
                || !connection
                    .IsConnected()
                    .map_err(|_| "연결 상태를 확인하지 못했어요.")?
                    .as_bool()
            {
                continue;
            }
            let network = connection
                .GetNetwork()
                .map_err(|_| "네트워크 프로필을 확인하지 못했어요.")?;
            let category = network
                .GetCategory()
                .map_err(|_| "네트워크 종류를 확인하지 못했어요.")?;
            let id = network
                .GetNetworkId()
                .map_err(|_| "네트워크 ID를 확인하지 못했어요.")?;
            found.push(NetworkScope {
                adapter: format!("{:032x}", u128::from(adapter)),
                network: format!("{:032x}", u128::from(id)),
                subnet: Ipv4Addr::from(u32::from(address) & u32::from(v4.netmask)),
                mask: v4.netmask,
                trusted: category == NLM_NETWORK_CATEGORY_PRIVATE
                    || category == NLM_NETWORK_CATEGORY_DOMAIN_AUTHENTICATED,
            });
        }
        found.sort_by(|a, b| a.network.cmp(&b.network));
        found.dedup();
        if !finished || found.len() != 1 {
            return err("선택한 어댑터의 Windows 네트워크가 모호해요. 자동 복구를 보류했어요.");
        }
        let scope = found.remove(0);
        scope.validate()?;
        Ok(scope)
    }
}
