//! The transport seam (remote.md §7, ADR-0021): start the tunnel to a
//! loopback port, yield its address, notice it ending, start it again with a
//! backoff. One implementation, TryCloudflare's quick tunnel; nothing else in
//! Weft knows which provider is behind it, and no setting chooses one.

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

/// How long `cloudflared` has to print its address.
const ADDRESS_WITHIN: Duration = Duration::from_secs(30);
const FIRST_WAIT: Duration = Duration::from_secs(2);
const LONGEST_WAIT: Duration = Duration::from_secs(300);

/// Where `cloudflared` is: where `[remote] cloudflared` says, else on `PATH`.
/// Weft never downloads it; missing, the view says how to install it.
pub fn find(configured: Option<&str>) -> Option<PathBuf> {
    let runnable = |p: &Path| {
        use std::os::unix::fs::PermissionsExt;
        p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if let Some(path) = configured {
        return runnable(Path::new(path)).then(|| PathBuf::from(path));
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).map(|d| d.join("cloudflared")).find(|p| runnable(p))
}

/// The first quick-tunnel address in a line of `cloudflared`'s output:
/// `https://[a-z0-9-]+\.trycloudflare\.com`, never its own API's host.
pub fn address(line: &str) -> Option<String> {
    const SUFFIX: &str = ".trycloudflare.com";
    let mut rest = line;
    while let Some(at) = rest.find("https://") {
        let after = &rest[at + "https://".len()..];
        let host_len = after
            .bytes()
            .take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
            .count();
        let host = &after[..host_len];
        if host_len > 0 && after[host_len..].starts_with(SUFFIX) && host != "api" {
            return Some(format!("https://{host}{SUFFIX}"));
        }
        rest = after;
    }
    None
}

pub struct Tunnel {
    program: PathBuf,
    stopped: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Tunnel {
    pub fn new(program: PathBuf) -> Self {
        Tunnel { program, stopped: Arc::default(), child: Arc::default() }
    }

    /// Keep a tunnel to `127.0.0.1:<port>` up until this is dropped.
    pub fn start(&self, port: u16) -> Receiver<TunnelEvent> {
        let (tx, rx) = channel();
        let (program, stopped, slot) =
            (self.program.clone(), self.stopped.clone(), self.child.clone());
        std::thread::spawn(move || {
            let url = format!("http://127.0.0.1:{port}");
            let mut wait = FIRST_WAIT;
            while !stopped.load(Ordering::SeqCst) {
                let reason = run_once(&program, &url, &slot, &stopped, |a| {
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
    }
}

/// One run of `cloudflared`, to its end: why it ended.
fn run_once(
    program: &Path,
    url: &str,
    slot: &Mutex<Option<Child>>,
    stopped: &AtomicBool,
    mut found: impl FnMut(String) -> bool,
) -> String {
    let started = Command::new(program)
        .args(["tunnel", "--no-autoupdate", "--url", url])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match started {
        Ok(c) => c,
        Err(e) => return format!("cloudflared did not start: {e}"),
    };
    let stderr = child.stderr.take().expect("piped");
    *slot.lock().expect("tunnel") = Some(child);
    if stopped.load(Ordering::SeqCst) {
        return "stopped".into();
    }
    // Every line is read to the end, so cloudflared never blocks on a full
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
                if let Some(a) = address(&line) {
                    break if found(a) { None } else { Some("nobody is listening") };
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                break Some("no address within 30 seconds");
            }
            Err(_) => break Some("cloudflared ended before it gave an address"),
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
                        None => format!("cloudflared exited ({status})"),
                    };
                }
                Ok(None) => {}
                Err(e) => {
                    slot.take();
                    return format!("cloudflared was lost: {e}");
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}
