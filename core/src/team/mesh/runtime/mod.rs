//! 앱 수명에 묶인 PC 연결. 자동 복구는 서명된 별도 동의와 같은 네트워크에서만 허용한다.
mod network;
pub use super::identity::consent::{Mode, NetworkScope};
use super::{
    identity::{automatic::AutoScope, Vault},
    probe::{Checker, Probe, Status},
};
use crate::{err, Result};
pub use network::SystemEnvironment;
use serde::Serialize;
use std::{
    net::Ipv4Addr,
    sync::{Arc, Mutex},
    time::Duration,
};

const BUDGET: u64 = 15 * 60 * 1000;
pub trait Environment: Send + Sync {
    fn now(&self) -> u64;
    fn inspect(&self, address: Ipv4Addr) -> Result<NetworkScope>;
    fn locate(&self, scope: &NetworkScope) -> Result<Ipv4Addr>;
}
#[derive(Clone)]
pub struct Config {
    pub team: String,
    pub address: Ipv4Addr,
    pub mode: Mode,
    pub restore: bool,
}
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleStatus {
    pub phase: &'static str,
    pub restore: bool,
    pub startup_enabled: bool,
    pub team_id: Option<String>,
    pub remaining_seconds: u64,
    pub notice: Option<String>,
    pub recoveries: u64,
}
#[derive(Serialize)]
pub struct RuntimeStatus {
    #[serde(flatten)]
    pub session: Status,
    pub lifecycle: LifecycleStatus,
}

struct Plan {
    config: Config,
    network: NetworkScope,
    approval: AutoScope,
    deadline: u64,
}
#[derive(Default)]
struct State {
    probe: Option<Probe>,
    plan: Option<Plan>,
    ready: bool,
    remembered: bool,
    phase: &'static str,
    notice: Option<String>,
    recoveries: u64,
    last_tick: u64,
    next_try: u64,
}
#[derive(Default)]
struct Control {
    generation: u64,
    suspended: bool,
    closing: bool,
    reset: bool,
    checker: Option<Checker>,
    session: Option<String>,
    starting: Option<u64>,
    stopping: bool,
}
struct Inner {
    vault: Vault,
    env: Arc<dyn Environment>,
    state: tokio::sync::Mutex<State>,
    control: Mutex<Control>,
    wake: tokio::sync::Notify,
}
#[derive(Clone)]
pub struct Controller(Arc<Inner>);
#[derive(Clone, Copy)]
pub enum Event {
    Suspend,
    Resume,
    Exit,
}
pub struct Worker {
    task: tokio::task::JoinHandle<()>,
    controller: Controller,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.controller.event(Event::Exit);
        self.task.abort();
    }
}
struct Ticket(Arc<Inner>, u64, bool);
impl Drop for Ticket {
    fn drop(&mut self) {
        let mut c = self.0.control.lock().unwrap();
        if self.2 {
            c.stopping = false;
        } else if c.starting == Some(self.1) {
            c.starting = None;
        }
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| "PC 연결 설정을 처리하지 못했어요.")?
}
impl Controller {
    pub fn new(vault: Vault) -> Self {
        Self::with_environment(vault, Arc::new(SystemEnvironment))
    }
    pub fn with_environment(vault: Vault, env: Arc<dyn Environment>) -> Self {
        Self(Arc::new(Inner {
            vault,
            env,
            state: Default::default(),
            control: Default::default(),
            wake: Default::default(),
        }))
    }
    pub fn event(&self, event: Event) {
        let mut c = self.0.control.lock().unwrap();
        match event {
            Event::Suspend if !c.closing => c.suspended = true,
            Event::Resume if c.suspended && !c.closing => c.suspended = false,
            Event::Exit => c.closing = true,
            _ => return,
        }
        c.generation = c.generation.wrapping_add(1);
        c.reset = true;
        c.session = None;
        if let Some(checker) = c.checker.take() {
            checker.cancel();
        }
        self.0.wake.notify_one();
    }
    pub fn worker(&self) -> Worker {
        let weak = Arc::downgrade(&self.0);
        let task = tokio::spawn(async move {
            loop {
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                let controller = Controller(inner);
                let closing = controller.0.control.lock().unwrap().closing;
                if closing {
                    controller.clean().await;
                    break;
                }
                controller.tick().await;
                tokio::select! { _=tokio::time::sleep(Duration::from_secs(1))=>{},_=controller.0.wake.notified()=>{} }
            }
        });
        Worker {
            task,
            controller: self.clone(),
        }
    }
    fn snapshot(&self) -> Result<u64> {
        let c = self.0.control.lock().unwrap();
        if c.closing || c.suspended || c.stopping {
            return err("앱이 종료·절전·중지 중이라 연결을 열지 않았어요.");
        }
        Ok(c.generation)
    }
    fn publish(&self, generation: u64, probe: &Probe, manual: bool, deadline: u64) -> Result<()> {
        let mut c = self.0.control.lock().unwrap();
        if c.closing
            || c.suspended
            || c.stopping
            || c.generation != generation
            || self.0.env.now() >= deadline
            || (manual && c.starting != Some(generation))
            || (!manual && c.starting.is_some())
        {
            probe.checker().cancel();
            return err("연결 요청 중 앱 상태가 바뀌어 시작을 취소했어요.");
        }
        c.session = probe.status().session;
        c.checker = Some(probe.checker());
        Ok(())
    }
    async fn close_probe(&self, state: &mut State) {
        {
            let mut c = self.0.control.lock().unwrap();
            c.session = None;
            if let Some(checker) = c.checker.take() {
                checker.cancel();
            }
        }
        if let Some(probe) = state.probe.take() {
            probe.shutdown().await;
        }
    }
    async fn open(&self, plan: &Plan, address: Ipv4Addr, remaining: u64) -> Result<Probe> {
        let v = self.0.vault.clone();
        let scope = plan.approval.clone();
        blocking(move || v.validate_auto_scope(&scope)).await?;
        let probe = Probe::start_managed(
            self.0.vault.clone(),
            &plan.config.team,
            address,
            plan.config.mode,
            Duration::from_millis(remaining),
        )
        .await?;
        if !probe.matches_approval(&plan.approval) {
            probe.checker().cancel();
            return err("연결을 여는 동안 승인 이력이 바뀌었어요. 다시 동의해 주세요.");
        }
        Ok(probe)
    }
    pub async fn start(&self, config: Config) -> Result<RuntimeStatus> {
        self.snapshot()?;
        let generation = {
            let mut c = self.0.control.lock().unwrap();
            if c.closing || c.suspended || c.stopping {
                return err("앱이 종료·절전·중지 중이라 연결을 열지 않았어요.");
            }
            if c.starting.is_some() {
                return err("앞의 시작 요청을 처리 중이에요.");
            }
            c.generation = c.generation.wrapping_add(1);
            // 복귀 뒤 새로 동의한 시작은 이전 실행의 지연된 정리 신호를 이어받지 않는다.
            c.reset = false;
            let generation = c.generation;
            c.starting = Some(generation);
            generation
        };
        let _ticket = Ticket(self.0.clone(), generation, false);
        let mut state = self.0.state.lock().await;
        state.ready = true;
        if state.probe.as_ref().is_some_and(|p| p.status().running) {
            return err("PC 연결이 이미 실행 중이에요.");
        }
        self.close_probe(&mut state).await;
        state.plan = None;
        let (env, address) = (self.0.env.clone(), config.address);
        let network = blocking(move || env.inspect(address)).await?;
        if config.restore && (!config.mode.automatic() || !network.trusted) {
            return err("자동 복구는 개인·도메인 네트워크의 자동 연결에서만 켤 수 있어요.");
        }
        let (v, team) = (self.0.vault.clone(), config.team.clone());
        let approval = blocking(move || v.runtime_scope(&team)).await?;
        let deadline = self.0.env.now().saturating_add(BUDGET);
        let plan = Plan {
            config: config.clone(),
            network,
            approval,
            deadline,
        };
        if self.snapshot()? != generation {
            return err("시작 요청이 취소됐어요.");
        }
        let probe = self.open(&plan, address, BUDGET).await?;
        // 성공적으로 열린 실행만 동의로 보관한다. 실패한 저장은 성공으로 표시하지 않는다.
        let (v, p, approval, mode) = (
            self.0.vault.clone(),
            plan.network.clone(),
            plan.approval.clone(),
            config.mode,
        );
        let saved = blocking(move || {
            if config.restore {
                v.remember_runtime(&approval, mode, p).map(|_| ())
            } else {
                v.forget_runtime()
            }
        })
        .await;
        if let Err(error) = saved {
            probe.checker().cancel();
            drop(probe);
            return Err(error);
        }
        state.remembered = config.restore;
        self.publish(generation, &probe, true, deadline)?;
        state.probe = Some(probe);
        state.plan = Some(plan);
        state.phase = "running";
        state.notice = None;
        state.next_try = 0;
        state.last_tick = self.0.env.now();
        Ok(self.status_locked(&state))
    }
    pub async fn stop(&self, session: Option<&str>) -> Result<RuntimeStatus> {
        // 늦은 화면의 중지 요청이 새 실행을 끄지 않도록 현재 실행을 대조한다.
        let generation = {
            let mut c = self.0.control.lock().unwrap();
            if c.stopping {
                return err("연결 중지를 처리하고 있어요.");
            }
            if c.session.is_some() && c.session.as_deref() != session {
                return err("연결이 바뀌었어요. 현재 연결 정보를 확인해 주세요.");
            }
            c.generation = c.generation.wrapping_add(1);
            c.reset = false;
            c.stopping = true;
            c.starting = None;
            c.session = None;
            if let Some(checker) = c.checker.take() {
                checker.cancel();
            }
            c.generation
        };
        let _ticket = Ticket(self.0.clone(), generation, true);
        let mut state = self.0.state.lock().await;
        state.plan = None;
        state.phase = "stopped";
        state.notice = None;
        state.ready = true;
        self.close_probe(&mut state).await;
        let v = self.0.vault.clone();
        if let Err(error) = blocking(move || v.forget_runtime()).await {
            state.notice = Some(format!(
                "연결은 중지했지만 자동 복구 동의 해제 저장에 실패했어요: {error}"
            ));
            return Err(state.notice.clone().unwrap());
        }
        state.remembered = false;
        Ok(self.status_locked(&state))
    }
    pub async fn checker(&self, session: &str) -> Result<Checker> {
        let state = self.0.state.lock().await;
        let p = state
            .probe
            .as_ref()
            .filter(|p| p.status().running && p.status().session.as_deref() == Some(session))
            .ok_or("현재 실행의 연결 정보를 확인해 주세요.")?;
        Ok(p.checker())
    }
    pub async fn wake(&self) {
        if let Some(p) = self.0.state.lock().await.probe.as_ref() {
            p.checker().wake();
        }
    }
    pub async fn status(&self) -> RuntimeStatus {
        let state = self.0.state.lock().await;
        self.status_locked(&state)
    }
    fn status_locked(&self, state: &State) -> RuntimeStatus {
        let session = state.probe.as_ref().map(Probe::status).unwrap_or_default();
        RuntimeStatus {
            lifecycle: LifecycleStatus {
                phase: state.phase,
                restore: state.plan.as_ref().is_some_and(|p| p.config.restore),
                startup_enabled: state.remembered,
                team_id: state.plan.as_ref().map(|p| p.config.team.clone()),
                remaining_seconds: state
                    .plan
                    .as_ref()
                    .map(|p| p.deadline.saturating_sub(self.0.env.now()) / 1000)
                    .unwrap_or(0),
                notice: state.notice.clone(),
                recoveries: state.recoveries,
            },
            session,
        }
    }
    async fn clean(&self) {
        let mut state = self.0.state.lock().await;
        self.close_probe(&mut state).await;
        state.plan = None;
        state.phase = "exited";
    }
    pub async fn tick(&self) {
        let mut state = self.0.state.lock().await;
        let now = self.0.env.now();
        if !state.ready {
            state.ready = true;
            let v = self.0.vault.clone();
            match blocking(move || v.runtime_profile()).await {
                Ok(Some(profile)) => {
                    state.remembered = true;
                    state.plan = Some(Plan {
                        config: Config {
                            team: profile.team().into(),
                            address: Ipv4Addr::UNSPECIFIED,
                            mode: profile.mode(),
                            restore: true,
                        },
                        network: profile.network().clone(),
                        approval: profile.approval(),
                        deadline: now.saturating_add(BUDGET),
                    });
                }
                Ok(None) => state.phase = "off",
                Err(error) => {
                    state.phase = "needs-consent";
                    state.notice = Some(error);
                }
            }
        }
        let (closing, suspended, reset, generation) = {
            let mut c = self.0.control.lock().unwrap();
            let reset = c.reset;
            c.reset = false;
            (c.closing, c.suspended, reset, c.generation)
        };
        let gap = state.last_tick != 0 && now.saturating_sub(state.last_tick) > 10_000;
        state.last_tick = now;
        if closing {
            self.close_probe(&mut state).await;
            state.plan = None;
            state.phase = "exited";
            return;
        }
        if reset || gap || suspended {
            self.close_probe(&mut state).await;
            if state.plan.as_ref().is_some_and(|p| !p.config.restore) {
                state.plan = None;
                state.phase = "needs-consent";
                state.notice = Some(
                    "절전 또는 실행 중단으로 연결을 닫았어요. 다시 동의하고 시작해 주세요.".into(),
                );
            }
        }
        if suspended {
            state.phase = "suspended";
            return;
        }
        if state.phase == "suspended" && state.plan.is_none() {
            state.phase = "needs-consent";
        }
        if state.plan.as_ref().is_some_and(|p| p.deadline <= now) {
            self.close_probe(&mut state).await;
            state.plan = None;
            state.phase = "expired";
            state.notice = Some("이번 실행의 15분이 끝났어요. 다시 동의하고 시작해 주세요.".into());
            return;
        }
        if state.probe.as_ref().is_some_and(|p| !p.status().running) {
            let status = state.probe.as_ref().unwrap().status();
            self.close_probe(&mut state).await;
            state.plan = None;
            state.phase = "needs-consent";
            state.notice = status.notice;
            return;
        }
        let Some(plan) = state.plan.as_ref() else {
            return;
        };
        if now < state.next_try {
            return;
        }
        let (env, network) = (self.0.env.clone(), plan.network.clone());
        let address = blocking(move || env.locate(&network)).await;
        match address {
            Err(error) => {
                self.close_probe(&mut state).await;
                if !state.plan.as_ref().unwrap().config.restore {
                    state.plan = None;
                }
                state.phase = "waiting-network";
                state.notice = Some(error);
                state.next_try = now.saturating_add(5000);
            }
            Ok(address) => {
                if state.probe.is_some() && state.plan.as_ref().unwrap().config.address == address {
                    return;
                }
                self.close_probe(&mut state).await;
                if !state.plan.as_ref().unwrap().config.restore {
                    state.plan = None;
                    state.phase = "needs-consent";
                    return;
                }
                let remaining = state
                    .plan
                    .as_ref()
                    .unwrap()
                    .deadline
                    .saturating_sub(self.0.env.now());
                if remaining == 0 {
                    return;
                }
                if self.snapshot().ok() != Some(generation) {
                    return;
                }
                match self
                    .open(state.plan.as_ref().unwrap(), address, remaining)
                    .await
                {
                    Ok(probe) => {
                        if let Err(error) = self.publish(
                            generation,
                            &probe,
                            false,
                            state.plan.as_ref().unwrap().deadline,
                        ) {
                            state.notice = Some(error);
                            return;
                        }
                        state.probe = Some(probe);
                        state.plan.as_mut().unwrap().config.address = address;
                        state.phase = "running";
                        state.notice = None;
                        state.recoveries = state.recoveries.saturating_add(1);
                        state.next_try = 0;
                    }
                    Err(error) => {
                        state.plan = None;
                        state.phase = "needs-consent";
                        state.notice = Some(error);
                    }
                }
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests;
