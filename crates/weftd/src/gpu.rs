//! Whether Sense runs on this machine's GPU. The GPU is tried in a child
//! process, as a GPU provider can abort its process: it must give the CPU's
//! answers on the bundle's golden Asks, at a lower p50. The verdict is kept
//! per Weft and Sense version; a failed or crashed try means the CPU.

use std::path::{Path, PathBuf};
use std::time::Duration;

use weft_sense::graph::Provider;
use weft_sense::trial::{Trial, keeps_gpu};

/// How long the child may take before it counts as failed.
const TRY_FOR: Duration = Duration::from_secs(180);

fn file(weft_dir: &Path) -> PathBuf {
    weft_dir.join("sense").join("gpu.json")
}

/// The provider chosen for this Weft and `sense` version, if it was.
pub fn verdict(weft_dir: &Path, sense: &str) -> Option<Provider> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(file(weft_dir)).ok()?).ok()?;
    let ours = v["weft"] == env!("CARGO_PKG_VERSION") && v["sense"] == sense;
    ours.then(|| serde_json::from_value(v["provider"].clone()).ok()).flatten()
}

fn keep(weft_dir: &Path, sense: &str, provider: Provider) {
    let v = serde_json::json!({"weft": env!("CARGO_PKG_VERSION"), "sense": sense,
                               "provider": provider});
    let _ = std::fs::write(file(weft_dir), v.to_string());
}

/// What the child `program` made of the golden Asks of the bundle in `dir`
/// on `gpu`: `program --sense-gpu-check <dir> <gpu>` prints its trial.
pub fn gpu_trial(program: &Path, dir: &Path, gpu: Provider) -> anyhow::Result<Trial> {
    let name = serde_json::to_value(gpu)?.as_str().unwrap_or_default().to_string();
    let mut child = std::process::Command::new(program)
        .args(["--sense-gpu-check".as_ref(), dir.as_os_str(), name.as_ref()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > TRY_FOR {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("the GPU's try took past {TRY_FOR:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    anyhow::ensure!(status.success(), "the GPU's try ended {status}");
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take().expect("piped"), &mut out)?;
    Ok(serde_json::from_str(out.trim())?)
}

/// What the child does: the golden Asks of the bundle in `dir` on the
/// provider named `gpu`, as the trial [`gpu_trial`] reads.
pub fn tried(dir: &Path, gpu: &str) -> anyhow::Result<String> {
    let provider: Provider = serde_json::from_value(serde_json::json!(gpu))?;
    let model = weft_sense::SenseModel::load_on(dir, provider)?;
    Ok(serde_json::to_string(&model.trial(dir)?)?)
}

/// The provider for the bundle in `dir`, whose CPU trial is `cpu`: the GPU
/// `program` tried, when it gave the same answers faster; else the CPU.
pub fn choose(program: &Path, dir: &Path, cpu: &Trial, gpu: Provider) -> Provider {
    match gpu_trial(program, dir, gpu) {
        Ok(tried) if keeps_gpu(cpu, &tried) => gpu,
        _ => Provider::Cpu,
    }
}

/// The verdict for the bundle in `dir`, Sense `version`, run now when there
/// is none: on the CPU unless this build carries a GPU provider that earns
/// its place. `cpu` is the bundle on the CPU.
pub fn settle(
    weft_dir: &Path,
    dir: &Path,
    version: &str,
    cpu: &weft_sense::SenseModel,
    program: &Path,
) -> Provider {
    if let Some(kept) = verdict(weft_dir, version) {
        return kept;
    }
    let provider = match (Provider::gpu(), cpu.trial(dir)) {
        (Some(gpu), Ok(trial)) => choose(program, dir, &trial, gpu),
        _ => Provider::Cpu,
    };
    keep(weft_dir, version, provider);
    provider
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trial(answers: &[&str], p50_ms: f64) -> Trial {
        Trial { answers: answers.iter().map(|a| a.to_string()).collect(), p50_ms }
    }

    /// A child that does what `body` says, as `sh` runs it.
    fn fake(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-weft");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn a_gpu_that_fails_crashes_differs_or_is_slower_leaves_the_cpu() {
        let tmp = tempfile::tempdir().unwrap();
        let cpu = trial(&["a", "b"], 120.0);
        let said = |t: Trial| format!("echo '{}'", serde_json::to_string(&t).unwrap());
        for (why, body) in [
            ("fails", "echo 'no adapter' >&2; exit 1".to_string()),
            ("crashes", "kill -ABRT $$".to_string()),
            ("says nothing", "exit 0".to_string()),
            ("differs", said(trial(&["a", "c"], 60.0))),
            ("is slower", said(trial(&["a", "b"], 300.0))),
        ] {
            let program = fake(tmp.path(), &body);
            let got = choose(&program, tmp.path(), &cpu, Provider::WebGpu);
            assert_eq!(got, Provider::Cpu, "a GPU that {why}");
        }
        let program = fake(tmp.path(), &said(trial(&["a", "b"], 60.0)));
        assert_eq!(choose(&program, tmp.path(), &cpu, Provider::WebGpu), Provider::WebGpu);
    }

    #[test]
    fn the_child_is_asked_for_the_bundle_and_the_gpu() {
        let tmp = tempfile::tempdir().unwrap();
        let seen = tmp.path().join("args");
        let body = format!(
            "echo \"$@\" > '{}'; echo '{}'",
            seen.display(),
            serde_json::to_string(&trial(&["a"], 1.0)).unwrap()
        );
        gpu_trial(&fake(tmp.path(), &body), Path::new("/b"), Provider::WebGpu).unwrap();
        assert_eq!(std::fs::read_to_string(seen).unwrap().trim(), "--sense-gpu-check /b webgpu");
    }

    #[test]
    fn a_verdict_is_kept_per_weft_and_sense_version() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("sense")).unwrap();
        assert_eq!(verdict(tmp.path(), "0.1.0"), None);
        keep(tmp.path(), "0.1.0", Provider::WebGpu);
        assert_eq!(verdict(tmp.path(), "0.1.0"), Some(Provider::WebGpu));
        assert_eq!(verdict(tmp.path(), "0.2.0"), None, "another Sense is tried again");
        let other_weft = serde_json::json!({"weft": "0.0.1", "sense": "0.1.0", "provider": "cpu"});
        std::fs::write(file(tmp.path()), other_weft.to_string()).unwrap();
        assert_eq!(verdict(tmp.path(), "0.1.0"), None, "another Weft is tried again");
    }
}
