//! ADR-0024: a tunnel is an adapter, and the adapter is data (the tool file).
//! No provider's name, address or command line is in the code that starts a
//! tunnel or reads its address: the daemon and `weft remote`, outside tests.

use std::path::Path;

/// What would mean a provider had been written into the code.
const PROVIDER: [&str; 4] = ["trycloudflare", "--no-autoupdate", "Cloudflare", "cloudflared"];

fn code(dir: &Path, out: &mut Vec<(String, usize, String)>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            code(&path, out);
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        // Everything from the tests module on is a test.
        let body = text.split("#[cfg(test)]").next().unwrap_or_default();
        for (i, line) in body.lines().enumerate() {
            let line = line.trim_start();
            if line.starts_with("//") {
                continue;
            }
            if PROVIDER.iter().any(|p| line.contains(p)) {
                out.push((path.display().to_string(), i + 1, line.to_string()));
            }
        }
    }
}

#[test]
fn no_tunnel_provider_is_named_in_the_code() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut found = Vec::new();
    for crate_dir in ["crates/weft-remote/src", "crates/weftd/src"] {
        code(&root.join(crate_dir), &mut found);
    }
    assert!(found.is_empty(), "a provider in the code: {found:#?}");
}
