//! Okapi BM25, which the Ask sheet matches the person's words to a plan by.

/// Okapi BM25 over a fixed set of documents.
pub(crate) struct Bm25 {
    docs: Vec<Vec<String>>,
    avg: f64,
    /// How many documents hold each word.
    df: std::collections::HashMap<String, usize>,
}

impl Bm25 {
    const K1: f64 = 1.2;
    const B: f64 = 0.75;

    pub(crate) fn new(docs: Vec<Vec<String>>) -> Self {
        let total: usize = docs.iter().map(Vec::len).sum();
        let avg = if docs.is_empty() { 0.0 } else { total as f64 / docs.len() as f64 };
        let mut df = std::collections::HashMap::new();
        for d in &docs {
            let mut seen: Vec<&String> = Vec::new();
            for w in d {
                if !seen.contains(&w) {
                    seen.push(w);
                    *df.entry(w.clone()).or_insert(0) += 1;
                }
            }
        }
        Bm25 { docs, avg, df }
    }

    pub(crate) fn score(&self, doc: usize, query: &[String]) -> f64 {
        let d = &self.docs[doc];
        if d.is_empty() {
            return 0.0;
        }
        let n = self.docs.len() as f64;
        let mut seen: Vec<&String> = Vec::new();
        let mut sum = 0.0;
        for q in query {
            if seen.contains(&q) {
                continue;
            }
            seen.push(q);
            let tf = d.iter().filter(|w| *w == q).count() as f64;
            if tf == 0.0 {
                continue;
            }
            let df = self.df.get(q).copied().unwrap_or(0) as f64;
            let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
            let norm = 1.0 - Self::B + Self::B * d.len() as f64 / self.avg;
            sum += idf * tf * (Self::K1 + 1.0) / (tf + Self::K1 * norm);
        }
        sum
    }
}

/// The lower-case words of a text, for matching.
pub(crate) fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}
