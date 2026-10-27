//! sense-d's one graph, `sense.int8.onnx`, on ONNX Runtime through `ort`
//! (sense-d spec/bundle.md §1): each question row read as one logit.

use anyhow::{Context, anyhow};
use ort::environment::Environment;
use ort::session::Session;
use ort::session::builder::{AutoDevicePolicy, GraphOptimizationLevel};
use ort::value::TensorRef;
use std::sync::{Mutex, OnceLock};

/// The ONNX Runtime Sense's golden logits are made with: they hold to 1e-4
/// only within one version, so `sense/pyproject.toml` pins the same one.
pub const ONNX_RUNTIME: &str = "1.31";

/// Rows per pass: a call's rows are independent, so a long one runs in
/// passes of this many.
const ROWS_PER_PASS: usize = 64;

pub struct Graph {
    session: Mutex<Session>,
}

impl Graph {
    /// The graph in `model`, on the CPU: every optimization, one thread per
    /// performance core, and each weight's quantization kept apart from the
    /// activations, so a row's logit does not move with its batch mates.
    pub fn load(model: &[u8]) -> anyhow::Result<Self> {
        let session = Session::builder(environment()?)
            .map_err(ort_error)?
            .with_auto_device(AutoDevicePolicy::PreferCPU)
            .map_err(ort_error)?
            .with_optimization_level(GraphOptimizationLevel::All)
            .map_err(ort_error)?
            .with_intra_threads(performance_cores())
            .map_err(ort_error)?
            .with_config_entry("session.disable_quant_qdq", "1")
            .map_err(ort_error)?
            .commit_from_memory(model)
            .map_err(ort_error)
            .context("load sense.int8.onnx")?;
        Ok(Self { session: Mutex::new(session) })
    }

    /// Each row's logit, in passes of [`ROWS_PER_PASS`] rows padded with 0.
    pub fn logits(&self, rows: &[Vec<u32>]) -> anyhow::Result<Vec<f32>> {
        let mut logits = Vec::with_capacity(rows.len());
        for pass in rows.chunks(ROWS_PER_PASS) {
            logits.extend(self.pass(pass)?);
        }
        Ok(logits)
    }

    fn pass(&self, rows: &[Vec<u32>]) -> anyhow::Result<Vec<f32>> {
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        let (mut ids, mut mask): (Vec<i64>, Vec<i64>) = (Vec::new(), Vec::new());
        for row in rows {
            let pad = width - row.len();
            ids.extend(row.iter().map(|&id| i64::from(id)).chain(std::iter::repeat_n(0, pad)));
            mask.extend(std::iter::repeat_n(1, row.len()).chain(std::iter::repeat_n(0, pad)));
        }
        let shape = [rows.len(), width];
        let ids = TensorRef::from_array_view((shape, ids.as_slice())).map_err(ort_error)?;
        let mask = TensorRef::from_array_view((shape, mask.as_slice())).map_err(ort_error)?;
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let inputs = ort::inputs!["input_ids" => ids, "attention_mask" => mask];
        let outputs = session.run(inputs).map_err(ort_error)?;
        let (_, logit) = outputs["logit"].try_extract_tensor::<f32>().map_err(ort_error)?;
        Ok(logit.to_vec())
    }
}

/// The version of the ONNX Runtime linked in, as its build names it.
pub fn onnx_runtime_version() -> Option<&'static str> {
    let info = ort::info();
    let branch = info.split("git-branch=rel-").nth(1)?;
    branch.split(|c: char| c == ',' || c.is_whitespace()).next()
}

/// The one ONNX Runtime environment a process may have.
fn environment() -> anyhow::Result<&'static Environment> {
    static ENVIRONMENT: OnceLock<Environment> = OnceLock::new();
    if let Some(env) = ENVIRONMENT.get() {
        return Ok(env);
    }
    let env = ort::init().with_name("weft-sense").build().map_err(ort_error)?;
    Ok(ENVIRONMENT.get_or_init(|| env))
}

/// The performance cores, where a Mac says; else every core.
fn performance_cores() -> usize {
    #[cfg(target_os = "macos")]
    if let Some(n) = sysctl_u32(c"hw.perflevel0.physicalcpu") {
        return n as usize;
    }
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

#[cfg(target_os = "macos")]
fn sysctl_u32(name: &std::ffi::CStr) -> Option<u32> {
    unsafe extern "C" {
        fn sysctlbyname(
            name: *const std::ffi::c_char,
            oldp: *mut std::ffi::c_void,
            oldlenp: *mut usize,
            newp: *mut std::ffi::c_void,
            newlen: usize,
        ) -> i32;
    }
    let (mut value, mut len) = (0u32, std::mem::size_of::<u32>());
    // SAFETY: `value` is a u32 and `len` says so; nothing is written.
    let found = unsafe {
        sysctlbyname(name.as_ptr(), (&raw mut value).cast(), &mut len, std::ptr::null_mut(), 0)
    };
    (found == 0 && value > 0).then_some(value)
}

fn ort_error<R>(e: ort::Error<R>) -> anyhow::Error {
    anyhow!("ONNX Runtime: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weft_links_the_onnx_runtime_senses_golden_logits_are_made_with() {
        let version = onnx_runtime_version().unwrap_or_else(|| panic!("{}", ort::info()));
        assert!(
            version.starts_with(&format!("{ONNX_RUNTIME}.")),
            "ONNX Runtime {version} is linked, not {ONNX_RUNTIME}"
        );
    }

    #[test]
    fn a_mac_names_its_performance_cores() {
        assert!(performance_cores() >= 1);
    }
}
