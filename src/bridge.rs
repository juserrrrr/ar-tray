//! Command queue between the panel and the unit. Quick clicks merge into one command.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::ac::{Changes, Device, Failure, Outcome, Unit};
use crate::log;
use crate::smartthings::LoginControl;

/// A click goes out at once. Quick clicks still merge: the ones that land while a command is on its
/// way wait in the queue and leave together when it returns.
const FLUSH_DELAY: Duration = Duration::ZERO;
const POLL_VISIBLE: Duration = Duration::from_secs(15);
const POLL_HIDDEN: Duration = Duration::from_secs(60);
const STALE_AFTER: Duration = Duration::from_secs(5);
const CLIMATE_KEYS: [&str; 5] = ["temp", "modo", "vento", "extra", "oscilar"];
const SETTABLE_KEYS: [&str; 9] = ["ligado", "temp", "modo", "vento", "extra", "oscilar", "display", "bipe", "autoclean"];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Run {
    Idle,
    Command,
    Refresh,
    Account,
}

/// Where the SmartThings connection stands, which picks the panel's screen.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Account {
    Starting,
    SignedOut,
    SigningIn,
    Choosing,
    NoDevice,
    Ready,
    Demo,
}

impl Account {
    fn name(self) -> &'static str {
        match self {
            Account::Starting => "starting",
            Account::SignedOut => "signedOut",
            Account::SigningIn => "signingIn",
            Account::Choosing => "choosing",
            Account::NoDevice => "noDevice",
            Account::Ready => "ready",
            Account::Demo => "demo",
        }
    }

    fn connected(self) -> bool {
        matches!(self, Account::Ready | Account::Demo)
    }
}

enum AccountJob {
    Login,
    Choose(String, Option<String>),
    ForgetDevice,
    Logout,
}

enum LocalJob {
    SetEnabled(bool),
    Find,
    Choose(String),
}

enum Job {
    Run(Changes),
    Account(AccountJob),
    Local(LocalJob),
}

struct State {
    confirmed: Option<Device>,
    pending: Changes,
    inflight: Changes,
    run: Run,
    error: Value,
    updated_at: Option<(u64, Instant)>,
    account: Account,
    devices: Vec<Value>,
    login_error: bool,
    account_job: Option<AccountJob>,
    local_job: Option<LocalJob>,
    local: Value,
    flush_at: Option<Instant>,
    refresh_wanted: bool,
    next_poll: Instant,
    visible: bool,
    demo: bool,
}

fn put(changes: &mut Changes, key: &str, value: Value) {
    match changes.iter_mut().find(|(existing, _)| existing == key) {
        Some(entry) => entry.1 = value,
        None => changes.push((key.to_owned(), value)),
    }
}

fn has(changes: &Changes, key: &str) -> bool {
    changes.iter().any(|(existing, _)| existing == key)
}

impl State {
    fn poll_interval(&self) -> Duration {
        if self.visible { POLL_VISIBLE } else { POLL_HIDDEN }
    }

    /// Setpoint, mode, fan and direction only take effect on a running unit, so they wait for power.
    fn sendable(&self) -> Changes {
        let power = self.pending.iter().find(|(key, _)| key == "ligado").map(|(_, value)| value);
        let will_be_on = match power {
            Some(value) => value.as_bool().unwrap_or(false),
            None => self.confirmed.as_ref().is_some_and(|device| device["ligado"] == true),
        };
        self.pending.iter().filter(|(key, _)| will_be_on || !CLIMATE_KEYS.contains(&key.as_str())).cloned().collect()
    }

    fn view(&self) -> Value {
        let sendable = self.sendable();
        let device = self.confirmed.as_ref().map(|confirmed| {
            let mut device = confirmed.clone();
            for (key, value) in self.inflight.iter().chain(&self.pending) {
                device.insert(key.clone(), value.clone());
            }
            device
        });
        let held: Vec<&String> = self.pending.iter().map(|(key, _)| key).filter(|key| !has(&sendable, key)).collect();
        json!({
            "device": device,
            "syncing": self.run == Run::Command || !sendable.is_empty(),
            "held": held,
            "error": self.error,
            "updatedAt": self.updated_at.map(|(millis, _)| millis),
            "account": self.account.name(),
            "devices": self.devices,
            "loginError": self.login_error,
            "local": self.local,
        })
    }

    /// Leaves the device behind, for a sign-out or a device change.
    fn clear_device(&mut self) {
        self.confirmed = None;
        self.pending.clear();
        self.inflight.clear();
        self.error = Value::Null;
        self.updated_at = None;
        self.flush_at = None;
    }

    fn finish_run(&mut self, result: Result<Outcome, Failure>) {
        let now = Instant::now();
        // A command returns at once with the change applied; reconcile the full state soon after,
        // without blocking the click, instead of waiting out the whole poll interval.
        let was_command = !self.inflight.is_empty();
        self.inflight.clear();
        match result {
            Ok(outcome) => {
                let millis = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as u64);
                self.confirmed = Some(outcome.device);
                self.updated_at = Some((millis, now));
                self.error = if outcome.pending.is_empty() { Value::Null } else { json!({ "kind": "unconfirmed", "keys": outcome.pending }) };
                self.account = if self.demo { Account::Demo } else { Account::Ready };
                self.devices.clear();
            }
            Err(Failure::SignedOut) => {
                self.clear_device();
                self.account = Account::SignedOut;
            }
            Err(Failure::Choose(devices)) => {
                self.clear_device();
                self.devices = devices;
                self.account = Account::Choosing;
            }
            Err(Failure::NoDevice) => {
                self.clear_device();
                self.account = Account::NoDevice;
            }
            Err(Failure::Failed(message)) => {
                log::line(format_args!("SmartThings: {message}"));
                self.error = json!({ "kind": "failed", "message": message });
                // A network error says nothing about the session, so the main panel shows the retry.
                if self.account == Account::Starting {
                    self.account = Account::Ready;
                }
            }
            Err(Failure::Cancelled) => {}
        }
        if self.account.connected() && !self.sendable().is_empty() {
            self.flush_at = Some(now + FLUSH_DELAY);
        }
        let reconcile = Duration::from_millis(1500);
        self.next_poll = now + if was_command && self.account.connected() { reconcile.min(self.poll_interval()) } else { self.poll_interval() };
    }
}

pub type Publish = Box<dyn Fn(&Value) + Send + Sync>;

pub struct Bridge {
    state: Mutex<State>,
    wake: Condvar,
    publish: Publish,
    login: Option<Arc<LoginControl>>,
    /// Raised while a command waits, so a refresh in progress steps aside.
    interrupt: Arc<AtomicBool>,
}

impl Bridge {
    /// Starts the worker thread, which refreshes right away.
    pub fn start(mut unit: Box<dyn Unit>, publish: Publish) -> Arc<Self> {
        let interrupt = Arc::new(AtomicBool::new(false));
        unit.set_interrupt(Arc::clone(&interrupt));
        let bridge = Arc::new(Self {
            state: Mutex::new(State {
                confirmed: None,
                pending: Vec::new(),
                inflight: Vec::new(),
                run: Run::Idle,
                error: Value::Null,
                updated_at: None,
                account: Account::Starting,
                devices: Vec::new(),
                login_error: false,
                account_job: None,
                local_job: None,
                local: Value::Null,
                flush_at: None,
                refresh_wanted: true,
                next_poll: Instant::now(),
                visible: false,
                demo: unit.is_demo(),
            }),
            wake: Condvar::new(),
            publish,
            login: unit.login_control(),
            interrupt,
        });
        let worker = Arc::clone(&bridge);
        thread::spawn(move || worker.work(unit));
        bridge
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Changes the state, wakes the worker and publishes outside the lock, because publishing
    /// touches the tray on the main thread.
    fn update(&self, change: impl FnOnce(&mut State)) {
        let view = {
            let mut state = self.lock();
            change(&mut state);
            state.view()
        };
        self.wake.notify_one();
        (self.publish)(&view);
    }

    pub fn set(&self, key: &str, value: Value) {
        if !SETTABLE_KEYS.contains(&key) {
            return;
        }
        self.update(|state| {
            if !state.account.connected() {
                return;
            }
            let already_there = state.confirmed.as_ref().and_then(|device| device.get(key)) == Some(&value) && !has(&state.inflight, key);
            if already_there {
                state.pending.retain(|(existing, _)| existing != key);
            } else {
                put(&mut state.pending, key, value);
            }
            state.error = Value::Null;
            state.flush_at = Some(Instant::now() + FLUSH_DELAY);
            if !state.sendable().is_empty() {
                self.interrupt.store(true, Ordering::Relaxed);
            }
        });
    }

    pub fn refresh(&self) {
        self.update(|state| state.refresh_wanted = true);
    }

    pub fn set_visible(&self, visible: bool) {
        self.update(|state| {
            state.visible = visible;
            state.next_poll = Instant::now() + state.poll_interval();
            let stale = state.updated_at.is_none_or(|(_, at)| at.elapsed() > STALE_AFTER);
            if visible && stale && state.account.connected() {
                state.refresh_wanted = true;
            }
        });
    }

    fn account_job(&self, job: AccountJob, account: Account) {
        self.update(|state| {
            if !matches!(job, AccountJob::Login) {
                state.clear_device();
                state.devices.clear();
            }
            state.account_job = Some(job);
            state.account = account;
            state.login_error = false;
        });
    }

    /// Opens the Samsung login in the browser.
    pub fn login(&self) {
        if self.lock().account == Account::SigningIn {
            self.reopen_login();
            return;
        }
        self.account_job(AccountJob::Login, Account::SigningIn);
    }

    pub fn cancel_login(&self) {
        if let Some(control) = &self.login {
            control.cancel();
        }
    }

    pub fn reopen_login(&self) {
        if let Some(control) = &self.login {
            control.reopen();
        }
    }

    pub fn choose_device(&self, id: &str, name: Option<&str>) {
        self.account_job(AccountJob::Choose(id.to_owned(), name.map(str::to_owned)), Account::Starting);
    }

    /// Forgets the saved device and looks for air conditioners again.
    pub fn change_device(&self) {
        self.account_job(AccountJob::ForgetDevice, Account::Starting);
    }

    pub fn logout(&self) {
        self.account_job(AccountJob::Logout, Account::SignedOut);
    }

    fn local_job(&self, job: LocalJob) {
        self.update(|state| state.local_job = Some(job));
    }

    /// Turns the home-network path on or off.
    pub fn set_local(&self, enabled: bool) {
        self.local_job(LocalJob::SetEnabled(enabled));
    }

    /// Looks for air conditioners on the network.
    pub fn find_local(&self) {
        self.local_job(LocalJob::Find);
    }

    /// Picks the unit at this address for the network path.
    pub fn choose_local(&self, addr: &str) {
        self.local_job(LocalJob::Choose(addr.to_owned()));
    }

    fn next_job(&self) -> Job {
        let mut state = self.lock();
        loop {
            let now = Instant::now();
            if let Some(job) = state.account_job.take() {
                state.run = Run::Account;
                return Job::Account(job);
            }
            if let Some(job) = state.local_job.take() {
                state.run = Run::Account;
                return Job::Local(job);
            }
            if state.account.connected() && state.flush_at.is_some_and(|at| at <= now) {
                state.flush_at = None;
                let changes = state.sendable();
                if !changes.is_empty() {
                    state.pending.retain(|(key, _)| !has(&changes, key));
                    state.inflight = changes.clone();
                    self.interrupt.store(false, Ordering::Relaxed);
                    state.run = Run::Command;
                    return Job::Run(changes);
                }
            }
            // Without a connected account there is nothing to poll until the panel asks.
            if state.refresh_wanted || (state.account.connected() && now >= state.next_poll) {
                state.refresh_wanted = false;
                self.interrupt.store(false, Ordering::Relaxed);
                state.run = Run::Refresh;
                return Job::Run(Vec::new());
            }
            let deadline = match state.flush_at {
                Some(at) if state.account.connected() => at.min(state.next_poll),
                _ => state.next_poll,
            };
            let wait = if state.account.connected() { deadline.saturating_duration_since(now) } else { Duration::from_secs(3600) };
            state = self.wake.wait_timeout(state, wait).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
        }
    }

    /// Runs one job at a time; a failure inside a job is logged and the worker keeps going.
    fn work(&self, mut unit: Box<dyn Unit>) {
        loop {
            let job = self.next_job();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run_job(&mut *unit, job)));
            if result.is_err() {
                self.update(|state| {
                    state.run = Run::Idle;
                    state.inflight.clear();
                    state.error = json!({ "kind": "failed", "message": "falha interna" });
                    if state.account == Account::SigningIn || state.account == Account::Starting {
                        state.account = if state.confirmed.is_some() { Account::Ready } else { Account::SignedOut };
                    }
                });
            }
        }
    }

    fn run_job(&self, unit: &mut dyn Unit, job: Job) {
        {
            match job {
                Job::Run(changes) => {
                    let view = self.lock().view();
                    (self.publish)(&view);
                    let result = unit.run(&changes);
                    if let Err(failure) = &result {
                        log::line(format_args!("atualização: {failure:?}"));
                    }
                    let local = unit.local_view();
                    self.update(|state| {
                        state.run = Run::Idle;
                        state.local = local;
                        state.finish_run(result);
                    });
                }
                Job::Local(job) => {
                    match &job {
                        LocalJob::SetEnabled(enabled) => unit.set_local(*enabled),
                        LocalJob::Find => unit.find_local(),
                        LocalJob::Choose(addr) => unit.choose_local(addr),
                    }
                    let local = unit.local_view();
                    self.update(|state| {
                        state.run = Run::Idle;
                        state.local = local;
                        // Pull fresh state through the path the user just changed.
                        state.refresh_wanted = true;
                    });
                }
                Job::Account(AccountJob::Login) => {
                    log::line("login: abrindo o navegador");
                    let result = unit.login();
                    log::line(format_args!("login: {result:?}"));
                    self.update(|state| {
                        state.run = Run::Idle;
                        match result {
                            Ok(()) => {
                                state.account = Account::Starting;
                                state.refresh_wanted = true;
                            }
                            Err(Failure::Cancelled) => state.account = Account::SignedOut,
                            Err(_) => {
                                state.account = Account::SignedOut;
                                state.login_error = true;
                            }
                        }
                    });
                }
                Job::Account(job) => {
                    log::line(format_args!("conta: {}", match &job {
                        AccountJob::Choose(..) => "escolher aparelho",
                        AccountJob::ForgetDevice => "trocar de aparelho",
                        AccountJob::Logout => "sair",
                        AccountJob::Login => "entrar",
                    }));
                    match &job {
                        AccountJob::Choose(id, name) => unit.choose(id, name.as_deref()),
                        AccountJob::ForgetDevice => unit.forget_device(),
                        AccountJob::Logout => unit.logout(),
                        AccountJob::Login => unreachable!("login has its own arm"),
                    }
                    self.update(|state| {
                        state.run = Run::Idle;
                        state.clear_device();
                        state.devices.clear();
                        if matches!(job, AccountJob::Logout) {
                            state.account = Account::SignedOut;
                        } else {
                            state.account = Account::Starting;
                            state.refresh_wanted = true;
                        }
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    struct Script {
        signed_in: bool,
        devices: usize,
        chosen: bool,
    }

    impl Unit for Script {
        fn run(&mut self, _changes: &[(String, Value)]) -> Result<Outcome, Failure> {
            if !self.signed_in {
                return Err(Failure::SignedOut);
            }
            match (self.devices, self.chosen) {
                (0, _) => Err(Failure::NoDevice),
                (1, _) | (_, true) => Ok(Outcome { device: json!({ "ligado": true, "temp": 23 }).as_object().unwrap().clone(), pending: vec![] }),
                (count, false) => Err(Failure::Choose((0..count).map(|index| json!({ "id": index.to_string(), "nome": format!("Ar {index}") })).collect())),
            }
        }

        fn login(&mut self) -> Result<(), Failure> {
            self.signed_in = true;
            Ok(())
        }

        fn choose(&mut self, _id: &str, _name: Option<&str>) {
            self.chosen = true;
        }

        fn logout(&mut self) {
            self.signed_in = false;
        }
    }

    fn wait_for(views: &mpsc::Receiver<Value>, account: &str) -> Value {
        loop {
            let view = views.recv_timeout(Duration::from_secs(5)).expect("o estado deveria mudar");
            if view["account"] == account {
                return view;
            }
        }
    }

    #[test]
    fn walks_from_signed_out_to_ready_and_back() {
        let (sender, views) = mpsc::channel();
        let sender = Mutex::new(sender);
        let unit = Script { signed_in: false, devices: 2, chosen: false };
        let bridge = Bridge::start(Box::new(unit), Box::new(move |view| drop(sender.lock().unwrap().send(view.clone()))));
        wait_for(&views, "signedOut");
        bridge.login();
        let choosing = wait_for(&views, "choosing");
        assert_eq!(choosing["devices"].as_array().map(Vec::len), Some(2));
        bridge.choose_device("1", Some("Ar 1"));
        let ready = wait_for(&views, "ready");
        assert_eq!(ready["device"]["temp"], 23);
        bridge.logout();
        let signed_out = wait_for(&views, "signedOut");
        assert!(signed_out["device"].is_null());
    }
}
