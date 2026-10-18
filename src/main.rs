use anyhow::Result;

use weft::protocol;
use weft::server;

/// Put the starting `config.toml` where the person can see and change it,
/// once. The daemon only reads it; this runs before it starts.
fn write_config_if_absent(path: &std::path::Path) {
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, weft::routing::STARTING);
    }
}

/// fab7's files that came with the `weft` at `exe`, in its release: in the
/// bundle's Resources on macOS, beside the programs elsewhere.
fn bundled(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let dir = exe.parent()?;
    let fab7 = if dir.ends_with("Contents/MacOS") {
        dir.parent()?.join("Resources/fab7")
    } else {
        dir.join("fab7")
    };
    fab7.is_dir().then_some(fab7)
}

/// Whether a home set up from this version's files (`seeded`, the version
/// that last did) is set up again: one Homebrew updates is, once per
/// version, since no installer refreshes it; Weft's installer refreshes its own.
fn refresh(by: &Updater, seeded: Option<&str>, version: &str) -> bool {
    *by == Updater::Homebrew && seeded != Some(version)
}

/// A first start with nothing set up, as when the app came from a package
/// manager and no installer ran: Weft's harness and tool files, and
/// RingFrame's configuration (by the `ringframe` beside this `weft`), from
/// what came with it. What is there already is left alone, unless a newer
/// Homebrew Weft brought newer files.
fn seed(home: &std::path::Path, exe: &std::path::Path) {
    let Some(fab7) = bundled(exe) else { return };
    let version = env!("CARGO_PKG_VERSION");
    let stamp = home.join(".fab7/weft/.seeded");
    let seeded = std::fs::read_to_string(&stamp).ok();
    let again = refresh(&updated_by(exe, home), seeded.as_deref().map(str::trim), version);
    for name in ["harnesses", "tools"] {
        let (from, to) = (fab7.join("weft").join(name), home.join(".fab7/weft").join(name));
        if from.is_dir() && (again || !to.exists()) {
            let new = to.with_extension("new");
            let _ = std::fs::remove_dir_all(&new);
            if copy_dir(&from, &new).is_ok() {
                let _ = std::fs::remove_dir_all(&to);
                let _ = std::fs::rename(&new, &to);
            }
        }
    }
    let ringframe = exe.with_file_name("ringframe");
    if (again || !home.join(".fab7/rf/config").exists()) && ringframe.is_file() {
        let _ = std::process::Command::new(ringframe)
            .args(["init", "--global", "--from"])
            .arg(fab7.join("ringframe/config"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    if again {
        let _ = std::fs::write(&stamp, format!("{version}\n"));
    }
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            copy_dir(&path, &to.join(entry.file_name()))?;
        } else {
            std::fs::copy(&path, to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// Before the daemon starts: the home set up from what came with this Weft.
fn set_up_home() {
    if let (Some(home), Ok(exe)) =
        (std::env::var_os("HOME"), std::env::current_exe().and_then(|e| e.canonicalize()))
    {
        seed(std::path::Path::new(&home), &exe);
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let first = args.next();

    // Every hook of every harness: answers the hook, tells the
    // daemon, and exits 0 whatever happens, so no hook fails a harness's turn.
    if first.as_deref() == Some("signal") {
        std::panic::set_hook(Box::new(|_| {}));
        let args: Vec<String> = args.collect();
        let _ = std::panic::catch_unwind(|| {
            let at = weftd::signal::Where::from_env();
            weftd::signal::run(&args, &mut std::io::stdin().lock(), &mut std::io::stdout(), &at)
        });
        std::process::exit(0);
    }
    // Weft from another device: started by the daemon while remote access is
    // on (remote.md §5).
    if first.as_deref() == Some("remote") {
        // Its version is kept while it runs.
        let _held = weftd::versions::hold_own();
        return weft_remote::run();
    }
    // Started by the app to own the panes. One per machine, headless, and it
    // outlives whatever asked for it.
    if first.as_deref() == Some("--serve") {
        set_up_home();
        write_config_if_absent(&weft::routing::file());
        return server::Session::serve(&protocol::socket_path(), &weft::routing::file());
    }
    // The app's way to a daemon: through Weft's service when "Start at login"
    // installed it, so one owner keeps it running; else this process serves.
    if first.as_deref() == Some("--daemon") {
        let socket = protocol::socket_path();
        let service = weftd::service::Service::here();
        if protocol::is_live(&socket)
            || matches!(weftd::service::start_daemon(service.as_ref(), &socket), Ok(true))
        {
            println!("weft: listening on {}", socket.display());
            return Ok(());
        }
        set_up_home();
        write_config_if_absent(&weft::routing::file());
        return server::Session::serve(&socket, &weft::routing::file());
    }

    if first.as_deref() == Some("stop") {
        let socket = protocol::socket_path();
        if protocol::is_live(&socket) {
            protocol::stop(&socket)?;
            println!("weft: stopped the session and its agents");
        } else {
            println!("weft: nothing is running here");
        }
        return Ok(());
    }

    if matches!(first.as_deref(), Some("--version" | "-V")) {
        println!("weft {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if first.as_deref() == Some("update") {
        return update();
    }

    if matches!(first.as_deref(), Some("--help" | "-h")) {
        println!("{}", HELP.replace("VERSION", env!("CARGO_PKG_VERSION")));
        return Ok(());
    }

    // The app, on the project named, or as it was left.
    match first {
        None => open_app(None),
        Some(dir) => match std::path::PathBuf::from(&dir).canonicalize() {
            Ok(root) if root.is_dir() => open_app(Some(&root)),
            _ => anyhow::bail!("{dir} is not a directory; `weft --help` says what weft takes"),
        },
    }
}

const HELP: &str = "weft VERSION

weft [project-dir]     open the Weft app, on that project if one is named
weft update            install the latest Weft and RingFrame
weft stop              end the session and its agents
weft remote            Weft from another device (the session starts it)
weft signal <harness> <hook>   what a harness's hooks run

Agents run in a background session that outlives the app.
Closing Weft leaves them working; `weft stop` ends them.";

/// What `weft` with no arguments opens.
#[derive(Debug, PartialEq, Eq)]
enum Opens {
    /// Weft.app, which this `weft` is inside: opened as an app.
    Bundle(std::path::PathBuf),
    /// The desktop program beside this `weft`.
    Program(std::path::PathBuf),
}

/// The app that came with the `weft` at `exe`, if there is one.
fn app_to_open(exe: &std::path::Path) -> Option<Opens> {
    let dir = exe.parent()?;
    let desktop = dir.join("weft-desktop");
    if !desktop.is_file() {
        return None;
    }
    let bundle = dir.parent().and_then(std::path::Path::parent);
    match bundle {
        Some(b) if dir.ends_with("Contents/MacOS") && b.extension().is_some_and(|e| e == "app") => {
            Some(Opens::Bundle(b.to_path_buf()))
        }
        _ => Some(Opens::Program(desktop)),
    }
}

/// Open the desktop app beside this `weft`, on `project` when one is named,
/// and leave it running. Always a new start: one already running is handed
/// the project by it, and comes in front.
fn open_app(project: Option<&std::path::Path>) -> Result<()> {
    let exe = std::env::current_exe()?.canonicalize()?;
    match app_to_open(&exe) {
        Some(Opens::Bundle(app)) => {
            let mut open = std::process::Command::new("/usr/bin/open");
            open.arg("-n").arg(&app);
            if let Some(p) = project {
                open.arg("--args").arg(p);
            }
            let opened = open.status()?;
            if !opened.success() {
                anyhow::bail!("macOS would not open {}", app.display());
            }
        }
        Some(Opens::Program(desktop)) => {
            // Linux's desktops say where they are; macOS always has one.
            if cfg!(not(target_os = "macos"))
                && std::env::var_os("DISPLAY").is_none()
                && std::env::var_os("WAYLAND_DISPLAY").is_none()
            {
                anyhow::bail!(
                    "weft opens the app on a desktop, and there is no display here; \
                     `weft --help` lists what runs without one"
                );
            }
            start_program(&desktop, project, std::time::Duration::from_secs(1))?;
        }
        None => anyhow::bail!(
            "the desktop app is not installed beside {}; install Weft again",
            exe.display()
        ),
    }
    Ok(())
}

/// Start the desktop program, in its own session so closing this terminal
/// leaves it open; one that fails within `within` says why.
fn start_program(
    desktop: &std::path::Path,
    project: Option<&std::path::Path>,
    within: std::time::Duration,
) -> Result<()> {
    use std::os::unix::process::CommandExt as _;
    let mut program = std::process::Command::new(desktop);
    program.args(project);
    unsafe {
        program.pre_exec(|| {
            unsafe extern "C" {
                fn setsid() -> i32;
            }
            setsid();
            Ok(())
        });
    }
    let mut child = program
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    // An app that cannot start ends at once; say why, rather than nothing.
    let deadline = std::time::Instant::now() + within;
    while std::time::Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            if status.success() {
                break;
            }
            let mut said = String::new();
            if let Some(mut err) = child.stderr.take() {
                let _ = std::io::Read::read_to_string(&mut err, &mut said);
            }
            anyhow::bail!("the app ended as it started ({status}): {}", said.trim());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok(())
}

const REPO: &str = "fab7hq/weft";

/// The latest published release's tag, or nothing when it cannot be reached.
fn latest_release() -> Option<String> {
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "10"])
        .arg(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .output()
        .ok()?;
    let release: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    release["tag_name"].as_str().map(str::to_string)
}

fn is_newer(tag: &str) -> bool {
    let parts = |s: &str| s.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parts(tag.trim_start_matches('v')) > parts(env!("CARGO_PKG_VERSION"))
}

/// A release's tag, and nothing else: `v`, then numbers and dots.
fn is_release_tag(tag: &str) -> bool {
    tag.strip_prefix('v')
        .is_some_and(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

/// What updates the Weft at `exe`.
#[derive(Debug, PartialEq, Eq)]
enum Updater {
    /// Weft's own installer, which keeps each version in `~/.fab7/weft/versions`.
    Installer,
    /// Homebrew, whose cask put Weft.app where it is.
    Homebrew,
}

impl Updater {
    /// What to tell the person instead of updating, when it is not Weft's.
    fn says(&self) -> Option<&'static str> {
        match self {
            Updater::Installer => None,
            Updater::Homebrew => Some("Run: brew upgrade --cask weft"),
        }
    }
}

/// A Weft.app outside `~/.fab7/weft/versions` came from Homebrew; anything
/// else is updated as Weft's installer put it there.
fn updated_by(exe: &std::path::Path, home: &std::path::Path) -> Updater {
    let in_app = exe.parent().is_some_and(|d| d.ends_with("Contents/MacOS"));
    let versions = home.join(".fab7/weft/versions");
    let versions = versions.canonicalize().unwrap_or(versions);
    if in_app && !exe.starts_with(versions) { Updater::Homebrew } else { Updater::Installer }
}

/// Replace this Weft with the latest release, with that release's own
/// installer, pinned to it: never `main`'s, which no release has vouched for.
/// Restarting is the person's. One Homebrew installed is Homebrew's to update.
fn update() -> Result<()> {
    let now = env!("CARGO_PKG_VERSION");
    if let (Some(home), Ok(exe)) =
        (std::env::var_os("HOME"), std::env::current_exe().and_then(|e| e.canonicalize()))
        && let Some(say) = updated_by(&exe, std::path::Path::new(&home)).says()
    {
        println!("{say}");
        return Ok(());
    }
    let Some(tag) = latest_release() else {
        anyhow::bail!("could not reach the latest release");
    };
    if !is_release_tag(&tag) {
        anyhow::bail!("the latest release is not tagged as one: {tag}");
    }
    if !is_newer(&tag) {
        println!("weft {now} is the latest");
        // Installed by hand, or by an earlier update: a service on an older
        // Weft is told to start on this one once no agent runs.
        let socket = protocol::socket_path();
        if protocol::is_live(&socket) {
            let _ = protocol::tell_update(&socket, now);
        }
        return Ok(());
    }
    println!("weft {now} → {tag}");
    // Downloaded whole before it runs, in a folder of the person's own.
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let dir = std::path::PathBuf::from(home).join(".fab7/weft/update");
    std::fs::create_dir_all(&dir)?;
    let script = dir.join("install.sh");
    let url = format!("https://github.com/{REPO}/releases/download/{tag}/install.sh");
    let got = std::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "60", "-o"])
        .arg(&script)
        .arg(&url)
        .status()?;
    if !got.success() {
        let _ = std::fs::remove_dir_all(&dir);
        anyhow::bail!("{tag} publishes no installer");
    }
    let ran = std::process::Command::new("sh").arg(&script).env("WEFT_VERSION", &tag).status();
    let _ = std::fs::remove_dir_all(&dir);
    if !ran?.success() {
        anyhow::bail!("the installer did not finish");
    }
    println!("\n{}", after_update(&protocol::socket_path(), tag.trim_start_matches('v')));
    Ok(())
}

/// Tell a running daemon a newer Weft is installed beside it, which it starts
/// on once no agent runs; what to say to the person.
fn after_update(socket: &std::path::Path, version: &str) -> String {
    if !protocol::is_live(socket) {
        return format!("Weft {version} starts the next time you run weft.");
    }
    match protocol::tell_update(socket, version) {
        Ok(Ok(())) => format!(
            "Weft {version} is installed. It starts when no agent is running; nothing to restart."
        ),
        Ok(Err(why)) => format!("Weft {version} is installed, but Weft's service said: {why}"),
        Err(e) => format!("Weft {version} is installed; Weft's service could not be told ({e})."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Homebrew installed is set up again from each new version: no
    /// installer refreshes it. Weft's own installer refreshes its own.
    #[test]
    fn a_homebrew_weft_sets_its_home_up_again_for_each_version() {
        assert!(refresh(&Updater::Homebrew, None, "0.6.0"), "never set up by this route");
        assert!(refresh(&Updater::Homebrew, Some("0.5.0"), "0.6.0"));
        assert!(!refresh(&Updater::Homebrew, Some("0.6.0"), "0.6.0"));
        assert!(!refresh(&Updater::Installer, Some("0.5.0"), "0.6.0"));
    }

    #[test]
    fn a_weft_homebrew_installed_is_updated_by_homebrew() {
        let home = std::path::Path::new("/Users/p");
        let installed = home.join(".fab7/weft/versions/0.5.0/Weft.app/Contents/MacOS/weft");
        assert_eq!(updated_by(&installed, home), Updater::Installer);
        let linux = home.join(".fab7/weft/versions/0.5.0/weft");
        assert_eq!(updated_by(&linux, home), Updater::Installer);
        let cask = std::path::Path::new("/Applications/Weft.app/Contents/MacOS/weft");
        assert_eq!(updated_by(cask, home), Updater::Homebrew);
        assert_eq!(Updater::Homebrew.says(), Some("Run: brew upgrade --cask weft"));
        assert_eq!(Updater::Installer.says(), None);
    }

    #[test]
    fn only_a_release_tag_is_updated_to() {
        for tag in ["v0.2.0", "v1.10.3"] {
            assert!(is_release_tag(tag), "{tag}");
        }
        for tag in ["0.2.0", "v", "v0.2.0;rm -rf ~", "v0.2.0-rc1", "main", "v0.2.0 "] {
            assert!(!is_release_tag(tag), "{tag:?}");
        }
    }

    #[test]
    fn with_no_arguments_weft_opens_the_app_it_came_with() {
        let dir = std::env::temp_dir().join(format!("weft-opens-{}", std::process::id()));
        // Inside Weft.app, the bundle, so the system shows it as the app.
        let macos = dir.join("Weft.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(macos.join("weft-desktop"), "").unwrap();
        assert_eq!(app_to_open(&macos.join("weft")), Some(Opens::Bundle(dir.join("Weft.app"))));
        // Elsewhere, the desktop program beside it.
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        assert_eq!(app_to_open(&bin.join("weft")), None, "no app beside it");
        std::fs::write(bin.join("weft-desktop"), "").unwrap();
        assert_eq!(app_to_open(&bin.join("weft")), Some(Opens::Program(bin.join("weft-desktop"))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_app_that_ends_as_it_starts_says_why() {
        let dir = std::env::temp_dir().join(format!("weft-starts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let program = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
            path
        };
        let long = std::time::Duration::from_secs(20);
        let failed = start_program(&program("fails", "echo no display >&2; exit 3"), None, long);
        let said = failed.expect_err("it ended").to_string();
        assert!(said.contains("no display"), "{said}");
        // Handed to the app already running, or still running: started.
        start_program(&program("hands", "exit 0"), None, long).expect("handed over");
        let short = std::time::Duration::from_millis(200);
        start_program(&program("runs", "sleep 5"), None, short).expect("running");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_starting_config_is_written_once_and_then_left_alone() {
        let dir = std::env::temp_dir().join(format!("weft-starting-config-{}", std::process::id()));
        let path = dir.join("weft").join("config.toml");
        write_config_if_absent(&path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), weft::routing::STARTING);
        std::fs::write(&path, "[routing]\nask = \"codex\"\n").unwrap();
        write_config_if_absent(&path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[routing]\nask = \"codex\"\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_help_says_what_weft_takes_and_names_no_key() {
        assert!(HELP.contains("open the Weft app"));
        for key in ["press", "[N]", "[X]", "Enter"] {
            assert!(!HELP.contains(key), "{key}");
        }
    }
}
