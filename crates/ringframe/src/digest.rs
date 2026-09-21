//! sha256 over bytes, over files, and the reference shape that names one.

use std::io::Read;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn sha256_bytes(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// How the ledger names a published file. `verify` re-checks every one of
/// these, so the four keys are a shape other code matches on.
pub fn artifact_ref(role: &str, rel_path: &str, path: &Path) -> std::io::Result<Value> {
    Ok(json!({
        "role": role,
        "path": rel_path,
        "bytes": path.metadata()?.len(),
        "sha256": sha256_file(path)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_of_bytes_and_file() {
        let dir = crate::testing::tmp_dir();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, b"hello\n").unwrap();
        assert_eq!(
            sha256_bytes(b"hello\n"),
            "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
        );
        assert_eq!(sha256_file(&p).unwrap(), sha256_bytes(b"hello\n"));
        assert_eq!(
            artifact_ref("source_intent", "asks/x/source.txt", &p).unwrap(),
            json!({
                "role": "source_intent",
                "path": "asks/x/source.txt",
                "bytes": 6,
                "sha256": sha256_bytes(b"hello\n"),
            })
        );
    }
}
