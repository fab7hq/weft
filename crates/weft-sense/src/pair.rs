//! A `pair` head (bundle 3): a small MLP over two embeddings `u` and `v`,
//! `[u, v, u⊙v, |u−v|, x]` → hidden, GELU → one logit, read from safetensors.

use anyhow::{Context, bail};
use serde::Deserialize;
use std::collections::HashMap;

pub struct PairHead {
    dim: usize,
    hidden: usize,
    /// `[hidden, 4 * dim + 1]`, row-major as torch writes `fc1.weight`.
    fc1_weight: Vec<f32>,
    fc1_bias: Vec<f32>,
    fc2_weight: Vec<f32>,
    fc2_bias: f32,
}

#[derive(Deserialize)]
struct TensorInfo {
    dtype: String,
    shape: Vec<usize>,
    data_offsets: [usize; 2],
}

impl PairHead {
    /// From a safetensors file holding `fc1.weight`, `fc1.bias`, `fc2.weight`
    /// and `fc2.bias`, all `F32`.
    pub fn from_safetensors(bytes: &[u8]) -> anyhow::Result<Self> {
        let tensors = read_safetensors(bytes)?;
        let get = |name: &str| tensors.get(name).with_context(|| format!("missing {name}"));
        let (fc1_shape, fc1_weight) = get("fc1.weight")?;
        let [hidden, features] = fc1_shape[..] else { bail!("fc1.weight is not 2-D") };
        if features < 5 || (features - 1) % 4 != 0 {
            bail!("fc1.weight takes {features} features, not 4 × dim + 1");
        }
        let (_, fc1_bias) = get("fc1.bias")?;
        let (_, fc2_weight) = get("fc2.weight")?;
        let (_, fc2_bias) = get("fc2.bias")?;
        if fc1_bias.len() != hidden || fc2_weight.len() != hidden || fc2_bias.len() != 1 {
            bail!("the pair head's tensors do not agree on {hidden} hidden units");
        }
        Ok(Self {
            dim: (features - 1) / 4,
            hidden,
            fc1_weight: fc1_weight.clone(),
            fc1_bias: fc1_bias.clone(),
            fc2_weight: fc2_weight.clone(),
            fc2_bias: fc2_bias[0],
        })
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    /// The logit for `u` and `v`, with `x` the extra feature.
    pub fn logit(&self, u: &[f32], v: &[f32], x: f32) -> f32 {
        let features: Vec<f32> = u
            .iter()
            .chain(v)
            .copied()
            .chain(u.iter().zip(v).map(|(a, b)| a * b))
            .chain(u.iter().zip(v).map(|(a, b)| (a - b).abs()))
            .chain([x])
            .collect();
        let width = features.len();
        let hidden = (0..self.hidden).map(|h| {
            let row = &self.fc1_weight[h * width..(h + 1) * width];
            let pre = row.iter().zip(&features).map(|(w, f)| w * f).sum::<f32>() + self.fc1_bias[h];
            crate::model::gelu(pre)
        });
        hidden.zip(&self.fc2_weight).map(|(a, w)| a * w).sum::<f32>() + self.fc2_bias
    }
}

/// A tensor's shape and values.
type Tensor = (Vec<usize>, Vec<f32>);

/// Every `F32` tensor in a safetensors file, by name.
fn read_safetensors(bytes: &[u8]) -> anyhow::Result<HashMap<String, Tensor>> {
    let len = bytes.get(..8).context("safetensors file too short")?;
    let len = usize::try_from(u64::from_le_bytes(len.try_into()?))?;
    let header = bytes.get(8..8 + len).context("safetensors header past the file")?;
    let data = &bytes[8 + len..];
    let mut infos: HashMap<String, serde_json::Value> =
        serde_json::from_slice(header).context("parse safetensors header")?;
    infos.remove("__metadata__");
    let mut tensors = HashMap::new();
    for (name, info) in infos {
        let info: TensorInfo = serde_json::from_value(info)?;
        if info.dtype != "F32" {
            bail!("{name} is {}, not F32", info.dtype);
        }
        let [start, end] = info.data_offsets;
        let raw = data.get(start..end).with_context(|| format!("{name} past the file"))?;
        let values: Vec<f32> =
            raw.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
        if values.len() != info.shape.iter().product::<usize>() {
            bail!("{name} has {} values for its shape {:?}", values.len(), info.shape);
        }
        tensors.insert(name, (info.shape, values));
    }
    Ok(tensors)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A safetensors file of the given F32 tensors.
    fn safetensors(tensors: &[(&str, &[usize], &[f32])]) -> Vec<u8> {
        let mut header = serde_json::Map::new();
        let mut data = Vec::new();
        for (name, shape, values) in tensors {
            let start = data.len();
            data.extend(values.iter().flat_map(|v| v.to_le_bytes()));
            header.insert(
                name.to_string(),
                serde_json::json!({"dtype": "F32", "shape": shape, "data_offsets": [start, data.len()]}),
            );
        }
        let header = serde_json::to_vec(&header).unwrap();
        let mut out = (header.len() as u64).to_le_bytes().to_vec();
        out.extend(header);
        out.extend(data);
        out
    }

    #[test]
    fn a_pair_logit_is_the_mlp_over_both_vectors_their_product_difference_and_x() {
        // dim 1, two hidden units: features [u, v, u*v, |u-v|, x].
        let bytes = safetensors(&[
            ("fc1.weight", &[2, 5], &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 2.0]),
            ("fc1.bias", &[2], &[0.0, -1.0]),
            ("fc2.weight", &[1, 2], &[1.0, -0.5]),
            ("fc2.bias", &[1], &[0.25]),
        ]);
        let head = PairHead::from_safetensors(&bytes).unwrap();
        assert_eq!(head.dim(), 1);
        let (u, v, x) = (2.0_f32, 3.0_f32, 0.5_f32);
        // Hidden 0 reads u; hidden 1 reads u*v + |u-v| + 2x - 1 = 6 + 1 + 1 - 1 = 7.
        let gelu = |z: f32| 0.5 * z * (1.0 + erf(z / std::f32::consts::SQRT_2));
        let want = gelu(2.0) - 0.5 * gelu(7.0) + 0.25;
        assert!((head.logit(&[u], &[v], x) - want).abs() < 1e-5);
    }

    #[test]
    fn a_pair_head_whose_tensors_disagree_is_refused() {
        let bytes = safetensors(&[
            ("fc1.weight", &[2, 5], &[0.0; 10]),
            ("fc1.bias", &[3], &[0.0; 3]),
            ("fc2.weight", &[1, 2], &[0.0; 2]),
            ("fc2.bias", &[1], &[0.0]),
        ]);
        assert!(PairHead::from_safetensors(&bytes).is_err());
        assert!(PairHead::from_safetensors(&[1, 2, 3]).is_err());
    }

    /// erf by its series, for the expected values only.
    fn erf(x: f32) -> f32 {
        let x = f64::from(x);
        let mut sum = 0.0;
        let mut term = x;
        for n in 0..200 {
            sum += term / f64::from(2 * n + 1);
            term *= -x * x / f64::from(n + 1);
        }
        (sum * 2.0 / std::f64::consts::PI.sqrt()) as f32
    }
}
