//! Meeting notes (`docs/36-meeting-notes-research.md`): a recording becomes
//! who said what when, and that becomes a summary.
//!
//! The rules live here: recognized tokens into words, words to speakers,
//! speakers found by spectral clustering of voice embeddings. The sound, the
//! models and the process they run in are `infra::speech` and
//! `meeting_sidecar`.

use std::sync::Arc;

use nalgebra::{DMatrix, SymmetricEigen};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::compaction;

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
}

/// What the recognition process writes to stdout, one JSON object a line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SidecarLine {
    Progress { stage: Stage, done: usize, total: usize },
    Result { utterances: Vec<Utterance>, seconds: f32 },
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
    // ponytail: one request for the whole transcript; summarize it in pieces when a call outgrows the model.
    #[error("the transcript is about {tokens} tokens, more than the model's window takes ({limit}) — pick a model with a larger one")]
    TooLong { tokens: usize, limit: usize },
    #[error("the summary failed: {0}")]
    Summary(String),
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

impl Meeting {
    pub fn info(&self) -> MeetingInfo {
        MeetingInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            created: self.created.clone(),
            seconds: self.seconds,
            speakers: self.utterances.iter().map(|u| u.speaker + 1).max().unwrap_or(0),
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
Speakers are numbered; when the conversation shows who someone is (\"Albina, can you…\"), call them by name.

Write Markdown with these sections, the headings in the language of the notes:
## Summary — what the meeting was about and where it ended, in a few sentences.
## Decisions — what was agreed, one bullet each.
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

/// One line per utterance, `[mm:ss] Speaker N: text`, speakers from 1.
pub fn render_transcript(utterances: &[Utterance]) -> String {
    utterances.iter().map(|u| format!("[{}] Speaker {}: {}\n", clock(u.start), u.speaker + 1, u.text)).collect()
}

fn clock(seconds: f32) -> String {
    let s = seconds.max(0.0) as u64;
    match s / 3600 {
        0 => format!("{:02}:{:02}", s / 60, s % 60),
        h => format!("{h}:{:02}:{:02}", s / 60 % 60, s % 60),
    }
}

/// The share of the window the transcript may take; the rest is the notes.
const TRANSCRIPT_PERCENT: u64 = 75;

/// `transcript` fits the model's window, or how far it does not.
pub fn fits(transcript: &str, context_limit: Option<u32>) -> Result<(), MeetingError> {
    let tokens = compaction::estimate_text_tokens(transcript);
    let limit = compaction::share_of_window(context_limit, TRANSCRIPT_PERCENT);
    if tokens > limit {
        return Err(MeetingError::TooLong { tokens, limit });
    }
    Ok(())
}

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

    #[test]
    fn the_transcript_is_a_line_per_utterance_with_its_time_and_speaker_from_one() {
        let got = render_transcript(&[said(0, 5.4, "Привет."), said(1, 65.0, "Да."), said(0, 3725.0, "Итак.")]);
        assert_eq!(got, "[00:05] Speaker 1: Привет.\n[01:05] Speaker 2: Да.\n[1:02:05] Speaker 1: Итак.\n");
    }

    #[test]
    fn the_notes_are_in_the_chosen_language_or_the_meetings() {
        assert!(summary_instructions(Some("Russian")).ends_with("Write the notes in Russian."));
        assert!(summary_instructions(None).ends_with("the language the meeting was held in."));
        assert!(summary_instructions(None).starts_with(SUMMARY_INSTRUCTIONS));
    }

    #[test]
    fn a_transcript_past_its_share_of_the_window_is_too_long() {
        // 75 % of 1000 tokens, at four characters a token.
        assert_eq!(fits(&"a".repeat(3000), Some(1000)), Ok(()));
        assert_eq!(fits(&"a".repeat(3004), Some(1000)), Err(MeetingError::TooLong { tokens: 751, limit: 750 }));
    }

    #[test]
    fn a_meetings_row_counts_its_speakers_and_says_if_it_has_notes() {
        let meeting = Meeting {
            id: "m".into(),
            name: "call.wav".into(),
            created: "2026-10-20T10:00:00Z".into(),
            seconds: 60.0,
            utterances: vec![said(0, 0.0, "а"), said(2, 1.0, "б")],
            summary: None,
        };
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
            SidecarLine::Result { utterances: vec![Utterance { speaker: 1, start: 0.5, end: 1.0, text: "да".into() }], seconds: 3.0 },
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
