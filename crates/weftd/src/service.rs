//! "Start at login", Weft's own option (ADR-0034 D1): a per-user service that
//! runs `weft --serve` when the person logs in, and again when it ends by
//! itself. A launchd agent on macOS, a systemd user unit on Linux; the code
//! differs per system, never per harness or tunnel.
//!
//! The daemon a service runs ends cleanly on `weft stop` (exit 0), which
//! neither system restarts: the person's stop wins, a crash does not. A
//! second `weft --serve` beside a live daemon ends cleanly too, so a service
//! loaded while Weft already runs starts nothing over it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// The launchd label, and the plist's name.
pub const LABEL: &str = "com.fab7.weft";
/// The systemd unit's name.
const UNIT: &str = "weft.service";
/// Set to `1` in the service's environment: the daemon it runs knows it is
/// the service's, and never stops it when the option changes.
pub const MANAGED: &str = "WEFT_SERVICE";
/// How long `weft` waits for the daemon a service starts to listen.
pub const START_WITHIN: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    /// macOS: an agent in `~/Library/LaunchAgents`.
    Launchd,
    /// Linux: a unit in `~/.config/systemd/user`.
    Systemd,
}

/// The service of one user on one system.
#[derive(Debug, Clone)]
pub struct Service {
    system: System,
    home: PathBuf,
    uid: u32,
    /// Where `launchctl` and `systemctl` are, when not on `PATH`: a test's.
    tools: Option<PathBuf>,
}

/// Whether this daemon is the one a service runs.
pub fn managed() -> bool {
    std::env::var_os(MANAGED).is_some_and(|v| v == "1")
}

impl Service {
    pub fn new(system: System, home: PathBuf, uid: u32) -> Self {
        Service { system, home, uid, tools: None }
    }

    /// Run `launchctl` and `systemctl` from this folder.
    pub fn with_tools(mut self, dir: PathBuf) -> Self {
        self.tools = Some(dir);
        self
    }

    /// This user's, on this system; none where Weft has no service.
    pub fn here() -> Option<Self> {
        let system = if cfg!(target_os = "macos") {
            System::Launchd
        } else if cfg!(target_os = "linux") {
            System::Systemd
        } else {
            return None;
        };
        let home = PathBuf::from(std::env::var_os("HOME")?);
        // SAFETY: getuid has no preconditions and cannot fail.
        let uid = unsafe { libc::getuid() };
        Some(Service::new(system, home, uid))
    }

    /// The file the service is.
    pub fn file(&self) -> PathBuf {
        match self.system {
            System::Launchd => {
                self.home.join("Library/LaunchAgents").join(format!("{LABEL}.plist"))
            }
            System::Systemd => self.home.join(".config/systemd/user").join(UNIT),
        }
    }

    pub fn installed(&self) -> bool {
        self.file().is_file()
    }

    /// What the file holds: `program --serve`, with `path` as its `PATH`.
    pub fn text(&self, program: &Path, path: &str) -> String {
        let program = program.to_string_lossy();
        match self.system {
            System::Launchd => {
                let x = |s: &str| {
                    s.replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;")
                        .replace('"', "&quot;")
                };
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                     <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
                     <plist version=\"1.0\">\n<dict>\n\
                     \t<key>Label</key>\n\t<string>{LABEL}</string>\n\
                     \t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>{}</string>\n\t\t<string>--serve</string>\n\t</array>\n\
                     \t<key>EnvironmentVariables</key>\n\t<dict>\n\
                     \t\t<key>PATH</key>\n\t\t<string>{}</string>\n\
                     \t\t<key>{MANAGED}</key>\n\t\t<string>1</string>\n\t</dict>\n\
                     \t<key>RunAtLoad</key>\n\t<true/>\n\
                     \t<key>KeepAlive</key>\n\t<dict>\n\t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>\n\
                     \t<key>ProcessType</key>\n\t<string>Interactive</string>\n\
                     </dict>\n</plist>\n",
                    x(&program),
                    x(path)
                )
            }
            System::Systemd => {
                // Quoted, with systemd's own specifiers and variables kept literal.
                let q = |s: &str| {
                    let s = s.replace('\\', "\\\\").replace('"', "\\\"");
                    format!("\"{}\"", s.replace('%', "%%").replace('$', "$$"))
                };
                format!(
                    "[Unit]\nDescription=Weft\n\n\
                     [Service]\nExecStart={} --serve\nEnvironment={}\nEnvironment={}\n\
                     Restart=on-failure\nRestartSec=2\n\n\
                     [Install]\nWantedBy=default.target\n",
                    q(&program),
                    q(&format!("PATH={path}")),
                    q(&format!("{MANAGED}=1"))
                )
            }
        }
    }

    /// Write the service and load it. `managed`: this daemon is the one the
    /// service already runs, which loading again must not stop.
    pub fn install(&self, program: &Path, path: &str, managed: bool) -> Result<(), String> {
        let file = self.file();
        let dir = file.parent().expect("a folder");
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let fresh = file.with_extension("new");
        std::fs::write(&fresh, self.text(program, path))
            .and_then(|()| std::fs::rename(&fresh, &file))
            .map_err(|e| format!("{}: {e}", file.display()))?;
        match self.system {
            System::Launchd => {
                let job = self.job();
                if !managed {
                    // Loaded before, with another PATH: loaded again as written.
                    let _ = self.run("launchctl", &["bootout", &job]);
                }
                self.run("launchctl", &["enable", &job])?;
                if !managed {
                    let file = file.to_string_lossy();
                    self.run("launchctl", &["bootstrap", &self.domain(), &file])?;
                }
                Ok(())
            }
            System::Systemd => {
                self.run("systemctl", &["--user", "daemon-reload"])?;
                self.run("systemctl", &["--user", "enable", UNIT])
            }
        }
    }

    /// Remove the service, so nothing starts Weft at login. The daemon it
    /// runs (`managed`) goes on, with every agent in it.
    pub fn remove(&self, managed: bool) -> Result<(), String> {
        if self.system == System::Systemd {
            let _ = self.run("systemctl", &["--user", "disable", UNIT]);
        }
        let file = self.file();
        if let Err(e) = std::fs::remove_file(&file)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(format!("{}: {e}", file.display()));
        }
        match self.system {
            System::Launchd if !managed => {
                let _ = self.run("launchctl", &["bootout", &self.job()]);
            }
            System::Launchd => {}
            System::Systemd => {
                let _ = self.run("systemctl", &["--user", "daemon-reload"]);
            }
        }
        Ok(())
    }

    /// Start the daemon through the service, and wait for it to listen at
    /// `socket`.
    pub fn start(&self, socket: &Path, within: Duration) -> Result<(), String> {
        let started = match self.system {
            System::Launchd => {
                let job = self.job();
                self.run("launchctl", &["kickstart", &job]).or_else(|_| {
                    // Not loaded (booted out by hand): load it, which starts it.
                    let file = self.file().to_string_lossy().into_owned();
                    self.run("launchctl", &["bootstrap", &self.domain(), &file])
                })
            }
            System::Systemd => self.run("systemctl", &["--user", "start", UNIT]),
        };
        started?;
        let deadline = Instant::now() + within;
        while !weft_proto::is_live(socket) {
            if Instant::now() >= deadline {
                return Err(format!("Weft's service did not start within {}s", within.as_secs()));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    fn domain(&self) -> String {
        format!("gui/{}", self.uid)
    }

    fn job(&self) -> String {
        format!("{}/{LABEL}", self.domain())
    }

    fn run(&self, tool: &str, args: &[&str]) -> Result<(), String> {
        let program = self.tools.as_ref().map_or_else(|| PathBuf::from(tool), |d| d.join(tool));
        let out = Command::new(&program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("{tool} could not run: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        let said = String::from_utf8_lossy(&out.stderr);
        Err(format!("{tool} {} failed: {}", args.join(" "), said.trim()))
    }
}

/// How `weft` gets a daemon when none listens: through the service when it is
/// installed (`Ok(true)`, listening), else `Ok(false)`, and `weft` starts one
/// itself; an error when the service did not bring it up, and `weft` starts
/// one itself all the same.
pub fn start_daemon(service: Option<&Service>, socket: &Path) -> Result<bool, String> {
    match service.filter(|s| s.installed()) {
        Some(s) => s.start(socket, START_WITHIN).map(|()| true),
        None => Ok(false),
    }
}
