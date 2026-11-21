use anyhow::{Context, Result};

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

/// Before the daemon starts: a home this Weft cannot run with gets the newest fab7 release it
/// runs with, or, with nothing installed and no network, the copy that came with it.
fn seed(home: &std::path::Path, exe: &std::path::Path) {
    weftd::fab7::set_up(home, weftd::fab7::bundled_beside(exe).as_deref());
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
        weftd::login_path::adopt();
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
        weftd::login_path::adopt();
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

    // Started by weftd to try Sense on this machine's GPU, which can abort
    // the process that tries it: prints the golden Asks' trial.
    if first.as_deref() == Some("--sense-gpu-check") {
        let mut args = std::env::args().skip(2);
        let dir = std::path::PathBuf::from(args.next().unwrap_or_default());
        println!("{}", weftd::gpu::tried(&dir, &args.next().unwrap_or_default())?);
        return Ok(());
    }

    if first.as_deref() == Some("update") {
        if std::env::args().nth(2).as_deref() == Some("--sense") {
            return update_sense();
        }
        println!("{UPDATE_WEFT}");
        return Ok(());
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
weft update --sense    install the latest Sense model
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

/// Weft is updated the way it was installed, never by itself.
const UPDATE_WEFT: &str = "Weft updates the way you installed it: brew upgrade --cask weft, \
or the install.sh line again. weft update --sense installs the Sense model.";

/// The latest Sense release, downloaded from its tag and checked; a running
/// daemon picks it up when its Updates step ends.
fn update_sense() -> Result<()> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let weft_dir = std::path::PathBuf::from(home).join(".fab7/weft");
    let out = weftd::outside::machine();
    let latest = weftd::outside::Outside::sense_release(&*out, true)
        .context("could not reach Sense's releases")?;
    if !weftd::versions::sense_wanted(&weft_dir, Some(&latest)) {
        println!("Sense {latest} is the latest, and installed");
        return Ok(());
    }
    let current = weftd::versions::fetch_sense(&weft_dir, &*out, &latest)?;
    println!("Sense {} installed", current.version);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Weft says how it is updated, and updates nothing itself.
    #[test]
    fn weft_update_says_how_weft_is_updated() {
        assert!(UPDATE_WEFT.contains("brew upgrade --cask weft"));
        assert!(UPDATE_WEFT.contains("install.sh"));
        assert!(!HELP.contains("install the latest Weft"));
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
