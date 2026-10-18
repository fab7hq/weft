//! The transport seam (remote.md §7): start the tunnel to
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
    /// A line its program said before it gave an address, as it said it.
    Said(String),
}

/// How long the program has to print its address.
pub const ADDRESS_WITHIN: Duration = Duration::from_secs(30);
const FIRST_WAIT: Duration = Duration::from_secs(2);
/// At most this long between starts, so a tunnel is back within it of the
/// network returning.
const LONGEST_WAIT: Duration = Duration::from_secs(30);

/// The wait before each start again: 2, 4, 8 … 30 seconds, and from 2 again
/// once the tunnel gave an address or the network came back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff(Duration);

impl Default for Backoff {
    fn default() -> Self {
        Backoff(FIRST_WAIT)
    }
}

impl Backoff {
    /// The wait now; the next one is twice it, at most the longest.
    pub fn wait(&mut self) -> Duration {
        let now = self.0;
        self.0 = (self.0 * 2).min(LONGEST_WAIT);
        now
    }

    pub fn reset(&mut self) {
        *self = Backoff::default();
    }
}

/// Where the tunnel's program is: where `[remote] program` says, else on
/// `PATH` by its name, else at one of the places its adapter names (`paths`).
pub fn find(configured: Option<&str>, program: &str, paths: &[String]) -> Option<PathBuf> {
    let runnable = |p: &Path| {
        use std::os::unix::fs::PermissionsExt;
        p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if let Some(path) = configured {
        return runnable(Path::new(path)).then(|| PathBuf::from(path));
    }
    let on_path = std::env::var_os("PATH").and_then(|dirs| {
        std::env::split_paths(&dirs).map(|d| d.join(program)).find(|p| runnable(p))
    });
    on_path.or_else(|| paths.iter().map(PathBuf::from).find(|p| runnable(p)))
}

/// The first address in a line of the program's output: an `https://` URL
/// whose host is one or more dot-separated labels of lower-case letters,
/// digits and dashes before `suffix`, and is not the provider's own `api`
/// host.
pub fn address(line: &str, suffix: &str) -> Option<String> {
    let mut rest = line;
    while let Some(at) = rest.find("https://") {
        let after = &rest[at + "https://".len()..];
        let host_len = after
            .bytes()
            .take_while(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'.'
            })
            .count();
        // The host runs into the suffix, which ends it: what comes before it is
        // the labels.
        let ends = |end: usize| {
            after[end..].strip_prefix(suffix).is_some_and(|rest| {
                !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            })
        };
        let found = (1..=host_len).find(|&end| ends(end)).map(|end| &after[..end]);
        if let Some(host) = found
            && host.split('.').all(|label| !label.is_empty())
            && host != "api"
        {
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
    /// Wakes a wait between starts: the network came back.
    poke: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    /// Where the running program's pid is kept, for the next weft remote to
    /// stop it if this one is killed before it can.
    pid: Option<PathBuf>,
}

impl Tunnel {
    pub fn new(program: PathBuf, run: Vec<String>, suffix: String) -> Self {
        Tunnel {
            program,
            run,
            suffix,
            stopped: Arc::default(),
            child: Arc::default(),
            poke: Mutex::default(),
            pid: None,
        }
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
        let (poke, poked) = channel();
        *self.poke.lock().expect("poke") = Some(poke);
        std::thread::spawn(move || {
            let mut backoff = Backoff::default();
            while !stopped.load(Ordering::SeqCst) {
                let reason =
                    run_once(&program, &args, &suffix, &slot, &stopped, pid.as_deref(), |e| {
                        if matches!(e, TunnelEvent::Address(_)) {
                            backoff.reset();
                        }
                        tx.send(e).is_ok()
                    });
                if stopped.load(Ordering::SeqCst) || tx.send(TunnelEvent::Ended(reason)).is_err() {
                    break;
                }
                // Pokes from before it ended are no news.
                while poked.try_recv().is_ok() {}
                match poked.recv_timeout(backoff.wait()) {
                    Ok(()) => backoff.reset(),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        rx
    }
}

impl Tunnel {
    /// The network came back: a tunnel waiting to start again starts now,
    /// and the waits begin again from the first.
    pub fn back(&self) {
        if let Some(poke) = self.poke.lock().ok().as_ref().and_then(|p| p.as_ref()) {
            let _ = poke.send(());
        }
    }

    /// End the running program so it starts again with a new address: one
    /// that runs but that Cloudflare no longer carries is no use to anyone.
    pub fn recycle(&self) {
        if let Some(child) = self.child.lock().ok().as_mut().and_then(|c| c.as_mut()) {
            let _ = child.kill();
        }
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        // A wait between starts ends at once.
        self.poke.lock().ok().map(|mut p| p.take());
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
    mut tell: impl FnMut(TunnelEvent) -> bool,
) -> String {
    let name = program
        .file_name()
        .map_or_else(|| "the tunnel".into(), |n| n.to_string_lossy().into_owned());
    let started = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match started {
        Ok(c) => c,
        Err(e) => return format!("{name} did not start: {e}"),
    };
    // A program may say its address on either: each is read.
    let stdout = child.stdout.take().expect("piped");
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
    let out_tx = lines_tx.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = out_tx.send(line);
        }
    });
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
                    let heard = tell(TunnelEvent::Address(a));
                    break if heard { None } else { Some("nobody is listening") };
                }
                if !line.trim().is_empty() {
                    tell(TunnelEvent::Said(line.trim().to_string()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_between_starts_doubles_to_thirty_seconds_and_resets() {
        let mut b = Backoff::default();
        let waits: Vec<u64> = (0..7).map(|_| b.wait().as_secs()).collect();
        assert_eq!(waits, [2, 4, 8, 16, 30, 30, 30]);
        b.reset();
        assert_eq!(b.wait().as_secs(), 2, "from the first again");
    }
}
