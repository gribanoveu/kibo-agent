//! Voice memory: a speaker the user named is known again by voice in the next
//! meeting (`docs/36-meeting-notes-research.md`, open question 3).
//!
//! A voice print is biometric data, so all of it lives in one place —
//! [`VoiceBook`], one file, removed whole by the feature's "Delete data" —
//! and only while the user turned the feature on. A name is learned only
//! from what the user did: renaming a speaker; a name the book guessed teaches
//! it nothing until the user keeps or corrects it.

use serde::{Deserialize, Serialize};

/// A speaker's mean voice is kept only past this much speech, all their
/// utterances together, and this many words: a cluster of word ends and "um"
/// looks alike in every meeting (0.91 between two such, docs/36). Words tell
/// it apart better than time — 14 s of it held 6 words; real people with 17 s
/// and 20 s, 36 and 42.
pub const MIN_SPEECH_SECONDS: f32 = 20.0;
pub const MIN_WORDS: usize = 20;
/// Words count toward [`MIN_WORDS`] only in an utterance this long: word ends
/// and "um" come one to an utterance, and on an hour's call they add up.
pub const MIN_PHRASE_WORDS: usize = 3;

/// How much a speaker said in a meeting, all their utterances together.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Speech {
    pub seconds: f32,
    /// Only those of utterances of [`MIN_PHRASE_WORDS`] or more.
    pub words: usize,
}
/// One person across two meetings was 0.78 alike, different people 0.52 at
/// most (docs/36): a voice is recognized from here up.
// ponytail: one pair of recordings set this; measure again as voices pile up.
pub const SAME_PERSON: f32 = 0.65;

/// A speaker of a meeting heard long enough to learn from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Heard {
    pub meeting: String,
    pub speaker: usize,
    /// Unit length.
    pub vector: Vec<f32>,
    /// The voice the user named this speaker as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Voice {
    pub id: String,
    pub name: String,
}

/// A row of the list: a voice and how many meetings it was named in.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceInfo {
    pub id: String,
    pub name: String,
    pub meetings: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceBook {
    #[serde(default)]
    pub voices: Vec<Voice>,
    #[serde(default)]
    pub heard: Vec<Heard>,
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

impl VoiceBook {
    /// Meeting `id`'s speakers with speech enough to learn from, kept; the
    /// rest are not. `speech` is how much each speaker said, `voices` their means.
    pub fn hear(&mut self, id: &str, voices: &[Vec<f32>], speech: &[Speech]) {
        self.forget_meeting(id);
        for (speaker, vector) in voices.iter().enumerate() {
            // A speaker with no voice measured has nothing to learn from.
            if !vector.is_empty() && speech.get(speaker).is_some_and(|s| s.seconds >= MIN_SPEECH_SECONDS && s.words >= MIN_WORDS) {
                self.heard.push(Heard { meeting: id.to_string(), speaker, vector: vector.clone(), voice: None });
            }
        }
    }

    /// The names the book would give meeting `id`'s speakers: each one heard
    /// to the voice it is most like, when that is like enough.
    pub fn recognize(&self, id: &str) -> Vec<(usize, String)> {
        self.heard
            .iter()
            .filter(|h| h.meeting == id)
            .filter_map(|h| {
                self.voices
                    .iter()
                    .filter_map(|v| Some((v, cosine(&h.vector, &self.mean(&v.id)?))))
                    .filter(|(_, alike)| *alike >= SAME_PERSON)
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(v, _)| (h.speaker, v.name.clone()))
            })
            .collect()
    }

    /// The user called speaker `speaker` of meeting `id` `name` (blank: no
    /// name): that speaker's voice is now that person's. A voice no speaker is
    /// named as any more is gone. Nothing happens for a speaker not heard long enough.
    pub fn learn(&mut self, id: &str, speaker: usize, name: &str) {
        let Some(at) = self.heard.iter().position(|h| h.meeting == id && h.speaker == speaker) else { return };
        let name = name.trim();
        let voice = if name.is_empty() {
            None
        } else if let Some(v) = self.voices.iter().find(|v| v.name.to_lowercase() == name.to_lowercase()) {
            Some(v.id.clone())
        } else {
            let voice = Voice { id: uuid::Uuid::new_v4().to_string(), name: name.to_string() };
            self.voices.push(voice.clone());
            Some(voice.id)
        };
        self.heard[at].voice = voice;
        self.prune();
    }

    /// Meeting `id` deleted: what was heard in it goes.
    pub fn forget_meeting(&mut self, id: &str) {
        self.heard.retain(|h| h.meeting != id);
        self.prune();
    }

    /// The person forgotten: their voice and every sample of it.
    pub fn delete(&mut self, voice: &str) {
        self.heard.retain(|h| h.voice.as_deref() != Some(voice));
        self.voices.retain(|v| v.id != voice);
    }

    pub fn list(&self) -> Vec<VoiceInfo> {
        let mut rows: Vec<VoiceInfo> = self
            .voices
            .iter()
            .map(|v| VoiceInfo {
                id: v.id.clone(),
                name: v.name.clone(),
                meetings: self.heard.iter().filter(|h| h.voice.as_deref() == Some(&v.id)).count(),
            })
            .collect();
        rows.sort_by_key(|r| r.name.to_lowercase());
        rows
    }

    /// The unit mean of every sample named as `voice`.
    fn mean(&self, voice: &str) -> Option<Vec<f32>> {
        let samples: Vec<&Heard> = self.heard.iter().filter(|h| h.voice.as_deref() == Some(voice)).collect();
        let width = samples.first()?.vector.len();
        let mut sum = vec![0.0f32; width];
        for h in samples {
            sum.iter_mut().zip(&h.vector).for_each(|(s, x)| *s += x);
        }
        let norm = sum.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        Some(sum.iter().map(|x| x / norm).collect())
    }

    fn prune(&mut self) {
        let heard = &self.heard;
        self.voices.retain(|v| heard.iter().any(|h| h.voice.as_deref() == Some(&v.id)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit vector at `degrees` in the plane: cos 40° ≈ 0.77, cos 60° = 0.5.
    fn at(degrees: f32) -> Vec<f32> {
        let r = degrees.to_radians();
        vec![r.cos(), r.sin()]
    }

    fn spoke(seconds: f32) -> Speech {
        Speech { seconds, words: 100 }
    }

    fn book_with_albina() -> VoiceBook {
        let mut book = VoiceBook::default();
        book.hear("m1", &[at(0.0), at(90.0)], &[spoke(120.0), spoke(40.0)]);
        book.learn("m1", 0, "Альбина");
        book
    }

    #[test]
    fn a_named_voice_is_known_again_in_the_next_meeting_and_a_stranger_is_not() {
        let mut book = book_with_albina();
        book.hear("m2", &[at(60.0), at(40.0), at(-30.0)], &[spoke(300.0), spoke(90.0), spoke(60.0)]);
        assert_eq!(book.recognize("m2"), [(1, "Альбина".to_string()), (2, "Альбина".to_string())]);
        assert_eq!(book.recognize("m1"), [(0, "Альбина".to_string())]);

        // Like both, it is the one it is more like.
        book.learn("m2", 0, "Илья");
        book.hear("m3", &[at(35.0)], &[spoke(60.0)]); // 0.82 to Альбина, 0.91 to Илья
        assert_eq!(book.recognize("m3"), [(0, "Илья".to_string())]);
    }

    #[test]
    fn a_speaker_who_said_too_little_or_too_few_words_is_not_kept_nor_known() {
        let mut book = book_with_albina();
        book.hear("m2", &[at(0.0), at(5.0), at(5.0)], &[spoke(19.9), spoke(20.0), Speech { seconds: 60.0, words: 19 }]);
        assert_eq!(book.heard.iter().filter(|h| h.meeting == "m2").map(|h| h.speaker).collect::<Vec<_>>(), [1]);
        assert_eq!(book.recognize("m2"), [(1, "Альбина".to_string())]);
        // One with speech enough but no voice measured has nothing to keep.
        book.hear("m4", &[vec![], at(5.0)], &[spoke(60.0), spoke(60.0)]);
        assert_eq!(book.heard.iter().filter(|h| h.meeting == "m4").map(|h| h.speaker).collect::<Vec<_>>(), [1]);
        book.learn("m2", 0, "Илья");
        assert_eq!(book.list().len(), 1, "a speaker not kept teaches nothing");
    }

    #[test]
    fn samples_named_alike_are_one_voice_and_a_rename_moves_one() {
        let mut book = book_with_albina();
        book.hear("m2", &[at(10.0)], &[spoke(60.0)]);
        book.learn("m2", 0, " альбина ");
        assert_eq!(book.list(), [VoiceInfo { id: book.voices[0].id.clone(), name: "Альбина".into(), meetings: 2 }]);
        book.learn("m2", 0, "Оксана");
        assert_eq!(book.list().iter().map(|v| (v.name.as_str(), v.meetings)).collect::<Vec<_>>(), [("Альбина", 1), ("Оксана", 1)]);
        book.learn("m2", 0, "");
        assert_eq!(book.list().iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), ["Альбина"], "a voice named by nobody is gone");
    }

    #[test]
    fn a_deleted_voice_takes_its_samples_and_a_deleted_meeting_what_was_heard_in_it() {
        let mut book = book_with_albina();
        book.hear("m2", &[at(5.0)], &[spoke(60.0)]);
        book.learn("m2", 0, "Альбина");
        book.delete(&book.voices[0].id.clone());
        assert!(book.voices.is_empty());
        assert_eq!(book.heard.iter().map(|h| (h.meeting.as_str(), h.speaker)).collect::<Vec<_>>(), [("m1", 1)], "an unnamed one stays");

        let mut book = book_with_albina();
        book.forget_meeting("m1");
        assert_eq!(book, VoiceBook::default());
    }

    #[test]
    fn hearing_a_meeting_again_replaces_what_was_heard_in_it() {
        let mut book = book_with_albina();
        book.hear("m1", &[at(0.0)], &[spoke(60.0)]);
        assert_eq!(book.heard.len(), 1);
        assert!(book.voices.is_empty(), "its name went with it");
    }
}
