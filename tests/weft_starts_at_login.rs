//! "Start at login", Weft's own option (ADR-0034 D1, Phase 6.9 step 3): a
//! launchd agent or a systemd user unit running `weft --serve` with the
//! `PATH` the person had, written when the option is turned on and removed
//! when it is turned off. Turning Remote on or off never touches it.
//!
//! `launchctl` and `systemctl` are scripts here that write down how they were
//! called; the home is a scratch folder. The services themselves are not
//! tested: that is the platform's (ADR-0034 D7).

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weftd::service::{Service, System};

/// A scratch home, and a folder holding a `launchctl` and a `systemctl` that
/// write each call to `calls` and do what `then` says on `kickstart`/`start`.
struct World {
    root: PathBuf,
    home: PathBuf,
    tools: PathBuf,
}

impl World {
    fn new(name: &str) -> Self {
        Self::with(name, "")
    }

    fn with(name: &str, on_start: &str) -> Self {
        let root = std::env::temp_dir().join(format!("weft-login-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (home, tools) = (root.join("home"), root.join("tools"));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&tools).unwrap();
        for tool in ["launchctl", "systemctl"] {
            let path = tools.join(tool);
            let script = format!(
                "#!/bin/sh\necho \"{tool} $*\" >> \"{}\"\n\
                 case \"$*\" in *kickstart*|*\" start \"*) {on_start} ;; esac\nexit 0\n",
                tools.join("calls").display()
            );
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        World { root, home, tools }
    }

    fn service(&self, system: System) -> Service {
        Service::new(system, self.home.clone(), 501).with_tools(self.tools.clone())
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.tools.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn on_macos_turning_it_on_writes_a_launchd_agent_and_loads_it() {
    let w = World::new("launchd");
    let s = w.service(System::Launchd);
    let program = Path::new("/Users/me/.local/bin/weft");
    s.install(program, "/opt/homebrew/bin:/usr/bin", false).expect("installed");

    let file = w.home.join("Library/LaunchAgents/com.fab7.weft.plist");
    assert_eq!(s.file(), file);
    assert!(s.installed());
    let text = std::fs::read_to_string(&file).unwrap();
    for part in [
        "<key>Label</key>\n\t<string>com.fab7.weft</string>",
        "<key>ProgramArguments</key>\n\t<array>\n\t\t<string>/Users/me/.local/bin/weft</string>\n\t\t<string>--serve</string>\n\t</array>",
        "<key>PATH</key>\n\t\t<string>/opt/homebrew/bin:/usr/bin</string>",
        "<key>WEFT_SERVICE</key>\n\t\t<string>1</string>",
        "<key>RunAtLoad</key>\n\t<true/>",
        "<key>KeepAlive</key>\n\t<dict>\n\t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>",
    ] {
        assert!(text.contains(part), "{part:?} in\n{text}");
    }
    assert_eq!(
        w.calls(),
        [
            "launchctl bootout gui/501/com.fab7.weft".to_string(),
            "launchctl enable gui/501/com.fab7.weft".to_string(),
            format!("launchctl bootstrap gui/501 {}", file.display()),
        ]
    );

    s.remove(false).expect("removed");
    assert!(!file.exists() && !s.installed());
    assert_eq!(w.calls().last().unwrap(), "launchctl bootout gui/501/com.fab7.weft");
}

#[test]
fn on_linux_turning_it_on_writes_a_systemd_user_unit_and_enables_it() {
    let w = World::new("systemd");
    let s = w.service(System::Systemd);
    s.install(Path::new("/home/me/.local/bin/weft"), "/usr/local/bin:/usr/bin", false)
        .expect("installed");

    let file = w.home.join(".config/systemd/user/weft.service");
    assert_eq!(s.file(), file);
    let text = std::fs::read_to_string(&file).unwrap();
    for line in [
        "ExecStart=\"/home/me/.local/bin/weft\" --serve",
        "Environment=\"PATH=/usr/local/bin:/usr/bin\"",
        "Environment=\"WEFT_SERVICE=1\"",
        "Restart=on-failure",
        "WantedBy=default.target",
    ] {
        assert!(text.lines().any(|l| l == line), "{line:?} in\n{text}");
    }
    assert_eq!(
        w.calls(),
        ["systemctl --user daemon-reload", "systemctl --user enable weft.service"]
    );

    s.remove(false).expect("removed");
    assert!(!file.exists());
    assert_eq!(
        w.calls()[2..],
        ["systemctl --user disable weft.service", "systemctl --user daemon-reload"]
    );
}

/// The daemon the service runs is never stopped by turning the option off or
/// on again: that would end every agent in it. It leaves launchd's list when
/// it next ends, or at log out.
#[test]
fn the_daemon_the_service_runs_is_not_stopped_by_changing_the_option() {
    let w = World::new("managed");
    let s = w.service(System::Launchd);
    s.install(Path::new("/w/weft"), "/usr/bin", true).unwrap();
    s.remove(true).unwrap();
    assert!(w.calls().iter().all(|c| !c.contains("bootout")), "{:?}", w.calls());
    assert!(!s.installed());
}

/// A path with a space, a quote and an ampersand stays one path in each file.
#[test]
fn every_path_is_written_as_one_path() {
    let w = World::new("quoting");
    let odd = Path::new("/Users/a & b/\"x\"/weft");
    let mac = w.service(System::Launchd);
    mac.install(odd, "/a b:/c&d", false).unwrap();
    let text = std::fs::read_to_string(mac.file()).unwrap();
    assert!(text.contains("<string>/Users/a &amp; b/&quot;x&quot;/weft</string>"), "{text}");
    assert!(text.contains("<string>/a b:/c&amp;d</string>"), "{text}");

    let linux = w.service(System::Systemd);
    linux.install(Path::new("/home/a b/50%/weft"), "/a b:/$c", false).unwrap();
    let text = std::fs::read_to_string(linux.file()).unwrap();
    assert!(text.contains("ExecStart=\"/home/a b/50%%/weft\" --serve"), "{text}");
    assert!(text.contains("Environment=\"PATH=/a b:/$$c\""), "{text}");
}

/// While it is installed, `weft` starts the daemon through it, and waits for
/// the daemon to listen; not installed, nothing is started this way and
/// `weft` starts it itself.
#[test]
fn weft_starts_the_daemon_through_the_service_when_it_is_installed() {
    let runtime = std::env::temp_dir().join(format!("wl-rt-{}", std::process::id()));
    std::fs::create_dir_all(&runtime).unwrap();
    let weft = env!("CARGO_BIN_EXE_weft");
    // The service's start, as launchd would run it: `weft --serve`, detached.
    let on_start = format!(
        "XDG_RUNTIME_DIR=\"{}\" HOME=\"$WEFT_TEST_HOME\" nohup \"{weft}\" --serve </dev/null >/dev/null 2>&1 &",
        runtime.display()
    );
    let w = World::with("start", &on_start);
    // SAFETY: this test's process is its own (nextest); nothing else reads it.
    unsafe { std::env::set_var("WEFT_TEST_HOME", &w.home) };
    let socket = runtime.join("weft").join("weft.sock");
    let s = w.service(System::Launchd);

    assert!(!s.installed());
    assert_eq!(weftd::service::start_daemon(Some(&s), &socket), Ok(false), "not installed");
    assert!(w.calls().is_empty(), "nothing started through it");

    s.install(Path::new(weft), "/usr/bin:/bin", false).unwrap();
    assert_eq!(weftd::service::start_daemon(Some(&s), &socket), Ok(true));
    assert!(protocol::is_live(&socket), "listening when it returns");
    assert_eq!(w.calls().last().unwrap(), "launchctl kickstart gui/501/com.fab7.weft");

    // `weft stop` stops it, and leaves the service where it is.
    protocol::stop(&socket).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while protocol::is_live(&socket) {
        assert!(Instant::now() < deadline, "still running");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(s.installed(), "weft stop does not remove it");
    let _ = std::fs::remove_dir_all(&runtime);
}

/// A service that does not bring the daemon up within its time is said so,
/// so `weft` starts the daemon itself rather than waiting on it.
#[test]
fn a_service_that_starts_nothing_is_said_so() {
    let w = World::new("dud");
    let s = w.service(System::Systemd);
    s.install(Path::new("/w/weft"), "/usr/bin", false).unwrap();
    let socket = w.root.join("nobody.sock");
    let said = weftd::service::start_daemon(Some(&s), &socket);
    assert!(said.is_err(), "{said:?}");
    assert!(w.calls().contains(&"systemctl --user start weft.service".to_string()));
}

/// A second `weft --serve` beside a live daemon leaves it alone and ends
/// cleanly, so a service started beside one is not restarted over it.
#[test]
fn a_second_daemon_leaves_the_first_alone() {
    let socket = protocol::private_socket("weft-second");
    let config = socket.with_extension("no-config.toml");
    let (listening, read) = (socket.clone(), config.clone());
    std::thread::spawn(move || weft::server::Session::serve(&listening, &read));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !protocol::is_live(&socket) {
        assert!(Instant::now() < deadline, "no daemon");
        std::thread::sleep(Duration::from_millis(20));
    }
    let (said, ended) = std::sync::mpsc::channel();
    let (again, read) = (socket.clone(), config.clone());
    std::thread::spawn(move || said.send(weft::server::Session::serve(&again, &read).is_ok()));
    let second = ended.recv_timeout(Duration::from_secs(5));
    assert_eq!(second, Ok(true), "it ends at once, cleanly");
    assert!(protocol::is_live(&socket), "the first still listens");
    protocol::stop(&socket).unwrap();
}

// --- through the daemon ----------------------------------------------------

struct Client {
    next: u64,
    stream: UnixStream,
    lines: Lines<UnixStream>,
    /// Events that came while an answer was awaited.
    heard: Vec<Event>,
}

impl Client {
    fn open(socket: &Path, project: &Path) -> (Self, serde_json::Value) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            if let Ok(s) = UnixStream::connect(socket) {
                break s;
            }
            assert!(Instant::now() < deadline, "the server never came up");
            std::thread::sleep(Duration::from_millis(20));
        };
        let lines = Lines::new(stream.try_clone().unwrap());
        let mut c = Client { next: 1, stream, lines, heard: Vec::new() };
        c.answer(Call::Hello { client: "weft-tui".into(), protocol: PROTOCOL }).expect("hello");
        let path = project.to_string_lossy().into_owned();
        let opened = c.answer(Call::Open { path, rows: 24, cols: 80, raw: false }).expect("open");
        (c, opened)
    }

    fn answer(&mut self, call: Call) -> Result<serde_json::Value, String> {
        let id = self.next;
        self.next += 1;
        self.stream.write_all(&Line::Call { id, call }.encode()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            self.stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            match self.lines.next() {
                Ok(Some(Line::Result { id: got, result })) if got == id => return Ok(result),
                Ok(Some(Line::Error { id: got, code, .. })) if got == id => return Err(code),
                Ok(Some(Line::Event(e))) => self.heard.push(e),
                Ok(None) => break,
                _ => continue,
            }
        }
        panic!("no answer to call {id}");
    }

    fn told(&mut self) -> Option<bool> {
        let kept = self.heard.iter().find_map(|e| match e {
            Event::StartAtLogin { on } => Some(*on),
            _ => None,
        });
        self.heard.clear();
        if kept.is_some() {
            return kept;
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            self.stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
            if let Ok(Some(Line::Event(Event::StartAtLogin { on }))) = self.lines.next() {
                return Some(on);
            }
        }
        None
    }
}

fn daemon(w: &World, service: Service) -> (PathBuf, PathBuf) {
    let file = w.root.join("config.toml");
    std::fs::write(&file, "").unwrap();
    let socket = protocol::private_socket("weft-login");
    let world = weft::outside::Fixed::new().with_service(service);
    let (listening, read) = (socket.clone(), file.clone());
    std::thread::spawn(move || {
        let _ = weft::server::Session::serve_in(&listening, &read, None, world.shared());
    });
    (socket, file)
}

/// The option lives in `config.toml`, off until it is turned on; turning it
/// on and off from a window writes and removes the service with that
/// window's `PATH`, and every window is told.
#[test]
fn the_option_is_kept_in_config_and_changes_the_service() {
    let w = World::new("option");
    let s = w.service(System::Systemd);
    let (socket, config) = daemon(&w, s.clone());
    let (mut tui, opened) = Client::open(&socket, &w.root);
    assert_eq!(opened["start_at_login"], false, "off until turned on");

    let on = Call::StartAtLogin {
        on: true,
        program: "/home/me/.local/bin/weft".into(),
        path: "/usr/local/bin:/usr/bin".into(),
    };
    assert_eq!(tui.answer(on).unwrap()["start_at_login"], true);
    assert_eq!(tui.told(), Some(true));
    assert!(s.installed());
    let unit = std::fs::read_to_string(s.file()).unwrap();
    assert!(unit.contains("Environment=\"PATH=/usr/local/bin:/usr/bin\""), "{unit}");
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("[service]\nstart_at_login = true"), "{text}");

    let off = Call::StartAtLogin { on: false, program: String::new(), path: String::new() };
    assert_eq!(tui.answer(off).unwrap()["start_at_login"], false);
    assert_eq!(tui.told(), Some(false));
    assert!(!s.installed());
    assert!(std::fs::read_to_string(&config).unwrap().contains("start_at_login = false"));
    tui.answer(Call::Shutdown).ok();
}

/// Turning Remote on or off writes no service and runs no `launchctl` or
/// `systemctl` (ADR-0034 D1).
#[test]
fn turning_remote_on_or_off_never_touches_the_service() {
    let w = World::new("remote");
    let s = w.service(System::Launchd);
    let (socket, config) = daemon(&w, s.clone());
    let (mut tui, _) = Client::open(&socket, &w.root);
    for act in ["on", "off", "on", "off"] {
        tui.answer(Call::Remote { act: serde_json::json!({"act": act}) }).unwrap();
    }
    assert!(!s.installed());
    assert!(w.calls().is_empty(), "{:?}", w.calls());
    assert!(!std::fs::read_to_string(&config).unwrap().contains("start_at_login"));
    tui.answer(Call::Shutdown).ok();
}

/// A world with no service (a test's, or a system Weft has none for) says so
/// and keeps the option off.
#[test]
fn with_no_service_here_the_option_stays_off() {
    let w = World::new("none");
    let file = w.root.join("config.toml");
    std::fs::write(&file, "").unwrap();
    let socket = protocol::private_socket("weft-login-none");
    let (listening, read) = (socket.clone(), file.clone());
    std::thread::spawn(move || {
        let world = weft::outside::Fixed::new().shared();
        let _ = weft::server::Session::serve_in(&listening, &read, None, world);
    });
    let (mut tui, _) = Client::open(&socket, &w.root);
    let on = Call::StartAtLogin { on: true, program: "/w/weft".into(), path: "/usr/bin".into() };
    assert_eq!(tui.answer(on), Err("no_service".into()));
    assert!(!std::fs::read_to_string(&file).unwrap().contains("start_at_login"));
    tui.answer(Call::Shutdown).ok();
}
