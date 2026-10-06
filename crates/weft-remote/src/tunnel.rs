//! The transport seam (remote.md §7, ADR-0021, ADR-0024): start the tunnel to
//! a loopback port, yield its address, notice it ending, start it again with a
//! backoff. Which program, how it is started and how its address reads are
//! the adapter's (its tool file); nothing here knows a provider.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelEvent {
    /// The tunnel is up at this address.
    Address(String),
    /// It ended, and why; it is started again after a wait.
    Ended(String),
}

/// How long the program has to print its address.
const ADDRESS_WITHIN: Duration = Duration::from_secs(30);
const FIRST_WAIT: Duration = Duration::from_secs(2);
const LONGEST_WAIT: Duration = Duration::from_secs(300);

/// Where the tunnel's program is: where `[remote] program` says, else on
/// `PATH` by its name.
pub fn find(configured: Option<&str>, program: &str) -> Option<PathBuf> {
    let runnable = |p: &Path| {
        use std::os::unix::fs::PermissionsExt;
        p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if let Some(path) = configured {
        return runnable(Path::new(path)).then(|| PathBuf::from(path));
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).map(|d| d.join(program)).find(|p| runnable(p))
}

/// The first address in a line of the program's output: an `https://` URL
/// whose host is lower-case letters, digits and dashes before `suffix`, and is
/// not the provider's own `api` host.
pub fn address(line: &str, suffix: &str) -> Option<String> {
    let mut rest = line;
    while let Some(at) = rest.find("https://") {
        let after = &rest[at + "https://".len()..];
        let host_len = after
            .bytes()
            .take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
            .count();
        let host = &after[..host_len];
        if host_len > 0 && after[host_len..].starts_with(suffix) && host != "api" {
            return Some(format!("https://{host}{suffix}"));
        }
        rest = after;
    }
    None
}

pub struct Tunnel {
    program: PathBuf,
    /// What follows the program; `{port}` is the loopback port.
    run: Vec<String>,
    /// The end of the host its address has.
    suffix: String,
    stopped: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    /// Where the running program's pid is kept, for the next weft remote to
    /// stop it if this one is killed before it can.
    pid: Option<PathBuf>,
}

impl Tunnel {
    pub fn new(program: PathBuf, run: Vec<String>, suffix: String) -> Self {
        Tunnel { program, run, suffix, stopped: Arc::default(), child: Arc::default(), pid: None }
    }

    /// Keep the running program's pid in `file`.
    pub fn keeping_pid(mut self, file: PathBuf) -> Self {
        self.pid = Some(file);
        self
    }

    /// Keep a tunnel to `127.0.0.1:<port>` up until this is dropped.
    pub fn start(&self, port: u16) -> Receiver<TunnelEvent> {
        let (tx, rx) = channel();
        let (program, stopped, slot, pid) =
            (self.program.clone(), self.stopped.clone(), self.child.clone(), self.pid.clone());
        let args: Vec<String> =
            self.run.iter().map(|w| w.replace("{port}", &port.to_string())).collect();
        let suffix = self.suffix.clone();
        std::thread::spawn(move || {
            let mut wait = FIRST_WAIT;
            while !stopped.load(Ordering::SeqCst) {
                let reason =
                    run_once(&program, &args, &suffix, &slot, &stopped, pid.as_deref(), |a| {
                        wait = FIRST_WAIT;
                        tx.send(TunnelEvent::Address(a)).is_ok()
                    });
                if stopped.load(Ordering::SeqCst) || tx.send(TunnelEvent::Ended(reason)).is_err() {
                    break;
                }
                std::thread::sleep(wait);
                wait = (wait * 2).min(LONGEST_WAIT);
            }
        });
        rx
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(mut child) = self.child.lock().ok().and_then(|mut c| c.take()) {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(file) = &self.pid {
            let _ = std::fs::remove_file(file);
        }
    }
}

/// Stop the program a killed weft remote left running, by the pid it kept in
/// `file`, if that pid is still `program` and not a process that took its
/// number since.
pub fn stop_left(file: &Path, program: &Path) {
    let Some(pid) = std::fs::read_to_string(file).ok().and_then(|p| p.trim().parse::<u32>().ok())
    else {
        return;
    };
    let _ = std::fs::remove_file(file);
    let pid = pid.to_string();
    let args = Command::new("ps").args(["-p", &pid, "-o", "args="]).output();
    let name = program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let ours = args.is_ok_and(|o| {
        String::from_utf8_lossy(&o.stdout)
            .split_whitespace()
            .take(2)
            .any(|w| Path::new(w).file_name().is_some_and(|n| n.to_string_lossy() == name))
    });
    if ours && !name.is_empty() {
        let _ = Command::new("kill").arg(&pid).status();
    }
}

/// One run of the program, to its end: why it ended.
fn run_once(
    program: &Path,
    args: &[String],
    suffix: &str,
    slot: &Mutex<Option<Child>>,
    stopped: &AtomicBool,
    pid: Option<&Path>,
    mut found: impl FnMut(String) -> bool,
) -> String {
    let name = program
        .file_name()
        .map_or_else(|| "the tunnel".into(), |n| n.to_string_lossy().into_owned());
    let started = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match started {
        Ok(c) => c,
        Err(e) => return format!("{name} did not start: {e}"),
    };
    let stderr = child.stderr.take().expect("piped");
    if let Some(file) = pid {
        let _ = std::fs::write(file, child.id().to_string());
    }
    *slot.lock().expect("tunnel") = Some(child);
    if stopped.load(Ordering::SeqCst) {
        return "stopped".into();
    }
    // Every line is read to the end, so the program never blocks on a full
    // pipe; the first address is the one.
    let (lines_tx, lines) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = lines_tx.send(line);
        }
    });
    let deadline = std::time::Instant::now() + ADDRESS_WITHIN;
    let failed = loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match lines.recv_timeout(left) {
            Ok(line) => {
                if let Some(a) = address(&line, suffix) {
                    break if found(a) { None } else { Some("nobody is listening") };
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                break Some("no address within 30 seconds");
            }
            Err(_) => break Some("it ended before it gave an address"),
        }
    };
    drop(lines);
    // Up, or given up on: wait for it to end, held where dropping the tunnel
    // can kill it.
    loop {
        {
            let mut slot = slot.lock().expect("tunnel");
            let Some(child) = slot.as_mut() else { return "stopped".into() };
            if failed.is_some() {
                let _ = child.kill();
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    slot.take();
                    return match failed {
                        Some(why) => why.to_string(),
                        None => format!("{name} exited ({status})"),
                    };
                }
                Ok(None) => {}
                Err(e) => {
                    slot.take();
                    return format!("{name} was lost: {e}");
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}
