//! The bundled model's Unigram tokenizer over a flat trie (LZ-1,
//! `docs/54-lazy-index.md`).
//!
//! `tokenizers` keeps a Unigram vocabulary as a trie with a hash map of
//! children in every node: the 2.9 MB of strings of this model's 271 538
//! tokens took ~280 MB resident. Here the trie is three flat arrays — per
//! node a range of edges sorted by byte — and the same vocabulary is ~15 MB.
//!
//! Only the model is ours. Normalization, pre-tokenization and the added
//! `[PAD]`/`[UNK]` stay in `tokenizers`, through its own [`TokenizerImpl`]:
//! this model's normalizer is a charsmap and 34 replacements, and a second
//! copy of them is a second place to disagree. The search itself is HF's
//! `Unigram::encode_optimized` line for line — the same unknown-character
//! penalty, the same fusing of `[UNK]`s, the same tie-breaking — so the ids are
//! the same and so are the vectors: an index built before this needs nothing
//! rebuilt. The tests compare both on one vocabulary.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use tokenizers::models::unigram::UnigramTrainer;
use tokenizers::{
    DecoderWrapper, Model, NormalizerWrapper, PostProcessorWrapper, PreTokenizerWrapper, Token, TokenizerImpl,
};

/// A `tokenizer.json` read with [`CompactUnigram`] as its model.
pub type CompactTokenizer =
    TokenizerImpl<CompactUnigram, NormalizerWrapper, PreTokenizerWrapper, PostProcessorWrapper, DecoderWrapper>;

/// Below the lowest score, per character no token covers — HF's `K_UNK_PENALTY`.
const UNK_PENALTY: f64 = 10.0;
const NO_TOKEN: u32 = u32::MAX;

pub struct CompactUnigram {
    trie: Trie,
    scores: Vec<f64>,
    /// Every token's bytes end to end; token `id` is `text[offsets[id]..offsets[id + 1]]`.
    text: Vec<u8>,
    offsets: Vec<u32>,
    unk_id: u32,
    min_score: f64,
    median_token_bytes: usize,
}

/// Node 0 is the root. A node's edges are `labels[edges[n]..edges[n + 1]]`,
/// sorted, each leading to `targets` at the same index; `token[n]` is the id
/// of the token that ends there, or `NO_TOKEN`.
struct Trie {
    edges: Vec<u32>,
    labels: Vec<u8>,
    targets: Vec<u32>,
    token: Vec<u32>,
}

impl Trie {
    fn child(&self, node: u32, byte: u8) -> Option<u32> {
        let (from, to) = (self.edges[node as usize] as usize, self.edges[node as usize + 1] as usize);
        self.labels[from..to].binary_search(&byte).ok().map(|at| self.targets[from + at])
    }

    fn get(&self, bytes: &[u8]) -> Option<u32> {
        let node = bytes.iter().try_fold(0, |node, &byte| self.child(node, byte))?;
        Some(self.token[node as usize]).filter(|&id| id != NO_TOKEN)
    }
}

/// A `model` this tokenizer would not read the way HF does.
#[derive(Debug, thiserror::Error)]
enum UnreadableModel {
    #[error("a {0} model, not Unigram")]
    NotUnigram(String),
    #[error("a Unigram with byte fallback is not supported")]
    ByteFallback,
    #[error("the vocabulary has no [UNK]")]
    NoUnknown,
    #[error("the vocabulary is over 4 GB")]
    TooLarge,
}

/// The `model` object of a `tokenizer.json`, as much of it as a Unigram needs.
#[derive(Deserialize)]
struct Spec {
    #[serde(rename = "type")]
    kind: String,
    unk_id: Option<u32>,
    vocab: Vec<(String, f64)>,
    #[serde(default)]
    byte_fallback: bool,
}

impl<'de> Deserialize<'de> for CompactUnigram {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        CompactUnigram::new(Spec::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl CompactUnigram {
    fn new(spec: Spec) -> Result<Self, UnreadableModel> {
        if spec.kind != "Unigram" {
            return Err(UnreadableModel::NotUnigram(spec.kind));
        }
        // Spelling an unknown character byte by byte is a path HF takes and
        // this one does not; this model has no such tokens.
        if spec.byte_fallback {
            return Err(UnreadableModel::ByteFallback);
        }
        let count = spec.vocab.len();
        let unk_id = spec.unk_id.filter(|&id| (id as usize) < count).ok_or(UnreadableModel::NoUnknown)?;

        let mut text = Vec::new();
        let mut offsets = Vec::with_capacity(count + 1);
        let mut scores = Vec::with_capacity(count);
        for (token, score) in &spec.vocab {
            offsets.push(u32::try_from(text.len()).map_err(|_| UnreadableModel::TooLarge)?);
            text.extend_from_slice(token.as_bytes());
            scores.push(*score);
        }
        offsets.push(u32::try_from(text.len()).map_err(|_| UnreadableModel::TooLarge)?);
        drop(spec);
        let min_score = scores.iter().copied().fold(f64::INFINITY, f64::min);

        let key = |id: u32| &text[offsets[id as usize] as usize..offsets[id as usize + 1] as usize];
        let mut sorted: Vec<u32> = (0..count as u32).collect();
        sorted.sort_by(|&a, &b| key(a).cmp(key(b)).then(a.cmp(&b)));
        // A string listed twice is the later id, as in HF's `token_to_ids`.
        let mut keys: Vec<u32> = Vec::with_capacity(count);
        for id in sorted {
            match keys.last_mut() {
                Some(last) if key(*last) == key(id) => *last = id,
                _ => keys.push(id),
            }
        }

        let mut lengths: Vec<usize> = keys.iter().map(|&id| key(id).len()).collect();
        lengths.sort_unstable();
        let median_token_bytes = lengths.get(lengths.len() / 2).copied().unwrap_or(1);

        let trie = build_trie(&keys, key);
        Ok(Self { trie, scores, text, offsets, unk_id, min_score, median_token_bytes })
    }

    pub fn unk_id(&self) -> u32 {
        self.unk_id
    }

    /// The median length of a token in bytes, over distinct strings.
    pub fn median_token_bytes(&self) -> usize {
        self.median_token_bytes
    }

    fn token_str(&self, id: u32) -> Option<&str> {
        let (from, to) = (*self.offsets.get(id as usize)?, *self.offsets.get(id as usize + 1)?);
        std::str::from_utf8(&self.text[from as usize..to as usize]).ok()
    }

    /// The best split of `sentence` as `(id, start, end)`, byte offsets.
    /// HF's `encode_optimized`, returning ids where it returns strings.
    fn split(&self, sentence: &str) -> Vec<(u32, usize, usize)> {
        #[derive(Clone, Copy)]
        struct Best {
            id: u32,
            score: f64,
            from: Option<usize>,
        }
        let bytes = sentence.as_bytes();
        let size = bytes.len();
        let unk_score = self.min_score - UNK_PENALTY;
        let mut best = vec![Best { id: 0, score: 0.0, from: None }; size + 1];
        let offer = |best: &mut [Best], end: usize, id: u32, score: f64, start: usize| {
            let target = &mut best[end];
            if target.from.is_none() || score > target.score {
                *target = Best { id, score, from: Some(start) };
            }
        };

        let mut start = 0;
        while start < size {
            let Some(first) = sentence[start..].chars().next() else { break };
            let char_len = first.len_utf8();
            let here = best[start].score;
            let mut covers_one_char = false;
            let mut node = 0;
            for (taken, &byte) in bytes[start..].iter().enumerate() {
                let Some(next) = self.trie.child(node, byte) else { break };
                node = next;
                let id = self.trie.token[node as usize];
                if id == NO_TOKEN {
                    continue;
                }
                let len = taken + 1;
                offer(&mut best, start + len, id, here + self.scores[id as usize], start);
                covers_one_char |= len == char_len;
            }
            if !covers_one_char {
                offer(&mut best, start + char_len, self.unk_id, here + unk_score, start);
            }
            start += char_len;
        }

        // Back from the end. A run of `[UNK]`s is one token, looked up as a
        // whole — what HF's `fuse_unk` does before mapping strings to ids.
        let mut pieces = Vec::new();
        let mut unknown_until = None;
        let mut end = size;
        while end > 0 {
            let node = best[end];
            let Some(start) = node.from else { break };
            if node.id == self.unk_id {
                unknown_until.get_or_insert(end);
            } else {
                if let Some(until) = unknown_until.take() {
                    pieces.push((self.fused(&bytes[end..until]), end, until));
                }
                pieces.push((node.id, start, end));
            }
            end = start;
        }
        if let Some(until) = unknown_until {
            pieces.push((self.fused(&bytes[..until]), 0, until));
        }
        pieces.reverse();
        pieces
    }

    fn fused(&self, bytes: &[u8]) -> u32 {
        self.trie.get(bytes).unwrap_or(self.unk_id)
    }
}

/// The trie of `keys` — sorted, distinct — breadth first: the keys under a
/// node at depth `d` are one run of `keys`, its children the runs that share
/// byte `d`.
fn build_trie<'a>(keys: &[u32], key: impl Fn(u32) -> &'a [u8]) -> Trie {
    let mut trie = Trie { edges: Vec::new(), labels: Vec::new(), targets: Vec::new(), token: Vec::new() };
    let mut nodes: Vec<(usize, usize, usize)> = vec![(0, keys.len(), 0)];
    let mut next = 0;
    while let Some(&(mut from, to, depth)) = nodes.get(next) {
        trie.edges.push(trie.labels.len() as u32);
        // The key that ends here sorts first among the keys under the node.
        if from < to && key(keys[from]).len() == depth {
            trie.token.push(keys[from]);
            from += 1;
        } else {
            trie.token.push(NO_TOKEN);
        }
        while from < to {
            let byte = key(keys[from])[depth];
            let run = keys[from..to].iter().take_while(|&&id| key(id)[depth] == byte).count();
            trie.labels.push(byte);
            trie.targets.push(nodes.len() as u32);
            nodes.push((from, from + run, depth + 1));
            from += run;
        }
        next += 1;
    }
    trie.edges.push(trie.labels.len() as u32);
    for list in [&mut trie.edges, &mut trie.targets, &mut trie.token] {
        list.shrink_to_fit();
    }
    trie.labels.shrink_to_fit();
    trie
}

impl Model for CompactUnigram {
    type Trainer = UnigramTrainer;

    fn tokenize(&self, sentence: &str) -> tokenizers::Result<Vec<Token>> {
        Ok(self
            .split(sentence)
            .into_iter()
            .map(|(id, start, end)| Token::new(id, sentence[start..end].to_string(), (start, end)))
            .collect())
    }

    fn token_to_id(&self, token: &str) -> Option<u32> {
        self.trie.get(token.as_bytes())
    }

    fn id_to_token(&self, id: u32) -> Option<String> {
        self.token_str(id).map(str::to_string)
    }

    /// Built on demand; nothing on the embedding path asks for it.
    fn get_vocab(&self) -> HashMap<String, u32> {
        // Ascending, so a string listed twice keeps its later id.
        (0..self.scores.len() as u32).filter_map(|id| Some((self.token_str(id)?.to_string(), id))).collect()
    }

    fn get_vocab_size(&self) -> usize {
        self.scores.len()
    }

    fn save(&self, _folder: &Path, _prefix: Option<&str>) -> tokenizers::Result<Vec<PathBuf>> {
        Err("the bundled tokenizer is read, never written".into())
    }

    fn get_trainer(&self) -> UnigramTrainer {
        UnigramTrainer::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokenizers::Tokenizer;

    /// A small `tokenizer.json` shaped like the bundled one: added `[PAD]` and
    /// `[UNK]`, a normalizer that spaces out punctuation, Metaspace. The
    /// vocabulary is chosen so the best split is not the greedy one, two
    /// splits tie, a string is listed twice, and a letter takes two bytes.
    fn spec(model: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "version": "1.0", "truncation": null, "padding": null,
            "added_tokens": [
                {"id": 0, "content": "[PAD]", "single_word": false, "lstrip": false, "rstrip": false, "normalized": false, "special": true},
                {"id": 1, "content": "[UNK]", "single_word": false, "lstrip": false, "rstrip": false, "normalized": false, "special": true}
            ],
            "normalizer": {"type": "Sequence", "normalizers": [
                {"type": "Replace", "pattern": {"String": "."}, "content": " . "},
                {"type": "Replace", "pattern": {"Regex": "\\s+"}, "content": " "},
                {"type": "Strip", "strip_left": true, "strip_right": true}
            ]},
            "pre_tokenizer": {"type": "Metaspace", "replacement": "▁", "prepend_scheme": "always", "split": false},
            "post_processor": null,
            "decoder": null,
            "model": model
        }))
        .unwrap()
    }

    fn vocab() -> serde_json::Value {
        serde_json::json!([
            ["[PAD]", -20.0], ["[UNK]", -20.0],
            ["▁", -3.0], ["a", -2.0], ["b", -2.0], ["ab", -3.0], ["▁ab", -2.5], ["abc", -9.0], ["c", -2.0],
            ["x", -1.0], ["y", -1.0], ["xy", -2.0], ["▁x", -1.5],
            [".", -1.0], ["▁.", -1.2],
            ["д", -2.0], ["до", -2.5], ["м", -2.0], ["▁дом", -3.0],
            ["a", -0.5],
            ["qq", -15.0],
            ["▁дом", -3.0], ["▁дом", -3.0], ["▁дом", -3.0],
            ["zk", -25.0], ["v", -10.0], ["kv", -3.0]
        ])
    }

    fn unigram() -> serde_json::Value {
        serde_json::json!({"type": "Unigram", "unk_id": 1, "vocab": vocab(), "byte_fallback": false})
    }

    fn both() -> (Tokenizer, CompactTokenizer) {
        let json = spec(unigram());
        (Tokenizer::from_bytes(&json).unwrap(), serde_json::from_slice(&json).unwrap())
    }

    const TEXTS: &[&str] = &[
        "ab", "abc", "a b c", "xy", "x y", "xyxy", "a.b", "...", "дом", "до м", "дома", "zz", "z", "q ab q",
        "qq", "qqq", "zkv", "kzkv", "🦀ab🦀", "[UNK] ab [PAD]", "ab[PAD]ab", "", "   ", "\r\n ab\t", "ёж", "a  .  b",
    ];

    /// The point of the module: on every text, the ids HF's own Unigram gives.
    #[test]
    fn ids_are_hfs_on_every_text() {
        let (hf, compact) = both();
        for text in TEXTS {
            let want = hf.encode(*text, false).unwrap();
            let got = compact.encode(*text, false).unwrap();
            assert_eq!(got.get_ids(), want.get_ids(), "{text:?}: {:?}", want.get_tokens());
        }
        let batch: Vec<String> = TEXTS.iter().map(|t| t.to_string()).collect();
        let want = hf.encode_batch_fast(batch.clone(), false).unwrap();
        let got = compact.encode_batch_fast(batch, false).unwrap();
        assert!(want.iter().zip(&got).all(|(w, g)| w.get_ids() == g.get_ids()));
    }

    fn ids(text: &str) -> Vec<u32> {
        both().1.encode(text, false).unwrap().get_ids().to_vec()
    }

    /// What the comparison above stands on, said without HF: the best path is
    /// taken over the greedy one, a tie keeps the split found first, and
    /// characters no token covers are one `[UNK]`.
    #[test]
    fn the_split_is_the_best_scored_one() {
        // `▁ab` (-2.5) over `▁` + `ab` (-6.0) and over `▁` + `a` + `b`.
        assert_eq!(ids("ab"), [6]);
        // After `▁a`, `xy` (-2.0) ties `x` + `y`: the split found first stays.
        assert_eq!(ids("axy"), [2, 19, 11]);
        assert_eq!(ids("xyxy"), [12, 10, 11]);
        // `z` and `z` have no token: one `[UNK]` for the run, not two.
        assert_eq!(ids("zz"), [2, 1]);
        // `zk` + `v` (-35) against an unknown `z` + `kv`: the unknown costs
        // the lowest score (-25, not the first) less 10, so -38 loses.
        assert_eq!(ids("zkv"), [2, 24, 25]);
    }

    /// A string listed twice is its later id — and scores as that id.
    #[test]
    fn a_string_listed_twice_is_its_later_id() {
        let (_, compact) = both();
        assert_eq!(compact.token_to_id("a"), Some(19));
        assert_eq!(compact.get_model().get_vocab()["a"], 19);
        // After `▁x`, `a` at its later score (-0.5) + `b` beats `ab` (-3.0);
        // at the earlier one (-2.0) it would not.
        assert_eq!(ids("xab"), [12, 19, 4]);
    }

    #[test]
    fn the_model_answers_what_the_pipeline_asks() {
        let (_, compact) = both();
        let model = compact.get_model();
        assert_eq!(model.get_vocab_size(), 27);
        assert_eq!(model.unk_id(), 1);
        assert_eq!(model.id_to_token(16).as_deref(), Some("до"));
        assert_eq!(model.id_to_token(23).as_deref(), Some("▁дом"));
        assert_eq!(model.id_to_token(27), None);
        assert_eq!(model.token_to_id("▁do"), None);
        assert_eq!(model.token_to_id("▁дом"), Some(23));
    }

    fn model(vocab: serde_json::Value) -> CompactUnigram {
        serde_json::from_value(serde_json::json!({"type": "Unigram", "unk_id": 0, "vocab": vocab})).unwrap()
    }

    /// The middle of the distinct strings' lengths in bytes, as HF's
    /// vocabulary map has them: [1, 1, 5, 9] → 5. Counting the repeats would
    /// give [1, 1, 5, 9, 9, 9] → 9.
    #[test]
    fn the_median_is_over_distinct_strings() {
        let vocab = serde_json::json!([["[UNK]", -5.0], ["a", -1.0], ["b", -1.0], ["ccccccccc", -2.0], ["ccccccccc", -2.0], ["ccccccccc", -2.0]]);
        assert_eq!(model(vocab).median_token_bytes(), 5);
    }

    /// A run of `[UNK]`s is looked up whole, as HF's `fuse_unk` does: here
    /// the vocabulary's own `[UNK]` string twice is a token of its own.
    /// Reached only below the pipeline — through it, `[UNK]` in a text is
    /// taken out as an added token first — so it is compared on the models.
    #[test]
    fn a_fused_run_that_is_a_token_is_that_token() {
        let pairs = vec![("[UNK]".to_string(), -1.0), ("[UNK][UNK]".to_string(), -50.0), ("a".to_string(), -1.0)];
        let hf = tokenizers::models::unigram::Unigram::from(pairs, Some(0), false).unwrap();
        let ours = model(serde_json::json!([["[UNK]", -1.0], ["[UNK][UNK]", -50.0], ["a", -1.0]]));
        let ids = |tokens: Vec<Token>| tokens.into_iter().map(|t| t.id).collect::<Vec<_>>();
        for text in ["[UNK][UNK]", "a[UNK][UNK]", "[UNK][UNK]a", "[UNK]", "x[UNK]x", "[UNK]x[UNK]", "a[UNK]a"] {
            assert_eq!(ids(ours.tokenize(text).unwrap()), ids(hf.tokenize(text).unwrap()), "{text:?}");
        }
        // At the start of the text and after a token: both ends of the walk back.
        assert_eq!(ids(ours.tokenize("[UNK][UNK]").unwrap()), [1]);
        assert_eq!(ids(ours.tokenize("a[UNK][UNK]").unwrap()), [2, 1]);
    }

    #[test]
    fn a_model_it_cannot_read_the_same_way_is_refused() {
        let refused = |model: serde_json::Value| serde_json::from_slice::<CompactTokenizer>(&spec(model)).is_err();
        assert!(refused(serde_json::json!({"type": "Unigram", "unk_id": 1, "vocab": vocab(), "byte_fallback": true})));
        assert!(refused(serde_json::json!({"type": "Unigram", "unk_id": 99, "vocab": vocab()})));
        assert!(refused(serde_json::json!({"type": "Unigram", "unk_id": null, "vocab": vocab()})));
        assert!(refused(serde_json::json!({"type": "BPE", "unk_id": 1, "vocab": vocab()})));
        assert!(!refused(unigram()));
    }
}
