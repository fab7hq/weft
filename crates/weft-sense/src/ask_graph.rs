//! The Ask graph, bundle 3's `ask.int8.onnx`: DeBERTa-v3's encoder with its
//! relative-position attention, int8 as ONNX Runtime runs it after its graph
//! optimizations, and its entailment head read as one logit per row
//! (spec/bundle-3.md §1 and §4). One Ask is one batch: every quantized
//! product's scale is the whole batch's, so an answer holds for its batch.

use crate::model::{OnnxGraph, QuantLinear, erf_f32};
use crate::unigram::AskTokenizer;
use anyhow::bail;

const D: usize = 384;
const HEADS: usize = 6;
const HEAD_DIM: usize = 64;
const FFN: usize = 1536;
/// Relative positions each way: the encoder's position buckets.
const SPAN: usize = 256;
/// The longest row, in tokens.
pub const MAX_ROW: usize = 256;
/// A question row keeps the Ask's last tokens when it must cut it.
const QUESTION_ROW_ASK_LAST: usize = 32;
/// The Ask alone keeps its first and last tokens when it must cut it.
const ALONE_FIRST: usize = MAX_ROW - 2 - ALONE_LAST;
const ALONE_LAST: usize = 64;
const LN_EPS: f32 = 1e-7;
/// √(head width × 3): content to content, content to position and back.
const SCALE: f32 = 13.856_406;

pub struct AskGraph {
    words: Vec<u8>,
    word_zp: u8,
    word_scale: f32,
    emb_norm: Norm,
    layers: Vec<Layer>,
    /// For each distance `i - j`, offset by `MAX_ROW - 1`, the row of the
    /// relative position it attends to.
    positions: Vec<usize>,
    pooler: QuantLinear,
    classifier: QuantLinear,
}

/// What one batch gives: a logit per row, and each row's `[CLS]` vector.
pub struct AskOutput {
    pub logits: Vec<f32>,
    pub embeddings: Vec<Vec<f32>>,
}

struct Layer {
    qkv: QuantLinear,
    /// Per head, the relative positions' query projection, transposed:
    /// `[HEAD_DIM][2 * SPAN]`.
    pos_q: Vec<Vec<f32>>,
    /// The same for the key projection.
    pos_k: Vec<Vec<f32>>,
    attn_out: QuantLinear,
    attn_norm: Norm,
    ffn_in: QuantLinear,
    ffn_out: QuantLinear,
    ffn_norm: Norm,
}

struct Norm {
    weight: Vec<f32>,
    bias: Vec<f32>,
}

/// A padded batch: `rows` rows `width` tokens wide, and which tokens are real.
struct Batch {
    rows: usize,
    width: usize,
    real: Vec<bool>,
}

impl AskGraph {
    pub fn load(model_bytes: &[u8]) -> anyhow::Result<Self> {
        let g = OnnxGraph::read(model_bytes)?;
        let (words, word_zp, word_scale) =
            g.quantized("net.deberta.embeddings.word_embeddings.weight", D)?;
        let emb_norm = Norm::read(&g, "net.deberta.embeddings.LayerNorm")?;
        let mut rel = g.f32s("net.deberta.encoder.rel_embeddings.weight", 2 * SPAN * D)?;
        Norm::read(&g, "net.deberta.encoder.LayerNorm")?.rows(&mut rel);
        let mut layers = Vec::new();
        while g.has_node(&query_node(layers.len())) {
            layers.push(Layer::read(&g, layers.len(), &rel)?);
        }
        if layers.is_empty() {
            bail!("ask.int8.onnx has no encoder layers");
        }
        Ok(Self {
            words,
            word_zp,
            word_scale,
            emb_norm,
            layers,
            positions: relative_positions(),
            pooler: g.qlinear(
                "/net/pooler/dense/Gemm_MatMul_quant",
                "net.pooler.dense.bias",
                D,
                D,
            )?,
            classifier: g.qlinear(
                "/net/classifier/Gemm_MatMul_quant",
                "net.classifier.bias",
                D,
                2,
            )?,
        })
    }

    /// One pass over `rows`, padded to the longest with `[PAD]`.
    pub fn run(&self, rows: &[Vec<u32>]) -> anyhow::Result<AskOutput> {
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        if width == 0 || width > MAX_ROW {
            bail!("an Ask row is {width} tokens, not 1 to {MAX_ROW}");
        }
        let vocab = self.words.len() / D;
        if let Some(id) = rows.iter().flatten().find(|&&id| id as usize >= vocab) {
            bail!("token {id} is past the Ask graph's {vocab} words");
        }
        let real = rows.iter().flat_map(|r| (0..width).map(|p| p < r.len())).collect();
        let batch = Batch { rows: rows.len(), width, real };
        let mut x = self.embed(rows, width);
        for layer in &self.layers {
            x = layer.forward(&x, &batch, &self.positions);
        }
        Ok(self.head(&x, &batch))
    }

    /// Each real token's word, normalized; padding stays zero.
    fn embed(&self, rows: &[Vec<u32>], width: usize) -> Vec<f32> {
        let mut x = vec![0.0_f32; rows.len() * width * D];
        for (r, row) in rows.iter().enumerate() {
            for (p, &id) in row.iter().enumerate() {
                let word = &self.words[id as usize * D..][..D];
                let dst = &mut x[(r * width + p) * D..][..D];
                for (d, &w) in dst.iter_mut().zip(word) {
                    *d = (i32::from(w) - i32::from(self.word_zp)) as f32 * self.word_scale;
                }
                self.emb_norm.rows(dst);
            }
        }
        x
    }

    /// The entailment head on each row's `[CLS]`: yes minus no.
    fn head(&self, x: &[f32], batch: &Batch) -> AskOutput {
        let cls: Vec<f32> =
            (0..batch.rows).flat_map(|r| x[r * batch.width * D..][..D].iter().copied()).collect();
        let mut pooled = linear(&self.pooler, &cls, batch.rows);
        pooled.iter_mut().for_each(|v| *v = gelu(*v));
        let yes_no = linear(&self.classifier, &pooled, batch.rows);
        AskOutput {
            logits: yes_no.chunks_exact(2).map(|c| c[0] - c[1]).collect(),
            embeddings: cls.chunks_exact(D).map(<[f32]>::to_vec).collect(),
        }
    }
}

impl Layer {
    fn read(g: &OnnxGraph, i: usize, rel: &[f32]) -> anyhow::Result<Self> {
        let path = format!("/net/deberta/encoder/layer.{i}");
        let name = format!("net.deberta.encoder.layer.{i}");
        let proj = |p: &str| {
            let node = format!("{path}/attention/self/{p}/MatMul_quant");
            g.qlinear(&node, &format!("{name}.attention.self.{p}.bias"), D, D)
        };
        let (q, k, v) = (proj("query_proj")?, proj("key_proj")?, proj("value_proj")?);
        // The relative positions go through the projections' float weights.
        let float_weight = |p: &str| -> anyhow::Result<Vec<f32>> {
            let prefix = g.weight_of(&format!("{path}/attention/self/{p}/MatMul_quant"))?;
            g.f32s(prefix, D * D)
        };
        let pos_q = by_head(&project(rel, &float_weight("query_proj")?, &q.bias));
        let pos_k = by_head(&project(rel, &float_weight("key_proj")?, &k.bias));
        let bias = [&q.bias[..], &k.bias, &v.bias].concat();
        let qkv = QuantLinear::joined(&[(&q, 1.0), (&k, 1.0), (&v, 1.0)], bias);
        let dense = |node: &str, bias: &str, in_dim, out_dim| {
            g.qlinear(
                &format!("{path}/{node}/MatMul_quant"),
                &format!("{name}.{bias}"),
                in_dim,
                out_dim,
            )
        };
        Ok(Self {
            qkv,
            pos_q,
            pos_k,
            attn_out: dense("attention/output/dense", "attention.output.dense.bias", D, D)?,
            attn_norm: Norm::read(g, &format!("{name}.attention.output.LayerNorm"))?,
            ffn_in: dense("intermediate/dense", "intermediate.dense.bias", D, FFN)?,
            ffn_out: dense("output/dense", "output.dense.bias", FFN, D)?,
            ffn_norm: Norm::read(g, &format!("{name}.output.LayerNorm"))?,
        })
    }

    fn forward(&self, x: &[f32], batch: &Batch, positions: &[usize]) -> Vec<f32> {
        let n = batch.rows * batch.width;
        let qkv = linear(&self.qkv, x, n);
        let ctx = self.attention(&qkv, batch, positions);
        let mut attended = linear(&self.attn_out, &ctx, n);
        attended.iter_mut().zip(x).for_each(|(a, r)| *a += r);
        self.attn_norm.rows(&mut attended);
        let mut inner = linear(&self.ffn_in, &attended, n);
        inner.iter_mut().for_each(|v| *v = gelu(*v));
        let mut out = linear(&self.ffn_out, &inner, n);
        out.iter_mut().zip(&attended).for_each(|(o, r)| *o += r);
        self.ffn_norm.rows(&mut out);
        out
    }

    /// Disentangled attention, every row and head: content to content plus
    /// content to position plus position to content, each over the scale.
    fn attention(&self, qkv: &[f32], batch: &Batch, positions: &[usize]) -> Vec<f32> {
        let w = batch.width;
        let mut ctx = vec![0.0_f32; batch.rows * w * D];
        for r in 0..batch.rows {
            let real = &batch.real[r * w..][..w];
            let rows = &qkv[r * w * 3 * D..][..w * 3 * D];
            for h in 0..HEADS {
                let head = Head { rows, h, w };
                let probs = self.head_probs(&head, real, positions);
                head.context(&probs, &mut ctx[r * w * D..][..w * D]);
            }
        }
        ctx
    }

    /// One head's attention probabilities, `[w][w]`.
    fn head_probs(&self, head: &Head, real: &[bool], positions: &[usize]) -> Vec<f32> {
        let w = head.w;
        let at = |i: usize, j: usize| positions[i + MAX_ROW - 1 - j];
        let keys_t = head.keys_scaled_t();
        let p2c = head.position_to_content(&self.pos_q[head.h], &at);
        let mut probs = vec![0.0_f32; w * w];
        for i in 0..w {
            let q = head.query(i);
            let qk = dot_rows(q, &keys_t, w);
            let first = at(i, w - 1);
            let c2p = dot_rows_range(q, &self.pos_k[head.h], first, at(i, 0) + 1);
            let row = &mut probs[i * w..][..w];
            for (j, s) in row.iter_mut().enumerate() {
                *s = if real[i] && real[j] {
                    qk[j] + (c2p[at(i, j) - first] / SCALE + p2c[j * w + i] / SCALE)
                } else {
                    f32::MIN
                };
            }
            softmax(row);
        }
        probs
    }
}

/// One attention head over one row of the batch.
struct Head<'a> {
    rows: &'a [f32],
    h: usize,
    w: usize,
}

impl Head<'_> {
    fn query(&self, i: usize) -> &[f32] {
        &self.rows[i * 3 * D + self.h * HEAD_DIM..][..HEAD_DIM]
    }

    fn key(&self, j: usize) -> &[f32] {
        &self.rows[j * 3 * D + D + self.h * HEAD_DIM..][..HEAD_DIM]
    }

    fn value(&self, j: usize) -> &[f32] {
        &self.rows[j * 3 * D + 2 * D + self.h * HEAD_DIM..][..HEAD_DIM]
    }

    /// The keys over the scale, transposed: `[HEAD_DIM][w]`.
    fn keys_scaled_t(&self) -> Vec<f32> {
        let mut t = vec![0.0_f32; HEAD_DIM * self.w];
        for j in 0..self.w {
            for (d, &k) in self.key(j).iter().enumerate() {
                t[d * self.w + j] = k / SCALE;
            }
        }
        t
    }

    /// For each key `j` and query `i`, `[j][i]`: the key with the query
    /// projection of their relative position.
    fn position_to_content(&self, pos_q: &[f32], at: &impl Fn(usize, usize) -> usize) -> Vec<f32> {
        let w = self.w;
        let mut out = vec![0.0_f32; w * w];
        for j in 0..w {
            let first = at(0, j);
            let p = dot_rows_range(self.key(j), pos_q, first, at(w - 1, j) + 1);
            for i in 0..w {
                out[j * w + i] = p[at(i, j) - first];
            }
        }
        out
    }

    /// Each query's probabilities times the values, into this head's columns.
    fn context(&self, probs: &[f32], ctx: &mut [f32]) {
        for (i, row) in probs.chunks_exact(self.w).enumerate() {
            let mut acc = [0.0_f32; HEAD_DIM];
            for (j, &p) in row.iter().enumerate() {
                for (a, &v) in acc.iter_mut().zip(self.value(j)) {
                    *a = p.mul_add(v, *a);
                }
            }
            ctx[i * D + self.h * HEAD_DIM..][..HEAD_DIM].copy_from_slice(&acc);
        }
    }
}

impl Norm {
    fn read(g: &OnnxGraph, prefix: &str) -> anyhow::Result<Self> {
        let (weight, bias) =
            g.layer_norm(&format!("{prefix}.weight"), &format!("{prefix}.bias"), D)?;
        Ok(Self { weight, bias })
    }

    /// Normalizes each row of `x` in place.
    fn rows(&self, x: &mut [f32]) {
        for row in x.chunks_exact_mut(D) {
            let (mut sum, mut squares) = (0.0_f32, 0.0_f32);
            for &v in row.iter() {
                sum += v;
                squares += v * v;
            }
            let mean = sum / D as f32;
            let std = (squares / D as f32 - mean * mean + LN_EPS).sqrt();
            for ((v, &w), &b) in row.iter_mut().zip(&self.weight).zip(&self.bias) {
                *v = (*v - mean) / std * w + b;
            }
        }
    }
}

fn query_node(layer: usize) -> String {
    format!("/net/deberta/encoder/layer.{layer}/attention/self/query_proj/MatMul_quant")
}

/// `x`'s rows through a quantized layer, the whole of `x` quantized at once.
fn linear(layer: &QuantLinear, x: &[f32], rows: usize) -> Vec<f32> {
    let (xq, x_scale) = quantize(x);
    let mut out = vec![0.0_f32; rows * layer.out_dim];
    let bias = &layer.bias;
    layer.exact_product(&xq, rows, x_scale, &mut out, |o, exact, scale, j| {
        *o = exact.mul_add(scale, bias[j]);
    });
    out
}

/// Dynamic uint8 quantization of the whole slice, as ONNX Runtime does it:
/// each value's level less the zero point, and the scale.
fn quantize(x: &[f32]) -> (Vec<f32>, f32) {
    let (lo, hi) = x.iter().fold((0.0_f32, 0.0_f32), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    let scale = if hi == lo { 1.0 } else { (hi - lo) / 255.0 };
    let zp = (-lo / scale).clamp(0.0, 255.0).round_ties_even();
    let q = x.iter().map(|&v| (v / scale).round_ties_even().clamp(-zp, 255.0 - zp)).collect();
    (q, scale)
}

fn gelu(v: f32) -> f32 {
    0.5 * v * (erf_f32(v * std::f32::consts::FRAC_1_SQRT_2) + 1.0)
}

fn softmax(row: &mut [f32]) {
    let max = row.iter().copied().fold(f32::MIN, f32::max);
    let mut sum = 0.0_f32;
    for v in row.iter_mut() {
        *v = (*v - max).exp();
        sum += *v;
    }
    let inv = 1.0 / sum;
    row.iter_mut().for_each(|v| *v *= inv);
}

/// `a` with each of `w` columns of `t` (`[a.len()][w]`), summed in order.
fn dot_rows(a: &[f32], t: &[f32], w: usize) -> Vec<f32> {
    let mut acc = vec![0.0_f32; w];
    for (d, &x) in a.iter().enumerate() {
        for (s, &y) in acc.iter_mut().zip(&t[d * w..][..w]) {
            *s = x.mul_add(y, *s);
        }
    }
    acc
}

/// `a` with the columns `from..to` of `t` (`[a.len()][2 * SPAN]`).
fn dot_rows_range(a: &[f32], t: &[f32], from: usize, to: usize) -> Vec<f32> {
    let mut acc = vec![0.0_f32; to - from];
    for (d, &x) in a.iter().enumerate() {
        for (s, &y) in acc.iter_mut().zip(&t[d * 2 * SPAN + from..d * 2 * SPAN + to]) {
            *s = x.mul_add(y, *s);
        }
    }
    acc
}

/// The relative positions through a projection: `rel · weight + bias`.
fn project(rel: &[f32], weight: &[f32], bias: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0_f32; 2 * SPAN * D];
    for (r, row) in rel.chunks_exact(D).enumerate() {
        let acc = dot_rows(row, weight, D);
        for ((o, a), b) in out[r * D..][..D].iter_mut().zip(acc).zip(bias) {
            *o = a + b;
        }
    }
    out
}

/// A projection's columns split by head, each transposed to
/// `[HEAD_DIM][2 * SPAN]`.
fn by_head(proj: &[f32]) -> Vec<Vec<f32>> {
    (0..HEADS)
        .map(|h| {
            let mut t = vec![0.0_f32; HEAD_DIM * 2 * SPAN];
            for (r, row) in proj.chunks_exact(D).enumerate() {
                for d in 0..HEAD_DIM {
                    t[d * 2 * SPAN + r] = row[h * HEAD_DIM + d];
                }
            }
            t
        })
        .collect()
}

/// For each distance from `-(MAX_ROW - 1)` to `MAX_ROW - 1`, the row of its
/// log-bucketed relative position, as the graph computes it in `f32`.
fn relative_positions() -> Vec<usize> {
    (-(MAX_ROW as i32 - 1)..MAX_ROW as i32)
        .map(|d| (bucket(d) + SPAN as i32).clamp(0, 2 * SPAN as i32 - 1) as usize)
        .collect()
}

/// DeBERTa-v3's log bucket of a distance: exact within 128, then
/// logarithmic up to 512.
fn bucket(d: i32) -> i32 {
    const MID: i32 = 128;
    let abs = if d < MID && d > -MID { MID - 1 } else { d.abs() };
    if abs <= MID {
        return d;
    }
    let log = ((abs as f32 / 128.0).ln() / 1.384_339_3_f32 * 127.0).ceil() + 128.0;
    (log * d.signum() as f32) as i32
}

/// One Ask's batch: a row per question, `[CLS] ask [SEP] question [SEP]`,
/// the Ask cut so the row fits and keeping its last tokens; then the Ask
/// alone, cut to its first and last tokens.
pub fn ask_rows<'q>(
    tok: &AskTokenizer,
    text: &str,
    questions: impl Iterator<Item = &'q str>,
) -> anyhow::Result<Vec<Vec<u32>>> {
    let (cls, sep) = (tok.id("[CLS]")?, tok.id("[SEP]")?);
    let ask = tok.encode(text);
    let mut rows = Vec::new();
    for question in questions {
        let q = tok.encode(question);
        let room = MAX_ROW.saturating_sub(q.len() + 3);
        let cut = cut_ask(&ask, room, room.saturating_sub(QUESTION_ROW_ASK_LAST));
        rows.push([&[cls][..], &cut, &[sep], &q, &[sep]].concat());
    }
    rows.push(alone_row(tok, text)?);
    Ok(rows)
}

/// `[CLS] text [SEP]`, cut to its first and last tokens to fit: an Ask
/// alone, or a rule's `when` line.
pub fn alone_row(tok: &AskTokenizer, text: &str) -> anyhow::Result<Vec<u32>> {
    let (cls, sep) = (tok.id("[CLS]")?, tok.id("[SEP]")?);
    let cut = cut_ask(&tok.encode(text), MAX_ROW - 2, ALONE_FIRST);
    Ok([&[cls][..], &cut, &[sep]].concat())
}

/// `ids` if they fit `room`; else their first `first` and as many last as
/// fill the room.
fn cut_ask(ids: &[u32], room: usize, first: usize) -> Vec<u32> {
    if ids.len() <= room {
        return ids.to_vec();
    }
    let last = room - first.min(room);
    [&ids[..first.min(room)], &ids[ids.len() - last..]].concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_distance_is_exact_within_128_and_log_bucketed_beyond() {
        assert_eq!(bucket(0), 0);
        assert_eq!(bucket(-127), -127);
        assert_eq!(bucket(128), 128);
        assert_eq!(bucket(129), 129, "ln(129/128)/ln(511/128)·127 rounds up to 1");
        assert_eq!(bucket(255), 192);
        assert_eq!(bucket(-255), -192);
        let positions = relative_positions();
        assert_eq!(positions[MAX_ROW - 1], SPAN, "distance 0");
    }

    #[test]
    fn an_ask_is_cut_from_its_middle_keeping_its_ends() {
        let ids: Vec<u32> = (0..10).collect();
        assert_eq!(cut_ask(&ids, 10, 2), ids);
        assert_eq!(cut_ask(&ids, 6, 2), [0, 1, 6, 7, 8, 9]);
    }

    #[test]
    fn quantizing_keeps_zero_exact_and_spans_255_levels() {
        let (q, scale) = quantize(&[-1.0, 0.0, 2.0]);
        assert!((scale - 3.0 / 255.0).abs() < 1e-9);
        assert_eq!(q[1], 0.0);
        assert_eq!(q[2] - q[0], 255.0);
    }
}
