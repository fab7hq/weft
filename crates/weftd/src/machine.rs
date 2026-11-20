//! This machine's own name, asked of its system once: the places each system
//! keeps it are weft-core's, and asking them is done here.

use std::sync::OnceLock;

use weft_core::machine::{Source, first, sources};

/// The system's own name for this machine, as Weft shows it: on a Mac its
/// Computer Name, on Linux its pretty name, else its host name; never an
/// address or a bare number. Asked once, then kept while the daemon runs.
pub fn system_name() -> String {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| first(sources(std::env::consts::OS), said)).clone()
}

/// What one place says, when it says anything.
fn said(source: &Source) -> Option<String> {
    match *source {
        Source::Program(program, args) => {
            let out = std::process::Command::new(program)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())?;
            Some(String::from_utf8_lossy(&out.stdout).lines().next()?.to_string())
        }
        Source::FileKey(path, key) => {
            weft_core::machine::file_key(&std::fs::read_to_string(path).ok()?, key)
        }
        Source::File(path) => Some(std::fs::read_to_string(path).ok()?.lines().next()?.into()),
        Source::HostName => host_name(),
    }
}

/// The kernel's host name.
fn host_name() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is writable for its whole length, which is passed.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    let name = std::ffi::CStr::from_bytes_until_nul(&buf).ok().filter(|_| ok)?;
    Some(name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_machine_has_a_name_that_is_not_an_address() {
        let name = system_name();
        assert!(!name.trim().is_empty());
        assert!(name.parse::<std::net::IpAddr>().is_err(), "{name} is an address");
        assert_eq!(system_name(), name, "asked once and kept");
    }
}
