//! What the admin terminal knows and does. Slow work (Service Control, HTTP, installing)
//! runs on background threads and reports back through channels, so the screen never stalls.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use comptroller_admin::config::{self, Config, Place};
use comptroller_admin::directory::{self, Directory};
use comptroller_admin::health::{self, Health};
use comptroller_admin::install::{self, Plan, Progress};
use comptroller_admin::preflight::{self, Check};
use comptroller_admin::service::{self, Installed, State};
use comptroller_admin::cost::{self, Period, Report};
use comptroller_admin::pricing::{self, Pricing};
use comptroller_admin::traffic::{self, Request, Summary, Today};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Terminal,
    Status,
    Preflight,
    Install,
    Uninstall,
    Settings,
    Users,
    Radar,
    Dashboard,
    Cost,
    BlackBox,
}

impl Screen {
    /// Screens that show today's figures, so the poller reads them while one is open.
    fn shows_today(self) -> bool {
        matches!(self, Screen::Terminal | Screen::Radar | Screen::Users | Screen::Status)
    }

    /// Screens that refresh at the Dashboard's rate rather than every few seconds.
    fn live(self) -> bool {
        matches!(self, Screen::Radar | Screen::Dashboard)
    }

    pub fn title(self) -> &'static str {
        match self {
            Screen::Terminal => "Terminal",
            Screen::Status => "Status",
            Screen::Preflight => "Preflight test",
            Screen::Install => "Install",
            Screen::Uninstall => "Uninstall",
            Screen::Settings => "Settings",
            Screen::Users => "Users",
            Screen::Radar => "Radar",
            Screen::Dashboard => "Dashboard",
            Screen::Cost => "Cost",
            Screen::BlackBox => "Black box",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Open(Screen),
    Takeoff,
    Terminate,
    Uninstall,
    /// A screen from a later build step.
    Later(u8),
}

pub struct MenuItem {
    pub key: char,
    pub title: &'static str,
    pub action: Action,
}

pub const SECTIONS: [(&str, &[MenuItem]); 5] = [
    ("WATCH", &[
        MenuItem { key: 'd', title: "Dashboard", action: Action::Open(Screen::Dashboard) },
        MenuItem { key: 'r', title: "Radar", action: Action::Open(Screen::Radar) },
        // 4: the $ key without Shift. No key here needs a modifier.
        MenuItem { key: '4', title: "Cost", action: Action::Open(Screen::Cost) },
        MenuItem { key: 's', title: "Status", action: Action::Open(Screen::Status) },
    ]),
    ("CONTROL", &[
        MenuItem { key: 't', title: "Takeoff", action: Action::Takeoff },
        MenuItem { key: 'x', title: "Terminate", action: Action::Terminate },
        MenuItem { key: 'p', title: "Preflight test", action: Action::Open(Screen::Preflight) },
    ]),
    ("DATA", &[
        MenuItem { key: 'b', title: "Black box", action: Action::Open(Screen::BlackBox) },
        MenuItem { key: 'a', title: "Archives", action: Action::Later(10) },
    ]),
    ("CONFIGURE", &[
        MenuItem { key: 'c', title: "Settings", action: Action::Open(Screen::Settings) },
        MenuItem { key: 'v', title: "Users", action: Action::Open(Screen::Users) },
    ]),
    ("SERVICE", &[
        MenuItem { key: 'i', title: "Install", action: Action::Open(Screen::Install) },
        MenuItem { key: 'u', title: "Uninstall", action: Action::Uninstall },
    ]),
];

pub fn menu_items() -> impl Iterator<Item = &'static MenuItem> {
    SECTIONS.iter().flat_map(|(_, items)| items.iter())
}

/// What the background poller last saw.
#[derive(Clone)]
pub struct Snapshot {
    pub place: Place,
    pub config: Result<Config, String>,
    pub service: Result<Option<Installed>, String>,
    pub health: Option<Health>,
    pub db_size: Option<u64>,
    pub upstream: Option<Result<(u16, Duration), String>>,
    pub service_name: String,
    /// Users and devices, read only while the Users screen is open. `Ok(None)`: no database yet.
    pub users: Option<Result<Option<Directory>, String>>,
    /// Today's figures, read while a screen that shows them is open.
    pub today: Option<Result<Option<Today>, String>>,
    /// The newest recorded requests, read while Radar or the Dashboard is open, with Radar's filters.
    pub feed: Option<Result<Option<Vec<Request>>, String>>,
    /// The Dashboard's period, read while it's open.
    pub summary: Option<Result<Option<Summary>, String>>,
    /// The Cost screen's report and the pricing table it used, read while it's open.
    pub cost: Option<CostView>,
    /// Per-day counts, read while the Black box is open. `Ok(None)`: no database yet.
    pub black_box: Option<Result<Option<comptroller_admin::blackbox::BlackBox>, String>>,
}

#[derive(Clone)]
pub struct CostView {
    pub pricing: Result<(Pricing, pricing::Source), String>,
    pub report: Result<Report, String>,
}

/// What the open screen asks the poller for.
#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    /// Radar: one person, and errors only.
    pub feed_user: Option<String>,
    pub errors_only: bool,
    /// Dashboard: 1, 7 or 14 days.
    pub days: u32,
    pub period: Period,
}

impl Default for Params {
    fn default() -> Self {
        Self { feed_user: None, errors_only: false, days: 1, period: Period::Month }
    }
}

impl Snapshot {
    pub fn installed(&self) -> Option<&Installed> {
        self.service.as_ref().ok().and_then(Option::as_ref)
    }

    pub fn state(&self) -> State {
        self.installed().map(|s| s.state).unwrap_or(State::NotInstalled)
    }

    /// The service is running in a console window rather than under Service Control.
    pub fn console(&self) -> bool {
        self.installed().is_none() && self.health.as_ref().is_some_and(|h| !h.windows_service)
    }

    pub fn config_or_default(&self) -> Config {
        self.config.clone().unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StepState {
    Pending,
    Running,
    Done(String),
    Failed(String),
}

pub struct StepsJob {
    pub plan: Plan,
    pub states: Vec<StepState>,
    pub finished: bool,
    receiver: Receiver<Progress>,
}

impl StepsJob {
    pub fn failed(&self) -> bool {
        self.states.iter().any(|s| matches!(s, StepState::Failed(_)))
    }

}

/// A quick job whose only output is a message.
pub enum Busy {
    Starting,
    Stopping,
    Restarting,
}

/// A change to users and devices that asks first.
#[derive(Clone, Debug)]
pub enum UserChange {
    RevokeDevice { id: i64, device: String, user: String },
    RemoveDevice { id: i64, device: String, user: String },
    RevokeUser { id: i64, user: String },
    RemoveUser { id: i64, user: String },
    CancelCode { hash: String, device: String, user: String },
}

pub enum Confirm {
    Terminate,
    Uninstall { remove_data: bool },
    Elevate(&'static str),
    /// Leaving Settings with changes not saved.
    Unsaved,
    /// Before editing a setting that affects every developer.
    Caution(usize),
    /// Saved settings that take effect after a restart.
    Restart(Vec<&'static str>),
    User(UserChange),
    /// Removing this many requests older than the retention, now.
    Prune(i64),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warn,
}

pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    until: Instant,
}

/// Messages to the background poller.
enum Poll {
    /// Check now.
    Now,
    /// Check now, including through the proxy to the upstream.
    WithUpstream,
    /// The open screen: Status gets the upstream check, Users and Radar their database reads.
    Watching(Screen),
    /// The open screen's choices, which also reads again at once.
    Params(Params),
}

/// How many recorded requests Radar reads.
pub const FEED_ROWS: usize = 200;

pub struct Options {
    pub config: Option<PathBuf>,
    pub service_name: Option<String>,
    pub install_dir: Option<PathBuf>,
}

pub struct App {
    pub screen: Screen,
    pub selected: usize,
    pub tick: u64,
    pub quit: bool,
    pub show_help: bool,
    /// The Commands panel at the bottom; `o` hides and shows it.
    pub show_commands: bool,
    pub snapshot: Snapshot,
    pub elevated: bool,
    pub install_dir: PathBuf,
    pub confirm: Option<Confirm>,
    pub toast: Option<Toast>,
    pub steps: Option<StepsJob>,
    pub busy: Option<Busy>,
    busy_result: Option<Receiver<Result<String, String>>>,
    pub preflight: Vec<Check>,
    pub preflight_running: bool,
    pub preflight_took: Option<Duration>,
    pub last_preflight: Option<(bool, Instant)>,
    preflight_results: Option<Receiver<Option<Check>>>,
    snapshots: Receiver<Snapshot>,
    refresh: Sender<Poll>,
    watching: Screen,
    pub settings: crate::settings_screen::State,
    pub users: crate::users_screen::State,
    pub radar: crate::radar_screen::State,
    pub cost: crate::cost_screen::State,
    /// What the screens ask the poller for.
    pub params: Params,
    /// Settings saved that the running service only reads when it starts, and when.
    pub restart_waiting: Option<(Vec<&'static str>, Instant)>,
}

/// How often the background checks run.
pub const POLL_SECS: u64 = 5;
/// How often the check through the proxy to the upstream runs, and only while Status is open.
const UPSTREAM_SECS: u64 = 60;

fn poll_once(
    options: &Options,
    last_upstream: &mut (Instant, Option<Result<(u16, Duration), String>>),
    force_upstream: bool,
    watching: Screen,
    params: &Params,
) -> Snapshot {
    let watching_status = watching == Screen::Status;
    let default_place = || Place { config_path: options.install_dir.clone().unwrap_or_else(config::default_install_dir).join(config::CONFIG_FILE) };
    let mut service_name = options.service_name.clone().unwrap_or_else(|| config::DEFAULT_SERVICE_NAME.into());

    // The service name can come from the config, which can come from the service.
    let probe = options.config.clone().unwrap_or_else(|| default_place().config_path);
    if options.service_name.is_none() {
        if let Ok(config) = Config::load(&probe) {
            service_name = config.service.name;
        }
    }
    let service = service::query(&service_name);

    let mut place = match (&options.config, &service) {
        (Some(path), _) => Place { config_path: path.clone() },
        (None, Ok(Some(installed))) => Place {
            config_path: installed
                .config_arg
                .clone()
                .unwrap_or_else(|| installed.exe.parent().map(|d| d.join(config::CONFIG_FILE)).unwrap_or_default()),
        },
        _ => default_place(),
    };
    let mut config = Config::load(&place.config_path);
    let port = config.as_ref().map(|c| c.proxy.admin_port).unwrap_or(8081);
    let health = health::fetch(port);

    // Not installed but a console instance answers: use the file it reports.
    if options.config.is_none() && matches!(service, Ok(None)) {
        if let Some(h) = &health {
            let reported = PathBuf::from(&h.config.path);
            if reported != place.config_path {
                place = Place { config_path: reported };
                config = Config::load(&place.config_path);
            }
        }
    }

    let db_size = config.as_ref().ok().and_then(|c| place.database_size(c));

    // Through the proxy to the upstream: only while Status is open, once a minute, or when asked.
    if health.is_none() {
        *last_upstream = (Instant::now(), None);
    } else if force_upstream || (watching_status && (last_upstream.1.is_none() || last_upstream.0.elapsed() > Duration::from_secs(UPSTREAM_SECS))) {
        let base = config::local_url(&health.as_ref().map(|h| h.listen_url.clone()).unwrap_or_default());
        *last_upstream = (Instant::now(), Some(health::upstream_latency(&base)));
    }

    let db = config.as_ref().ok().map(|c| place.database(c));
    let users = match (&db, watching) {
        (Some(db), Screen::Users) => Some(directory::load(db)),
        _ => None,
    };
    let today = match &db {
        Some(db) if watching.shows_today() => Some(traffic::today(db)),
        _ => None,
    };
    let feed = match (&db, watching) {
        (Some(db), Screen::Radar) => Some(traffic::recent(db, FEED_ROWS, params.feed_user.as_deref(), params.errors_only)),
        (Some(db), Screen::Dashboard) => Some(traffic::recent(db, 8, None, false)),
        _ => None,
    };
    let offset = comptroller_admin::local_offset();
    let summary = match (&db, watching) {
        (Some(db), Screen::Dashboard) => Some(traffic::summary(db, params.days, offset)),
        _ => None,
    };
    let cost = match (&config, &db, watching) {
        (Ok(c), Some(db), Screen::Cost) => {
            let pricing = pricing::load(&place.resolve(&c.cost.pricing_file));
            let report = match &pricing {
                Err(e) => Err(e.clone()),
                Ok(_) if !db.exists() => Err("No database yet. The service creates it when it first starts.".into()),
                Ok((table, _)) => {
                    let today = (health::now() + offset).div_euclid(86_400);
                    cost::report(db, &c.cost, table, params.period, today, offset)
                }
            };
            Some(CostView { pricing, report })
        }
        _ => None,
    };

    let black_box = match (&db, watching) {
        (Some(db), Screen::BlackBox) => Some(comptroller_admin::blackbox::read(db, offset)),
        _ => None,
    };

    Snapshot { place, config, service, health, db_size, upstream: last_upstream.1.clone(), service_name, users, today, feed, summary, cost, black_box }
}

impl App {
    pub fn new(options: Options) -> Self {
        let install_dir = options.install_dir.clone().unwrap_or_else(config::default_install_dir);
        let mut last_upstream = (Instant::now(), None);
        let first = poll_once(&options, &mut last_upstream, false, Screen::Terminal, &Params::default());
        let mut live_secs = first.config_or_default().dashboard.refresh_seconds.clamp(1, 60);

        let (snapshot_tx, snapshots) = channel();
        let (refresh, refresh_rx) = channel::<Poll>();
        thread::spawn(move || {
            comptroller_admin::activity::mark_thread_background();
            let mut watching = Screen::Terminal;
            let mut params = Params::default();
            loop {
                // Every few seconds, or straight away when asked. Radar and the Dashboard are live.
                let mut forced = false;
                let every = if watching.live() { live_secs } else { POLL_SECS };
                match refresh_rx.recv_timeout(Duration::from_secs(every)) {
                    Ok(Poll::Now) => {}
                    Ok(Poll::WithUpstream) => forced = true,
                    Ok(Poll::Watching(screen)) => {
                        watching = screen;
                        continue;
                    }
                    Ok(Poll::Params(p)) => params = p,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => return,
                }
                let snapshot = poll_once(&options, &mut last_upstream, forced, watching, &params);
                live_secs = snapshot.config_or_default().dashboard.refresh_seconds.clamp(1, 60);
                if snapshot_tx.send(snapshot).is_err() {
                    return;
                }
            }
        });

        Self {
            screen: Screen::Terminal,
            selected: 0,
            tick: 0,
            quit: false,
            show_help: false,
            show_commands: true,
            snapshot: first,
            elevated: service::is_admin(),
            install_dir,
            confirm: None,
            toast: None,
            steps: None,
            busy: None,
            busy_result: None,
            preflight: Vec::new(),
            preflight_running: false,
            preflight_took: None,
            last_preflight: None,
            preflight_results: None,
            snapshots,
            refresh,
            watching: Screen::Terminal,
            settings: Default::default(),
            users: Default::default(),
            radar: Default::default(),
            cost: Default::default(),
            params: Params::default(),
            restart_waiting: None,
        }
    }

    /// Asks the background poller to check again now.
    pub fn refresh_now(&self) {
        let _ = self.refresh.send(Poll::Now);
    }

    /// Tells the poller what the screens ask for, which also reads again at once.
    pub fn send_params(&self) {
        let _ = self.refresh.send(Poll::Params(self.params.clone()));
    }

    /// Seconds between updates on Radar and the Dashboard.
    pub fn live_secs(&self) -> u64 {
        self.snapshot.config_or_default().dashboard.refresh_seconds.clamp(1, 60)
    }

    /// Something is changing the service right now; quitting waits for it.
    pub fn working(&self) -> bool {
        self.busy.is_some() || self.steps.as_ref().is_some_and(|s| !s.finished)
    }

    pub fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.toast = Some(Toast { text: text.into(), kind, until: Instant::now() + Duration::from_secs(6) });
    }

    pub fn pump(&mut self) {
        if self.screen != self.watching {
            self.watching = self.screen;
            let _ = self.refresh.send(Poll::Watching(self.screen));
            match self.screen {
                Screen::Status => {
                    let _ = self.refresh.send(Poll::WithUpstream);
                }
                screen if screen.shows_today() => self.refresh_now(),
                _ => {}
            }
        }
        while let Ok(snapshot) = self.snapshots.try_recv() {
            self.snapshot = snapshot;
        }
        // The service started after the save, so it now uses those settings.
        if let (Some((_, saved)), Some(h)) = (&self.restart_waiting, &self.snapshot.health) {
            if h.uptime_seconds < saved.elapsed().as_secs() {
                self.restart_waiting = None;
            }
        }
        if self.toast.as_ref().is_some_and(|t| Instant::now() > t.until) {
            self.toast = None;
        }

        if let Some(rx) = &self.busy_result {
            if let Ok(result) = rx.try_recv() {
                self.busy = None;
                self.busy_result = None;
                let _ = self.refresh.send(Poll::Now);
                match result {
                    Ok(text) => self.toast(ToastKind::Success, text),
                    Err(text) => self.toast(ToastKind::Warn, text),
                }
            }
        }

        let mut install_finished = false;
        if let Some(job) = &mut self.steps {
            while let Ok(progress) = job.receiver.try_recv() {
                match progress {
                    Progress::Started(i) => job.states[i] = StepState::Running,
                    Progress::Done(i, detail) => job.states[i] = StepState::Done(detail),
                    Progress::Failed(i, error) => {
                        job.states[i] = StepState::Failed(error);
                        job.finished = true;
                    }
                    Progress::Finished => {
                        job.finished = true;
                        install_finished = self.screen == Screen::Install;
                    }
                }
            }
            if job.finished {
                let _ = self.refresh.send(Poll::Now);
            }
        }
        // A finished install goes straight on to the preflight test.
        if install_finished {
            self.screen = Screen::Preflight;
            self.run_preflight();
        }

        if let Some(rx) = &self.preflight_results {
            while let Ok(message) = rx.try_recv() {
                match message {
                    Some(check) => self.preflight.push(check),
                    None => {
                        self.preflight_running = false;
                        self.preflight_results = None;
                        self.preflight_took = Some(self.preflight.iter().map(|c| c.took).sum());
                        self.last_preflight = Some((self.preflight.iter().all(Check::passed), Instant::now()));
                        break;
                    }
                }
            }
        }
    }

    fn run_busy(&mut self, busy: Busy, work: impl FnOnce() -> Result<String, String> + Send + 'static) {
        let (tx, rx) = channel();
        thread::spawn(move || {
            let _ = tx.send(work());
        });
        self.busy = Some(busy);
        self.busy_result = Some(rx);
    }

    pub fn run_preflight(&mut self) {
        if self.preflight_running {
            return;
        }
        let (tx, rx) = channel();
        let config_path = self.snapshot.place.config_path.clone();
        let name = self.snapshot.service_name.clone();
        thread::spawn(move || {
            preflight::run(&config_path, &name, |check| {
                let _ = tx.send(Some(check.clone()));
            });
            let _ = tx.send(None);
        });
        self.preflight.clear();
        self.preflight_took = None;
        self.preflight_running = true;
        self.preflight_results = Some(rx);
    }

    /// Admin-only actions ask to reopen elevated instead of failing.
    pub fn needs_admin(&mut self, what: &'static str) -> bool {
        if self.elevated {
            return false;
        }
        self.confirm = Some(Confirm::Elevate(what));
        true
    }

    fn act(&mut self, action: Action) {
        match action {
            Action::Later(step) => self.toast(ToastKind::Info, format!("That screen arrives in build step {step}.")),
            Action::Open(Screen::Preflight) => {
                self.screen = Screen::Preflight;
                self.run_preflight();
            }
            Action::Open(Screen::Install) if self.snapshot.installed().is_some() => {
                let dir = self.snapshot.installed().map(|s| s.exe.parent().map(|p| p.display().to_string())).flatten();
                self.toast(ToastKind::Info, format!("Already installed in {}. Uninstall first to reinstall.", dir.unwrap_or_default()));
            }
            Action::Open(Screen::Settings) => {
                self.settings.load(&self.snapshot.place.config_path);
                self.screen = Screen::Settings;
            }
            Action::Open(screen) => self.screen = screen,
            Action::Takeoff => self.takeoff(),
            Action::Terminate => match self.snapshot.state() {
                _ if self.snapshot.console() => self.toast(ToastKind::Info, "The service runs in a console window. Stop it there with Ctrl+C."),
                State::Running => {
                    if !self.needs_admin("Terminate") {
                        self.confirm = Some(Confirm::Terminate);
                    }
                }
                State::NotInstalled => self.toast(ToastKind::Info, "The service isn't installed."),
                _ => self.toast(ToastKind::Info, "The service isn't running."),
            },
            Action::Uninstall => {
                if self.snapshot.installed().is_none() {
                    self.toast(ToastKind::Info, "The service isn't installed.");
                } else if !self.needs_admin("Uninstall") {
                    self.confirm = Some(Confirm::Uninstall { remove_data: false });
                }
            }
        }
    }

    fn takeoff(&mut self) {
        match self.snapshot.state() {
            _ if self.snapshot.console() => self.toast(ToastKind::Info, "Already running, in a console window."),
            State::NotInstalled => self.toast(ToastKind::Info, "Not installed yet. Press i to install."),
            State::Running => self.toast(ToastKind::Info, "Already running."),
            _ if self.needs_admin("Takeoff") => {}
            _ => {
                let name = self.snapshot.service_name.clone();
                let port = self.snapshot.config_or_default().proxy.admin_port;
                self.run_busy(Busy::Starting, move || {
                    service::start(&name)?;
                    service::wait_for(&name, State::Running, Duration::from_secs(30))?;
                    for _ in 0..20 {
                        if health::fetch(port).is_some() {
                            return Ok("Takeoff: the service is running.".into());
                        }
                        thread::sleep(Duration::from_millis(500));
                    }
                    Err("The service started but /health doesn't answer. Check Status and the log folder.".into())
                });
            }
        }
    }

    fn terminate(&mut self) {
        let name = self.snapshot.service_name.clone();
        let wait = Duration::from_secs(self.snapshot.config_or_default().proxy.shutdown_timeout_seconds as u64 + 30);
        self.run_busy(Busy::Stopping, move || {
            service::stop(&name)?;
            service::wait_for(&name, State::Stopped, wait)?;
            Ok("Terminated: the service has stopped.".into())
        });
    }

    /// Stop then start, so saved settings that are read at startup take effect.
    pub fn restart(&mut self) {
        if self.snapshot.console() {
            self.toast(ToastKind::Info, "The service runs in a console window: stop it with Ctrl+C and start it again.");
            return;
        }
        if self.snapshot.state() != State::Running {
            self.toast(ToastKind::Info, "The service isn't running. Takeoff starts it with the saved settings.");
            return;
        }
        if self.needs_admin("Restarting the service") {
            return;
        }
        let name = self.snapshot.service_name.clone();
        // The file as saved, which may have a new admin port.
        let config = Config::load(&self.snapshot.place.config_path).unwrap_or_else(|_| self.snapshot.config_or_default());
        let wait = Duration::from_secs(config.proxy.shutdown_timeout_seconds as u64 + 30);
        let port = config.proxy.admin_port;
        self.run_busy(Busy::Restarting, move || {
            service::stop(&name)?;
            service::wait_for(&name, State::Stopped, wait)?;
            service::start(&name)?;
            service::wait_for(&name, State::Running, Duration::from_secs(30))?;
            for _ in 0..20 {
                if health::fetch(port).is_some() {
                    return Ok("Restarted: the service uses the saved settings.".into());
                }
                thread::sleep(Duration::from_millis(500));
            }
            Err(format!("The service restarted but /health doesn't answer on port {port}. Check Status and the log folder."))
        });
    }

    pub fn start_install(&mut self) {
        if self.working() || self.snapshot.installed().is_some() || self.needs_admin("Install") {
            return;
        }
        let plan = install::install_plan(&self.snapshot.service_name, &self.install_dir);
        let (tx, rx) = channel();
        let runner = install::install_plan(&plan.name, &plan.dir);
        thread::spawn(move || install::install(&runner, &tx));
        self.steps = Some(StepsJob { states: vec![StepState::Pending; plan.steps.len()], plan, finished: false, receiver: rx });
    }

    fn start_uninstall(&mut self, remove_data: bool) {
        let place = self.snapshot.place.clone();
        let config = self.snapshot.config_or_default();
        let name = self.snapshot.service_name.clone();
        let plan = install::uninstall_plan(&name, &place, &config, remove_data);
        let runner = install::uninstall_plan(&name, &place, &config, remove_data);
        let (tx, rx) = channel();
        thread::spawn(move || install::uninstall(&runner, &place, &config, &tx));
        self.steps = Some(StepsJob { states: vec![StepState::Pending; plan.steps.len()], plan, finished: false, receiver: rx });
        self.screen = Screen::Uninstall;
    }

    fn open_logs(&mut self) {
        let config = self.snapshot.config_or_default();
        let dir = self.snapshot.place.log_dir(&config);
        if dir.exists() {
            let _ = comptroller_admin::activity::track(format!("explorer \"{}\"", dir.display()), || std::process::Command::new("explorer").arg(&dir).spawn(), |_| "opened".into());
        } else {
            self.toast(ToastKind::Info, format!("No log folder yet: {}", dir.display()));
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.show_help {
            self.show_help = false;
            return;
        }
        if let Some(confirm) = &mut self.confirm {
            match (confirm, key.code) {
                (Confirm::Uninstall { remove_data }, KeyCode::Char('d')) => *remove_data = !*remove_data,
                (Confirm::Uninstall { remove_data }, KeyCode::Char('y')) => {
                    let remove = *remove_data;
                    self.confirm = None;
                    self.start_uninstall(remove);
                }
                (Confirm::Terminate, KeyCode::Char('y')) => {
                    self.confirm = None;
                    self.terminate();
                }
                (Confirm::Elevate(_), KeyCode::Char('y')) => {
                    self.confirm = None;
                    match service::relaunch_as_admin() {
                        Ok(()) => self.quit = true,
                        Err(e) => self.toast(ToastKind::Warn, e),
                    }
                }
                (Confirm::Unsaved, KeyCode::Char('s')) => {
                    self.confirm = None;
                    crate::settings_screen::save(self);
                    if self.settings.pending.is_empty() && self.confirm.is_none() {
                        self.screen = Screen::Terminal;
                    }
                }
                (Confirm::Unsaved, KeyCode::Char('d')) => {
                    self.confirm = None;
                    self.settings.pending.clear();
                    self.screen = Screen::Terminal;
                }
                (Confirm::Caution(index), KeyCode::Char('y')) => {
                    let index = *index;
                    self.confirm = None;
                    self.settings.acknowledged.insert(index);
                    crate::settings_screen::edit(self);
                }
                (Confirm::Restart(_), KeyCode::Char('y')) => {
                    self.confirm = None;
                    self.restart();
                }
                (Confirm::Prune(_), KeyCode::Char('y')) => {
                    self.confirm = None;
                    crate::black_box_screen::prune(self);
                }
                (Confirm::User(change), KeyCode::Char('y')) => {
                    let change = change.clone();
                    self.confirm = None;
                    crate::users_screen::apply(self, change);
                }
                (_, KeyCode::Char('n') | KeyCode::Esc) => self.confirm = None,
                _ => {}
            }
            return;
        }

        // Typing goes to the field being edited, not to the keys below.
        match self.screen {
            Screen::Settings if self.settings.editing.is_some() => return crate::settings_screen::on_key(self, key),
            Screen::Users if self.users.typing() => return crate::users_screen::on_key(self, key),
            Screen::Cost if self.cost.editing() => return crate::cost_screen::on_key(self, key),
            _ => {}
        }

        if key.code == KeyCode::Char('?') {
            self.show_help = true;
            return;
        }
        if key.code == KeyCode::Char('o') {
            self.show_commands = !self.show_commands;
            return;
        }

        match self.screen {
            Screen::Terminal => self.on_terminal_key(key),
            Screen::Status => match key.code {
                KeyCode::Esc => self.screen = Screen::Terminal,
                KeyCode::Char('p') => self.act(Action::Open(Screen::Preflight)),
                KeyCode::Char('x') => self.act(Action::Terminate),
                KeyCode::Char('t') => self.act(Action::Takeoff),
                KeyCode::Char('l') => self.open_logs(),
                KeyCode::Char('r') => {
                    let _ = self.refresh.send(Poll::WithUpstream);
                    self.toast(ToastKind::Info, "Refreshing…");
                }
                _ => {}
            },
            Screen::Preflight => match key.code {
                KeyCode::Esc => self.screen = Screen::Terminal,
                KeyCode::Enter => self.run_preflight(),
                KeyCode::Char('s') => self.screen = Screen::Status,
                _ => {}
            },
            Screen::Install | Screen::Uninstall => match key.code {
                KeyCode::Esc if !self.working() => {
                    self.steps = None;
                    self.screen = Screen::Terminal;
                }
                KeyCode::Enter if self.screen == Screen::Install && self.steps.is_none() => self.start_install(),
                KeyCode::Char('l') => self.open_logs(),
                _ => {}
            },
            Screen::Settings => crate::settings_screen::on_key(self, key),
            Screen::Users => crate::users_screen::on_key(self, key),
            Screen::Radar => crate::radar_screen::on_key(self, key),
            Screen::Dashboard => crate::dashboard_screen::on_key(self, key),
            Screen::Cost => crate::cost_screen::on_key(self, key),
            Screen::BlackBox => crate::black_box_screen::on_key(self, key),
        }
    }

    fn on_terminal_key(&mut self, key: KeyEvent) {
        let count = menu_items().count();
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') => self.selected = (self.selected + count - 1) % count,
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => self.selected = (self.selected + 1) % count,
            KeyCode::Enter => {
                let action = menu_items().nth(self.selected).map(|i| i.action);
                if let Some(action) = action {
                    self.act(action);
                }
            }
            KeyCode::Char(c) => {
                if let Some((index, item)) = menu_items().enumerate().find(|(_, i)| i.key == c) {
                    self.selected = index;
                    self.act(item.action);
                }
            }
            _ => {}
        }
    }
}
