//! `weft-desktop` binary entrypoint.

use std::path::PathBuf;

use weft_desktop::app::WeftDesktopApp;
use weft_desktop::window::run_interactive;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("weft-desktop {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let mut app = WeftDesktopApp::new(home, machine_name());
    for arg in args.iter().filter(|a| !a.starts_with('-')) {
        if let Ok(dir) = PathBuf::from(arg).canonicalize()
            && dir.is_dir()
        {
            app.open_project_path(dir);
        }
    }

    if let Err(err) = run_interactive(app) {
        eprintln!("weft-desktop: {err}");
        std::process::exit(1);
    }
}

/// This machine's name, as the system calls it, without its domain; when
/// the system gives none, a plain word for it.
fn machine_name() -> String {
    unsafe extern "C" {
        fn gethostname(name: *mut std::ffi::c_char, len: usize) -> std::ffi::c_int;
    }
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is writable for its whole length, which is passed.
    let ok = unsafe { gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    let name = ok
        .then(|| std::ffi::CStr::from_bytes_until_nul(&buf).ok())
        .flatten()
        .map(|c| c.to_string_lossy().into_owned())
        .unwrap_or_default();
    weft_desktop::app::machine_name_of(&name)
}
