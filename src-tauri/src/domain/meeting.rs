//! Meeting notes (`docs/36-meeting-notes-research.md`): a recording becomes
//! who said what when, and that becomes a summary.
//!
//! The rules live here: recognized tokens into words, words to speakers,
//! speakers found by spectral clustering of voice embeddings. The sound, the
//! models and the process they run in are `infra::speech` and
//! `meeting_sidecar`.

use std::collections::BTreeMap;
use std::sync::Arc;

use nalgebra::{DMatrix, SymmetricEigen};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::compaction;
use super::voices::Speech;

/// The first argument that makes the app's exe the recognition process.
pub const SIDECAR_FLAG: &str = "--meeting-sidecar";

/// How long a token lasts when the model gives only where it starts: one GigaAM
/// frame is 40 ms, a token rarely less than two.
const TOKEN_SECONDS: f32 = 0.08;
/// The same speaker again after a pause shorter than this is one utterance.
const JOIN_GAP_SECONDS: f32 = 1.5;
/// One speaker embedding per window this long, every [`EMBED_HOP_SECONDS`],
/// over where the words are; shorter speech gets one window of its own length.
const EMBED_WINDOW_SECONDS: f32 = 1.5;
const EMBED_HOP_SECONDS: f32 = 0.75;
/// The eigengap is looked for among this many speakers at most.
const MAX_SPEAKERS: usize = 10;

#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub start: f32,
    pub end: f32,
    pub text: String,
}

/// A stretch of the recording and whose voice it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub start: f32,
    pub end: f32,
    pub speaker: usize,
}

/// One speaker's words with no long pause between them. Speakers are numbered
/// from 0 by who spoke first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Utterance {
    pub speaker: usize,
    pub start: f32,
    pub end: f32,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Decode,
    Speech,
    Speakers,
    /// The notes of a long meeting's parts, before the notes of the whole.
    Notes,
}

/// A recording recognized.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recognized {
    pub utterances: Vec<Utterance>,
    pub seconds: f32,
    /// Each speaker's mean voice embedding, unit length, by number — what
    /// `domain::voices` knows a person again by. Kept only while it is on.
    #[serde(default)]
    pub voices: Vec<Vec<f32>>,
}

/// What the recognition process writes to stdout, one JSON object a line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SidecarLine {
    Progress { stage: Stage, done: usize, total: usize },
    Result(Recognized),
    Error { message: String },
}

#[derive(Debug, Error, PartialEq)]
pub enum MeetingError {
    #[error("this build has no speech models in it — run `bun scripts/fetch-meeting-models.ts` and build again")]
    ModelsMissing,
    #[error("this recording cannot be read: {0}")]
    Unreadable(String),
    #[error("the recording has no sound in it")]
    Silent,
    #[error("the {0} model did not load")]
    ModelFailed(&'static str),
    #[error("recognition failed: {0}")]
    Recognition(String),
    #[error("no speech was found in the recording")]
    NoSpeech,
    #[error("stopped")]
    Cancelled,
    #[error("a recording is already being processed — wait for it or stop it")]
    Busy,
    #[error("no meeting {0}")]
    NotFound(String),
    #[error("meeting notes: {0}")]
    Store(String),
    /// Even the notes of a long meeting's parts do not fit one request.
    #[error("the meeting is about {tokens} tokens even in notes, more than the model's window takes ({limit}) — pick a model with a larger one")]
    TooLong { tokens: usize, limit: usize },
    #[error("the summary failed: {0}")]
    Summary(String),
    #[error("there is no speaker {0} in this meeting")]
    NoSpeaker(usize),
    #[error("a name is at most {0} characters")]
    NameTooLong(usize),
}

/// A recording made into notes. The sound itself is not kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meeting {
    pub id: String,
    /// The recording's file name.
    pub name: String,
    /// RFC 3339.
    pub created: String,
    pub seconds: f32,
    pub utterances: Vec<Utterance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// What the user called a speaker, by number. Two speakers given one name
    /// are one person the clustering split.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub names: BTreeMap<usize, String>,
    /// The speakers whose name the voice memory gave, not the user.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guessed: Vec<usize>,
}

/// A row of the list: the meeting without its text.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingInfo {
    pub id: String,
    pub name: String,
    pub created: String,
    pub seconds: f32,
    pub speakers: usize,
    pub summarized: bool,
}

/// A speaker's name, past which it is not one.
pub const MAX_NAME_CHARS: usize = 60;

impl Meeting {
    pub fn speakers(&self) -> usize {
        self.utterances.iter().map(|u| u.speaker + 1).max().unwrap_or(0)
    }

    /// How much each speaker said, by number.
    pub fn speech(&self) -> Vec<Speech> {
        let mut out = vec![Speech { seconds: 0.0, words: 0 }; self.speakers()];
        for u in &self.utterances {
            out[u.speaker].seconds += (u.end - u.start).max(0.0);
            out[u.speaker].words += u.text.split_whitespace().count();
        }
        out
    }

    /// Names recognized by voice, for speakers the user has not named.
    pub fn guess(&mut self, names: Vec<(usize, String)>) {
        for (speaker, name) in names {
            if !self.names.contains_key(&speaker) {
                self.names.insert(speaker, name);
                self.guessed.push(speaker);
            }
        }
    }

    /// The user's name for `speaker`, or `Speaker N` from 1 — as the transcript
    /// the model reads has it, and so as the notes name whoever is not named.
    pub fn speaker_name(&self, speaker: usize) -> String {
        self.names.get(&speaker).cloned().unwrap_or_else(|| format!("Speaker {}", speaker + 1))
    }

    /// `speaker` called `name`, or back to its number when `name` is blank.
    /// The notes follow: what they called the speaker becomes the new name,
    /// with no new request to the model.
    pub fn rename(&mut self, speaker: usize, name: &str) -> Result<(), MeetingError> {
        if speaker >= self.speakers() {
            return Err(MeetingError::NoSpeaker(speaker + 1));
        }
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(MeetingError::NameTooLong(MAX_NAME_CHARS));
        }
        let before = self.speaker_name(speaker);
        if name.is_empty() {
            self.names.remove(&speaker);
        } else {
            self.names.insert(speaker, name);
        }
        self.guessed.retain(|g| *g != speaker);
        let after = self.speaker_name(speaker);
        if let Some(summary) = &mut self.summary {
            *summary = replace_word(summary, &before, &after);
        }
        Ok(())
    }

    pub fn info(&self) -> MeetingInfo {
        MeetingInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            created: self.created.clone(),
            seconds: self.seconds,
            speakers: self.speakers(),
            summarized: self.summary.is_some(),
        }
    }
}

/// What processing a meeting reports, each naming the meeting it is about.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum MeetingEvent {
    Progress { id: String, name: String, stage: Stage, done: usize, total: usize },
    /// The transcript is saved: read the meeting again.
    Transcribed { id: String },
    /// A piece of the summary as the model writes it.
    SummaryDelta { id: String, text: String },
}

pub type MeetingEventSink = Arc<dyn Fn(MeetingEvent) + Send + Sync>;

/// What the summarizer is told. The transcript follows as the user's message.
pub const SUMMARY_INSTRUCTIONS: &str = "You write the notes of a meeting from its transcript. The transcript was made by \
speech recognition: words may be misheard and a line may be given to the wrong speaker, so read for what was meant. \
Speakers are numbered; when the conversation shows who someone is (\"Albina, can you…\"), call them by name. \
Anyone not named stays exactly as the transcript has them — \"Speaker 3\", in English whatever the notes' language — \
so the reader can find them in it.

Write Markdown with these sections, the headings in the language of the notes:
## Summary — what the meeting was about and where it ended, in a few sentences.
## Decisions — only what was plainly agreed, one bullet each. A proposal, a \"maybe\" or \"one of the options\" \
is not a decision: it goes to Open questions.
## Action items — who does what, and by when if it was said, one bullet each.
## Open questions — what was raised and left unresolved.

Leave out a section that has nothing in it. Do not invent names, dates or numbers that are not in the transcript. \
Reply with the notes only.";

/// What the instructions add about the language to write in.
pub fn summary_instructions(reply_language: Option<&str>) -> String {
    match reply_language {
        Some(language) => format!("{SUMMARY_INSTRUCTIONS}\n\nWrite the notes in {language}."),
        None => format!("{SUMMARY_INSTRUCTIONS}\n\nWrite the notes in the language the meeting was held in."),
    }
}

/// One line per utterance, `[mm:ss] Name: text` — `Speaker N` for whoever has no name.
pub fn render_transcript(meeting: &Meeting) -> Vec<String> {
    meeting.utterances.iter().map(|u| format!("[{}] {}: {}\n", clock(u.start), meeting.speaker_name(u.speaker), u.text)).collect()
}

/// `notes` written while the speakers were named as in `written`, with each
/// renamed since called as in `now`.
pub fn renamed_since(notes: &str, written: &Meeting, now: &Meeting) -> String {
    (0..now.speakers()).fold(notes.to_string(), |notes, speaker| {
        let (was, is) = (written.speaker_name(speaker), now.speaker_name(speaker));
        if was == is { notes } else { replace_word(&notes, &was, &is) }
    })
}

/// `from` as a whole word in `text` — `Speaker 1` is not inside `Speaker 12`.
fn replace_word(text: &str, from: &str, to: &str) -> String {
    let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(from) {
        let end = at + from.len();
        out.push_str(&rest[..at]);
        let whole = !word(rest[..at].chars().next_back()) && !word(rest[end..].chars().next());
        out.push_str(if whole { to } else { from });
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn clock(seconds: f32) -> String {
    let s = seconds.max(0.0) as u64;
    match s / 3600 {
        0 => format!("{:02}:{:02}", s / 60, s % 60),
        h => format!("{h}:{:02}:{:02}", s / 60 % 60, s % 60),
    }
}

/// The share of the window a request's input may take; the rest is the notes.
const INPUT_PERCENT: u64 = 75;

/// How many tokens of transcript one request may carry.
pub fn budget(context_limit: Option<u32>) -> usize {
    compaction::share_of_window(context_limit, INPUT_PERCENT)
}

/// `text` fits one request, or how far it does not.
pub fn fits(text: &str, context_limit: Option<u32>) -> Result<(), MeetingError> {
    let tokens = compaction::estimate_text_tokens(text);
    let limit = budget(context_limit);
    if tokens > limit {
        return Err(MeetingError::TooLong { tokens, limit });
    }
    Ok(())
}

/// The transcript's lines in parts of at most `budget` tokens, in order. A
/// line longer than that is a part of its own: it cannot be cut, only tried.
pub fn parts(lines: &[String], budget: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut tokens = 0;
    for line in lines {
        let cost = compaction::estimate_text_tokens(line);
        match out.last_mut() {
            Some(part) if tokens + cost <= budget => {
                part.push_str(line);
                tokens += cost;
            }
            _ => {
                out.push(line.clone());
                tokens = cost;
            }
        }
    }
    out
}

/// What the model is told for one part of a meeting too long to read at once.
pub const PART_INSTRUCTIONS: &str = "You take notes on one part of a long meeting's transcript; the notes of every part \
are put together into the meeting's notes afterwards, from yours alone. The transcript was made by speech recognition: \
words may be misheard and a line may be given to the wrong speaker, so read for what was meant.

Keep everything that reader needs, as bullets: what was discussed, what was agreed, who takes on what and by when, \
what was raised and left open. Name people as the transcript does and give the time of each point. Do not invent \
names, dates or numbers. Reply with the notes only.";

/// What comes before the parts' notes when they stand in for the transcript.
pub const PARTS_PREFACE: &str = "The meeting was too long to read at once. Instead of its transcript, here are notes \
of its parts, in order, taken from it. A point raised in one part may be settled in a later one.";

/// SentencePiece tokens joined into words: `▁` starts one — on the token, or
/// as a token of its own before it — and a token of only punctuation stays
/// with the word before it.
pub fn words_of(tokens: &[String], times: &[f32], durations: Option<&[f32]>, offset: f32) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    let mut mark = false;
    for (i, (token, &at)) in tokens.iter().zip(times).enumerate() {
        let starts = mark || token.starts_with('▁') || token.starts_with(' ');
        let piece = token.trim_start_matches(['▁', ' ']);
        if piece.is_empty() {
            mark = true;
            continue;
        }
        mark = false;
        let end = offset + at + durations.and_then(|d| d.get(i)).copied().filter(|d| *d > 0.0).unwrap_or(TOKEN_SECONDS);
        let punctuation = !piece.chars().any(char::is_alphanumeric);
        match words.last_mut() {
            Some(last) if !starts || punctuation => {
                last.text.push_str(piece);
                last.end = end;
            }
            _ => words.push(Word { start: offset + at, end, text: piece.to_string() }),
        }
    }
    words
}

/// Speech is where words are, pauses under half a second closed; each stretch
/// is cut into overlapping windows, one voice embedding each.
pub fn windows_over(words: &[Word]) -> Vec<(f32, f32)> {
    let mut stretches: Vec<(f32, f32)> = Vec::new();
    for w in words {
        match stretches.last_mut() {
            Some(last) if w.start - last.1 < 0.5 => last.1 = last.1.max(w.end),
            _ => stretches.push((w.start, w.end)),
        }
    }
    let mut out = Vec::new();
    for (start, end) in stretches {
        let mut at = start;
        loop {
            let to = (at + EMBED_WINDOW_SECONDS).min(end);
            out.push((at, to));
            if to >= end {
                break;
            }
            at += EMBED_HOP_SECONDS;
        }
    }
    out
}

/// Spectral clustering of embeddings: cosine affinity, each row pruned to its
/// `p` nearest, the speaker count the largest gap among the Laplacian's
/// smallest eigenvalues, k-means on that many eigenvectors. `p` is the one that
/// makes the gap clearest (NME-SC, Park et al. 2019). Labels by first appearance.
// ponytail: dense eigen-decomposition, O(n³) — ~4000 windows an hour is seconds; Lanczos if longer.
pub fn spectral(embeddings: &[Vec<f32>]) -> Vec<usize> {
    let n = embeddings.len();
    if n < 3 {
        return vec![0; n];
    }
    let unit: Vec<Vec<f64>> = embeddings
        .iter()
        .map(|e| {
            let norm = e.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt().max(1e-9);
            e.iter().map(|x| *x as f64 / norm).collect()
        })
        .collect();
    let cosine = DMatrix::from_fn(n, n, |i, j| unit[i].iter().zip(&unit[j]).map(|(a, b)| a * b).sum::<f64>());
    let max_k = MAX_SPEAKERS.min(n - 1);
    let mut best: Option<(f64, usize, DMatrix<f64>)> = None;
    for frac in [0.02, 0.04, 0.06, 0.08, 0.1, 0.15, 0.2, 0.25] {
        let p = ((n as f64 * frac).round() as usize).clamp(2, n - 1);
        let mut pruned = DMatrix::<f64>::zeros(n, n);
        for i in 0..n {
            let mut row: Vec<(usize, f64)> = (0..n).map(|j| (j, cosine[(i, j)])).collect();
            row.sort_by(|a, b| b.1.total_cmp(&a.1));
            for &(j, _) in row.iter().take(p) {
                pruned[(i, j)] = 1.0;
            }
        }
        let affinity = (&pruned + pruned.transpose()) * 0.5;
        let degree = DMatrix::from_diagonal(&affinity.column_sum());
        let eigen: SymmetricEigen<f64, nalgebra::Dyn> = SymmetricEigen::new(degree - &affinity);
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| eigen.eigenvalues[a].total_cmp(&eigen.eigenvalues[b]));
        let values: Vec<f64> = order.iter().map(|&i| eigen.eigenvalues[i]).collect();
        let (k, gap) = (1..=max_k).map(|k| (k, values[k] - values[k - 1])).max_by(|a, b| a.1.total_cmp(&b.1)).unwrap_or((1, 0.0));
        let ratio = (p as f64 / n as f64) / (gap / values[n - 1].max(1e-9) + 1e-10);
        if best.as_ref().is_none_or(|(r, _, _)| ratio < *r) {
            let vectors = DMatrix::from_fn(n, k, |i, c| eigen.eigenvectors[(i, order[c])]);
            best = Some((ratio, k, vectors));
        }
    }
    let Some((_, k, vectors)) = best else { return vec![0; n] };
    first_seen_order(&merge_same_voices(&unit, kmeans(&vectors, k)))
}

/// Each label's mean embedding, unit length, labels from 0 — a label with no
/// window is all zeros.
pub fn centroids(embeddings: &[Vec<f32>], labels: &[usize]) -> Vec<Vec<f32>> {
    let count = labels.iter().max().map_or(0, |m| m + 1);
    let width = embeddings.first().map_or(0, Vec::len);
    (0..count)
        .map(|c| {
            let mut sum = vec![0.0f64; width];
            for (e, _) in embeddings.iter().zip(labels).filter(|(_, l)| **l == c) {
                let norm = e.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt().max(1e-9);
                sum.iter_mut().zip(e).for_each(|(s, x)| *s += *x as f64 / norm);
            }
            let norm = sum.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-9);
            sum.iter().map(|x| (x / norm) as f32).collect()
        })
        .collect()
}

/// Clusters whose mean voices are this alike are one person. The eigengap
/// over-splits a short stretch: on 45 s of three people it found five, two of
/// them 0.79 alike; different people of two real calls were 0.56 at most.
const SAME_VOICE: f64 = 0.7;

/// The two most alike clusters joined, again, while any pair reaches [`SAME_VOICE`].
fn merge_same_voices(unit: &[Vec<f64>], mut labels: Vec<usize>) -> Vec<usize> {
    loop {
        let mut ids: Vec<usize> = labels.clone();
        ids.sort_unstable();
        ids.dedup();
        let centers: Vec<Vec<f64>> = ids
            .iter()
            .map(|&c| {
                let mut v = vec![0.0; unit[0].len()];
                for (i, _) in labels.iter().enumerate().filter(|(_, l)| **l == c) {
                    v.iter_mut().zip(&unit[i]).for_each(|(a, b)| *a += b);
                }
                let norm = v.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-9);
                v.iter().map(|x| x / norm).collect()
            })
            .collect();
        let cosine = |a: usize, b: usize| centers[a].iter().zip(&centers[b]).map(|(x, y)| x * y).sum::<f64>();
        let closest = (0..ids.len())
            .flat_map(|a| (a + 1..ids.len()).map(move |b| (a, b)))
            .max_by(|&(a, b), &(c, d)| cosine(a, b).total_cmp(&cosine(c, d)));
        match closest {
            Some((a, b)) if cosine(a, b) >= SAME_VOICE => labels.iter_mut().filter(|l| **l == ids[b]).for_each(|l| *l = ids[a]),
            _ => return labels,
        }
    }
}

/// K-means on the rows, started from the farthest-apart rows: the same input
/// gives the same labels.
fn kmeans(rows: &DMatrix<f64>, k: usize) -> Vec<usize> {
    let n = rows.nrows();
    let dist = |i: usize, c: &[f64]| (0..rows.ncols()).map(|d| (rows[(i, d)] - c[d]).powi(2)).sum::<f64>();
    let row = |i: usize| (0..rows.ncols()).map(|d| rows[(i, d)]).collect::<Vec<f64>>();
    let mut centers = vec![row(0)];
    while centers.len() < k {
        let far = (0..n)
            .max_by(|&a, &b| {
                let da = centers.iter().map(|c| dist(a, c)).fold(f64::MAX, f64::min);
                let db = centers.iter().map(|c| dist(b, c)).fold(f64::MAX, f64::min);
                da.total_cmp(&db)
            })
            .unwrap_or(0);
        centers.push(row(far));
    }
    let mut labels = vec![0; n];
    for _ in 0..50 {
        let next: Vec<usize> = (0..n)
            .map(|i| (0..k).min_by(|&a, &b| dist(i, &centers[a]).total_cmp(&dist(i, &centers[b]))).unwrap_or(0))
            .collect();
        let settled = next == labels;
        labels = next;
        for (c, center) in centers.iter_mut().enumerate() {
            let members: Vec<usize> = (0..n).filter(|&i| labels[i] == c).collect();
            if !members.is_empty() {
                *center = (0..rows.ncols()).map(|d| members.iter().map(|&i| rows[(i, d)]).sum::<f64>() / members.len() as f64).collect();
            }
        }
        if settled {
            break;
        }
    }
    labels
}

/// Labels renumbered by first appearance, so speaker 0 is whoever spoke first.
fn first_seen_order(labels: &[usize]) -> Vec<usize> {
    let mut seen: Vec<usize> = Vec::new();
    labels
        .iter()
        .map(|l| match seen.iter().position(|s| s == l) {
            Some(i) => i,
            None => {
                seen.push(*l);
                seen.len() - 1
            }
        })
        .collect()
}

/// Each word goes to the speaker whose segment it overlaps most — outside
/// every segment the overlap is minus the gap, so that is the nearest one;
/// consecutive words of one speaker, with no
/// long pause between, are one utterance.
pub fn utterances(words: &[Word], segments: &[Segment]) -> Vec<Utterance> {
    let mut out: Vec<Utterance> = Vec::new();
    for w in words {
        let speaker = speaker_of(w, segments);
        match out.last_mut() {
            Some(last) if last.speaker == speaker && w.start - last.end < JOIN_GAP_SECONDS => {
                last.text.push(' ');
                last.text.push_str(&w.text);
                last.end = w.end;
            }
            _ => out.push(Utterance { speaker, start: w.start, end: w.end, text: w.text.clone() }),
        }
    }
    out
}

fn speaker_of(w: &Word, segments: &[Segment]) -> usize {
    let overlap = |s: &Segment| w.end.min(s.end) - w.start.max(s.start);
    segments.iter().max_by(|a, b| overlap(a).total_cmp(&overlap(b))).map_or(0, |s| s.speaker)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(speaker: usize, start: f32, end: f32) -> Segment {
        Segment { start, end, speaker }
    }

    fn word(start: f32, end: f32, text: &str) -> Word {
        Word { start, end, text: text.into() }
    }

    fn tokens(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn spectral_finds_three_speakers_without_being_told() {
        // Three directions in 8 dimensions, ten noisy windows each, interleaved.
        let mut embeddings = Vec::new();
        for i in 0..30 {
            let mut e = vec![0.0f32; 8];
            e[i % 3] = 1.0;
            e[3 + i % 5] = 0.15;
            embeddings.push(e);
        }
        let labels = spectral(&embeddings);
        assert_eq!(labels.iter().max(), Some(&2));
        assert!((0..30).all(|i| labels[i] == labels[i % 3]), "{labels:?}");
        assert_eq!(&labels[..3], [0, 1, 2], "numbered by who spoke first");
    }

    #[test]
    fn a_speakers_voice_is_the_unit_mean_of_its_windows() {
        let got = centroids(&[vec![2.0, 0.0], vec![0.0, 1.0], vec![0.0, 3.0]], &[0, 0, 2]);
        let s = std::f32::consts::FRAC_1_SQRT_2;
        assert_eq!(got, [vec![s, s], vec![0.0, 0.0], vec![0.0, 1.0]]);
        assert!(centroids(&[], &[]).is_empty());
    }

    #[test]
    fn one_voice_is_one_speaker() {
        // One direction, a little noise on each window: the eigengap alone splits
        // this into several clusters, all alike.
        let embeddings: Vec<Vec<f32>> = (0..40)
            .map(|i| {
                let i = i as f32;
                vec![1.0, 0.05 * (i * 1.7).sin(), 0.05 * (i * 2.3).cos(), 0.05 * (i * 0.9).sin()]
            })
            .collect();
        let labels = spectral(&embeddings);
        assert!(labels.iter().all(|&l| l == 0), "{labels:?}");
        assert_eq!(spectral(&[vec![1.0], vec![0.0]]), [0, 0], "too few windows to tell");
    }

    #[test]
    fn speech_stretches_are_cut_into_overlapping_windows() {
        let words = [word(0.0, 1.0, "а"), word(1.2, 3.0, "б"), word(10.0, 10.4, "в")];
        assert_eq!(windows_over(&words), [(0.0, 1.5), (0.75, 2.25), (1.5, 3.0), (10.0, 10.4)]);
    }

    #[test]
    fn tokens_join_into_words_at_the_word_mark() {
        let got = words_of(&tokens(&["▁При", "вет", ",", "▁мир", "▁..."]), &[0.0, 0.2, 0.4, 1.0, 1.4], None, 10.0);
        assert_eq!(got, [word(10.0, 10.48, "Привет,"), word(11.0, 11.48, "мир...")]);
    }

    #[test]
    fn a_word_mark_of_its_own_starts_the_next_word() {
        let times = [0.0, 0.25, 0.25, 0.5, 1.0, 1.0];
        let got = words_of(&tokens(&["▁У", "▁", "лу", "ком", "▁", "дуб"]), &times, Some(&[0.25; 6]), 0.0);
        assert_eq!(got, [word(0.0, 0.25, "У"), word(0.25, 0.75, "луком"), word(1.0, 1.25, "дуб")]);
    }

    #[test]
    fn a_tokens_duration_is_its_end_when_the_model_gives_one() {
        let got = words_of(&tokens(&["▁да"]), &[1.0], Some(&[0.3]), 0.0);
        assert_eq!(got, [word(1.0, 1.3, "да")]);
    }

    #[test]
    fn a_word_goes_to_the_speaker_it_overlaps_most() {
        let segments = [seg(0, 0.0, 1.2), seg(1, 1.0, 3.0)];
        assert_eq!(speaker_of(&word(0.9, 1.5, "x"), &segments), 1);
        assert_eq!(speaker_of(&word(0.1, 1.1, "x"), &segments), 0);
    }

    #[test]
    fn a_word_outside_every_segment_goes_to_the_nearest() {
        let segments = [seg(0, 0.0, 1.0), seg(1, 5.0, 6.0)];
        assert_eq!(speaker_of(&word(4.0, 4.5, "x"), &segments), 1);
        assert_eq!(speaker_of(&word(1.2, 1.5, "x"), &segments), 0);
        assert_eq!(speaker_of(&word(1.2, 1.5, "x"), &[]), 0);
    }

    #[test]
    fn one_speakers_words_join_until_a_long_pause_or_another_speaker() {
        let segments = [seg(0, 0.0, 10.0), seg(1, 10.0, 12.0)];
        let words = [word(0.0, 0.5, "а"), word(1.0, 1.5, "б"), word(5.0, 5.5, "в"), word(10.5, 11.0, "г")];
        let got: Vec<_> = utterances(&words, &segments).into_iter().map(|u| (u.speaker, u.start, u.end, u.text)).collect();
        assert_eq!(got, [(0, 0.0, 1.5, "а б".to_string()), (0, 5.0, 5.5, "в".to_string()), (1, 10.5, 11.0, "г".to_string())]);
    }

    fn said(speaker: usize, start: f32, text: &str) -> Utterance {
        Utterance { speaker, start, end: start + 1.0, text: text.into() }
    }

    fn meeting_of(utterances: Vec<Utterance>) -> Meeting {
        Meeting {
            id: "m".into(),
            name: "call.wav".into(),
            created: "2026-10-20T10:00:00Z".into(),
            seconds: 60.0,
            utterances,
            summary: None,
            names: BTreeMap::new(),
            guessed: Vec::new(),
        }
    }

    #[test]
    fn the_transcript_is_a_line_per_utterance_with_its_time_and_speaker() {
        let mut meeting = meeting_of(vec![said(0, 5.4, "Привет."), said(1, 65.0, "Да."), said(0, 3725.0, "Итак.")]);
        assert_eq!(
            render_transcript(&meeting).concat(),
            "[00:05] Speaker 1: Привет.\n[01:05] Speaker 2: Да.\n[1:02:05] Speaker 1: Итак.\n"
        );
        meeting.rename(1, "Альбина").unwrap();
        assert_eq!(render_transcript(&meeting)[1], "[01:05] Альбина: Да.\n");
    }

    #[test]
    fn a_rename_reaches_the_notes_and_a_blank_name_takes_it_back() {
        let mut meeting = meeting_of(vec![said(0, 0.0, "а"), said(11, 1.0, "б")]);
        meeting.summary = Some("- Speaker 1 sends it; Speaker 12 checks. Speaker 1's draft.".into());
        meeting.rename(0, "  Женя   К. ").unwrap();
        assert_eq!(meeting.speaker_name(0), "Женя К.");
        assert_eq!(meeting.summary.as_deref(), Some("- Женя К. sends it; Speaker 12 checks. Женя К.'s draft."));
        meeting.rename(0, "Евгений").unwrap();
        assert_eq!(meeting.summary.as_deref(), Some("- Евгений sends it; Speaker 12 checks. Евгений's draft."));
        meeting.rename(0, " ").unwrap();
        assert_eq!(meeting.names, BTreeMap::new());
        assert_eq!(meeting.summary.as_deref(), Some("- Speaker 1 sends it; Speaker 12 checks. Speaker 1's draft."));
    }

    #[test]
    fn notes_written_before_a_rename_take_the_new_name() {
        let written = meeting_of(vec![said(0, 0.0, "а"), said(1, 1.0, "б")]);
        let mut now = written.clone();
        now.rename(1, "Илья").unwrap();
        assert_eq!(renamed_since("Speaker 1 asks Speaker 2.", &written, &now), "Speaker 1 asks Илья.");
        assert_eq!(renamed_since("Speaker 2.", &written, &written), "Speaker 2.");
    }

    #[test]
    fn a_guessed_name_fills_only_an_unnamed_speaker_and_a_rename_makes_it_the_users() {
        let mut meeting = meeting_of(vec![said(0, 0.0, "а"), said(1, 1.0, "б"), said(0, 2.0, "в")]);
        meeting.rename(0, "Оксана").unwrap();
        meeting.guess(vec![(0, "Альбина".into()), (1, "Илья".into())]);
        assert_eq!((meeting.speaker_name(0), meeting.speaker_name(1)), ("Оксана".into(), "Илья".into()));
        assert_eq!(meeting.guessed, [1]);
        meeting.rename(1, "Илья").unwrap();
        assert!(meeting.guessed.is_empty());
        assert_eq!(meeting.speech(), [Speech { seconds: 2.0, words: 2 }, Speech { seconds: 1.0, words: 1 }]);
    }

    #[test]
    fn only_a_speaker_of_the_meeting_is_renamed_and_only_to_a_short_name() {
        let mut meeting = meeting_of(vec![said(0, 0.0, "а"), said(1, 1.0, "б")]);
        assert_eq!(meeting.rename(2, "Илья"), Err(MeetingError::NoSpeaker(3)));
        assert_eq!(meeting.rename(1, &"я".repeat(61)), Err(MeetingError::NameTooLong(60)));
        assert!(meeting.rename(1, &"я".repeat(60)).is_ok());
    }

    #[test]
    fn a_long_transcript_is_cut_between_lines_into_parts_that_fit() {
        // Four characters a token: each line is 2 tokens, one is 5.
        let lines: Vec<String> = ["aaaaaaa\n", "bbbbbbb\n", "ccccccc\n", &"d".repeat(19), "e\n"].map(String::from).to_vec();
        assert_eq!(parts(&lines, 4), ["aaaaaaa\nbbbbbbb\n", "ccccccc\n", &"d".repeat(19), "e\n"]);
        assert_eq!(parts(&lines, 100).len(), 1);
        assert!(parts(&[], 4).is_empty());
    }

    #[test]
    fn the_notes_are_in_the_chosen_language_or_the_meetings() {
        assert!(summary_instructions(Some("Russian")).ends_with("Write the notes in Russian."));
        assert!(summary_instructions(None).ends_with("the language the meeting was held in."));
        assert!(summary_instructions(None).starts_with(SUMMARY_INSTRUCTIONS));
    }

    #[test]
    fn a_text_past_its_share_of_the_window_is_too_long() {
        // 75 % of 1000 tokens, at four characters a token.
        assert_eq!(fits(&"a".repeat(3000), Some(1000)), Ok(()));
        assert_eq!(fits(&"a".repeat(3004), Some(1000)), Err(MeetingError::TooLong { tokens: 751, limit: 750 }));
    }

    #[test]
    fn a_meetings_row_counts_its_speakers_and_says_if_it_has_notes() {
        let meeting = meeting_of(vec![said(0, 0.0, "а"), said(2, 1.0, "б")]);
        let info = meeting.info();
        assert_eq!((info.speakers, info.summarized), (3, false));
        assert_eq!(Meeting { summary: Some("x".into()), utterances: vec![], ..meeting }.info().speakers, 0);
    }

    #[test]
    fn an_event_names_its_kind_and_meeting() {
        let event = MeetingEvent::SummaryDelta { id: "m".into(), text: "x".into() };
        assert_eq!(serde_json::to_string(&event).unwrap(), r#"{"kind":"summaryDelta","id":"m","text":"x"}"#);
    }

    #[test]
    fn a_sidecar_line_reads_back_as_written() {
        let lines = [
            SidecarLine::Progress { stage: Stage::Speech, done: 2, total: 9 },
            SidecarLine::Result(Recognized {
                utterances: vec![Utterance { speaker: 1, start: 0.5, end: 1.0, text: "да".into() }],
                seconds: 3.0,
                voices: vec![vec![0.6, 0.8]],
            }),
            SidecarLine::Error { message: "x".into() },
        ];
        for line in lines {
            let text = serde_json::to_string(&line).unwrap();
            assert_eq!(serde_json::from_str::<SidecarLine>(&text).unwrap(), line, "{text}");
        }
        let wire = r#"{"type":"progress","stage":"speakers","done":0,"total":1}"#;
        assert_eq!(serde_json::from_str::<SidecarLine>(wire).unwrap(), SidecarLine::Progress { stage: Stage::Speakers, done: 0, total: 1 });
    }
}
