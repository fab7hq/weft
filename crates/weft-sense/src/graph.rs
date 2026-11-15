//! Sense's one graph on ONNX Runtime through `ort`: each question row read
//! as one logit, by the base with the adapter a run names, one per job.

use anyhow::{Context, anyhow};
use ort::environment::Environment;
use ort::session::builder::{AutoDevicePolicy, GraphOptimizationLevel};
use ort::session::{Adapter, RunOptions, Session};
use ort::value::TensorRef;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// The ONNX Runtime Sense's golden logits are made with: they hold to 1e-4
/// only within one version, so `sense/pyproject.toml` pins the same one.
pub const ONNX_RUNTIME: &str = "1.31";

/// Rows per pass: a call's rows are independent, so a long one runs in
/// passes of this many.
const ROWS_PER_PASS: usize = 64;

/// Where the graph runs: the CPU, or a GPU this build carries a provider
/// for (WebGPU, on Metal, on macOS).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Cpu,
    WebGpu,
}

impl Provider {
    /// The GPU provider this build carries, if any.
    pub fn gpu() -> Option<Self> {
        cfg!(target_os = "macos").then_some(Self::WebGpu)
    }
}

pub struct Graph {
    session: Mutex<Session>,
    /// Per adapter, the options that switch it on. Each holds its adapter
    /// alive, as every run that names it needs.
    adapters: BTreeMap<String, RunOptions>,
    /// Every row read so far.
    rows_read: AtomicUsize,
}

impl Graph {
    /// The graph in `model` on `provider`, with its `adapters`, each a name
    /// and its file's bytes: every optimization, one thread per performance
    /// core, and each weight's quantization kept apart from the activations,
    /// so a row's logit does not move with its batch mates. A GPU provider
    /// that cannot be had is an error.
    pub fn load_on(
        model: &[u8],
        adapters: &[(String, Vec<u8>)],
        provider: Provider,
    ) -> anyhow::Result<Self> {
        // One graph built at a time in a process, so a reload never builds
        // beside a load still running.
        static BUILDING: Mutex<()> = Mutex::new(());
        let _building = BUILDING.lock().unwrap_or_else(|e| e.into_inner());
        let builder = Session::builder(environment()?)
            .map_err(ort_error)?
            .with_auto_device(AutoDevicePolicy::PreferCPU)
            .map_err(ort_error)?;
        let builder = match provider {
            Provider::Cpu => builder,
            Provider::WebGpu => with_webgpu(builder)?,
        };
        let session = builder
            .with_optimization_level(GraphOptimizationLevel::All)
            .map_err(ort_error)?
            .with_intra_threads(performance_cores())
            .map_err(ort_error)?
            .with_config_entry("session.disable_quant_qdq", "1")
            .map_err(ort_error)?
            .commit_from_memory(model)
            .map_err(ort_error)
            .context("load Sense's graph")?;
        let mut options = BTreeMap::new();
        for (name, bytes) in adapters {
            let adapter = Adapter::from_memory(bytes, None)
                .map_err(ort_error)
                .with_context(|| format!("load the {name} adapter"))?;
            let mut run = RunOptions::new().map_err(ort_error)?;
            run.add_adapter(&adapter).map_err(ort_error)?;
            options.insert(name.clone(), run);
        }
        Ok(Self { session: Mutex::new(session), adapters: options, rows_read: AtomicUsize::new(0) })
    }

    /// Each row's logit read with `adapter`, in passes of
    /// [`ROWS_PER_PASS`] rows padded with 0.
    pub fn logits(&self, rows: &[Vec<u32>], adapter: &str) -> anyhow::Result<Vec<f32>> {
        let options = (self.adapters.get(adapter))
            .with_context(|| format!("Sense has no {adapter} adapter"))?;
        self.rows_read.fetch_add(rows.len(), Ordering::Relaxed);
        let mut logits = Vec::with_capacity(rows.len());
        for pass in rows.chunks(ROWS_PER_PASS) {
            logits.extend(self.pass(pass, options)?);
        }
        Ok(logits)
    }

    /// How many rows it has read.
    pub fn rows_read(&self) -> usize {
        self.rows_read.load(Ordering::Relaxed)
    }

    fn pass(&self, rows: &[Vec<u32>], options: &RunOptions) -> anyhow::Result<Vec<f32>> {
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
        let outputs = session.run_with_options(inputs, options).map_err(ort_error)?;
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

#[cfg(target_os = "macos")]
fn with_webgpu(
    builder: ort::session::builder::SessionBuilder,
) -> anyhow::Result<ort::session::builder::SessionBuilder> {
    let webgpu = ort::ep::WebGPU::default().build().error_on_failure();
    builder.with_execution_providers([webgpu]).map_err(ort_error)
}

#[cfg(not(target_os = "macos"))]
fn with_webgpu(
    _: ort::session::builder::SessionBuilder,
) -> anyhow::Result<ort::session::builder::SessionBuilder> {
    anyhow::bail!("this build of Weft carries no WebGPU")
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

    /// The fixture's base reads `scale · Σ ids + bias`; `state` sets them
    /// to 1 and 0, `ask` to −1 and 0.5 (`tests/fixtures/xs/make.py`).
    fn fixture() -> Graph {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/xs");
        let read = |p: &str| std::fs::read(dir.join(p)).unwrap();
        let adapters =
            ["state", "ask"].map(|a| (a.to_string(), read(&format!("adapters/{a}.onnx_adapter"))));
        Graph::load_on(&read("sense-xs.int8.onnx"), &adapters, Provider::Cpu).unwrap()
    }

    #[test]
    fn each_run_reads_with_the_adapter_it_names_and_switching_back_answers_the_same() {
        let graph = fixture();
        let rows = vec![vec![1, 2, 3], vec![4]];
        assert_eq!(graph.logits(&rows, "state").unwrap(), [6.0, 4.0]);
        assert_eq!(graph.logits(&rows, "ask").unwrap(), [-5.5, -3.5]);
        assert_eq!(graph.logits(&rows, "state").unwrap(), [6.0, 4.0]);
        let err = graph.logits(&rows, "eval").unwrap_err().to_string();
        assert!(err.contains("eval"), "{err}");
    }

    #[test]
    fn a_mac_names_its_performance_cores() {
        assert!(performance_cores() >= 1);
    }
}
