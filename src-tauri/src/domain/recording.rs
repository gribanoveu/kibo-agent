//! Recording a call (`docs/36-meeting-notes-research.md`, "Запись созвона"):
//! the microphone and the call's sound side by side in one stereo WAV — L the
//! microphone, R the call — at the rate the models read. The rules of the file
//! live here; the devices and the disk are `infra::meeting_recorder`.

use serde::Serialize;

/// How loud each track was over the last moment, 0 to 1, and how long the
/// recording is by now — what the page draws while it records.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Levels {
    pub seconds: f32,
    pub mic: f32,
    pub call: f32,
}

/// The recording under way, for a page opened while it runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordingState {
    pub id: String,
    pub seconds: f32,
    /// The microphone written as silence for now.
    pub muted: bool,
}

/// A call recorded and not yet made into a meeting: a row of the list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordingInfo {
    pub id: String,
    /// When it began — its file last written, less its length — RFC 3339.
    pub created: String,
    pub seconds: f32,
}

/// A microphone a call can be recorded from, for the page to choose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Microphone {
    /// The system's id for it, kept in the settings as it is.
    pub id: String,
    pub name: String,
}

/// What a recording needs the system's leave for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    Microphone,
    /// What the computer plays — the other side of the call.
    Call,
}

/// Whether the system lets the app record a [`Source`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Access {
    Granted,
    /// Refused, or barred by a policy: asking again shows nothing, only
    /// System Settings changes it.
    Denied,
    /// Not asked yet: asking shows the system's prompt.
    NotAsked,
    /// The system does not say — another platform, or no way to ask.
    Unknown,
}

/// Both sources' access, for the page to say what is missing before Record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingAccess {
    pub microphone: Access,
    pub call: Access,
}

/// How long `data_bytes` of [`CHANNELS`] at [`RATE`] last.
pub fn seconds_of(data_bytes: u64) -> f32 {
    data_bytes as f32 / (RATE as f32 * CHANNELS as f32 * 2.0)
}

/// The loudest of `samples`, for a level meter; silence is 0.
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, s| m.max(s.abs())).min(1.0)
}

/// What the models read, and so what is kept.
pub const RATE: u32 = 16_000;
/// The canonical header: `RIFF`, `fmt ` and `data` with nothing between.
pub const HEADER_BYTES: u64 = 44;
/// The microphone and the call.
pub const CHANNELS: u16 = 2;

/// A 16-bit PCM WAV header for `channels` at [`RATE`], saying `data_bytes` of samples follow.
pub fn wav_header(channels: u16, data_bytes: u32) -> [u8; HEADER_BYTES as usize] {
    let block = channels * 2;
    let mut out = [0u8; HEADER_BYTES as usize];
    out[0..4].copy_from_slice(b"RIFF");
    out[4..8].copy_from_slice(&data_bytes.saturating_add(36).to_le_bytes());
    out[8..16].copy_from_slice(b"WAVEfmt ");
    out[16..20].copy_from_slice(&16u32.to_le_bytes()); // the format chunk's size
    out[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    out[22..24].copy_from_slice(&channels.to_le_bytes());
    out[24..28].copy_from_slice(&RATE.to_le_bytes());
    out[28..32].copy_from_slice(&(RATE * block as u32).to_le_bytes()); // bytes a second
    out[32..34].copy_from_slice(&block.to_le_bytes()); // bytes a frame
    out[34..36].copy_from_slice(&16u16.to_le_bytes()); // bits a sample
    out[36..40].copy_from_slice(b"data");
    out[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    out
}

/// The most sample bytes a WAV holds — its sizes are 32-bit; 18 hours of two channels.
pub fn max_data_bytes(channels: u16) -> u32 {
    let block = channels as u32 * 2;
    (u32::MAX - 36) / block * block
}

/// The data size a header should say for a file `file_len` bytes long: the
/// whole frames written after it. A recording cut off — the app crashed, the
/// power went — says 0 until this is put in, and reads as nothing without it.
pub fn data_bytes_of(file_len: u64, channels: u16) -> u32 {
    let block = channels as u64 * 2;
    let whole = file_len.saturating_sub(HEADER_BYTES) / block * block;
    whole.min(max_data_bytes(channels) as u64) as u32
}

/// Interleaved frames of `channels` as one channel, their mean.
pub fn mono(interleaved: &[f32], channels: usize) -> impl Iterator<Item = f32> + '_ {
    let channels = channels.max(1);
    interleaved.chunks(channels).map(move |frame| frame.iter().sum::<f32>() / channels as f32)
}

/// Sound a device gives within this of where its last ended follows on it; later
/// is after a gap — the device gave nothing (Windows' loopback while nothing
/// plays) or was opened again.
pub const GAP_NANOS: u64 = 50_000_000;

/// Silence, in frames, to put before sound a device captured `at` nanoseconds
/// into the recording, so it lands where it was heard: `placed` frames of its
/// track are there already, and its last sound ended at `last_end` (`None`: this
/// is its first). Only after a gap — within a stream the device's own clock is
/// trusted, so its drift against the other side never becomes holes in speech.
/// Silence [`frames_ready`] already wrote for a device behind counts as placed.
pub fn silence_before(at: u64, last_end: Option<u64>, placed: u64) -> u64 {
    if last_end.is_some_and(|end| at < end.saturating_add(GAP_NANOS)) {
        return 0;
    }
    let due = (at as u128 * RATE as u128 / 1_000_000_000) as u64;
    due.saturating_sub(placed)
}

/// How many frames to write now, given each track's frames waiting and where
/// its last sound ended, nanoseconds in (`None`: none yet). The two devices
/// deliver at their own pace, so what both have is written and the rest waits
/// for its other half — unless the shorter one's sound ended `max_lag` frames'
/// time before the other's: a device that stopped delivering is silence, not a
/// reason to hold the other back. One still delivering is only behind by its
/// clock's drift, which a pad would make a hole in its speech; it is waited
/// for. `flush` (the recording stops) writes everything, the shorter padded.
/// Where the late one's sound then goes is [`silence_before`]'s.
pub fn frames_ready(mic: (usize, Option<u64>), call: (usize, Option<u64>), max_lag: usize, flush: bool) -> usize {
    let (short, long) = if mic.0 <= call.0 { (mic, call) } else { (call, mic) };
    let lag = max_lag as u64 * 1_000_000_000 / RATE as u64;
    let gone_quiet = match (short.1, long.1) {
        (None, _) => true,
        (Some(short), Some(long)) => long >= short.saturating_add(lag),
        (Some(_), None) => false,
    };
    if flush {
        long.0
    } else if gone_quiet {
        short.0.max(long.0.saturating_sub(max_lag))
    } else {
        short.0
    }
}

/// The first `frames` of each track as 16-bit stereo, L `mic` and R `call`; a
/// track shorter than that is silence past its end. Past ±1 is clipped.
pub fn interleave(mic: &[f32], call: &[f32], frames: usize) -> Vec<u8> {
    let sample = |track: &[f32], i: usize| ((track.get(i).copied().unwrap_or(0.0) * i16::MAX as f32) as i16).to_le_bytes();
    let mut out = Vec::with_capacity(frames * 4);
    for i in 0..frames {
        out.extend_from_slice(&sample(mic, i));
        out.extend_from_slice(&sample(call, i));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le32(bytes: &[u8]) -> u32 {
        u32::from_le_bytes(bytes.try_into().unwrap())
    }

    #[test]
    fn the_header_is_a_stereo_16_khz_pcm_one() {
        let h = wav_header(2, 4000);
        assert_eq!((&h[0..4], &h[8..16], &h[36..40]), (b"RIFF".as_slice(), b"WAVEfmt ".as_slice(), b"data".as_slice()));
        assert_eq!((le32(&h[4..8]), le32(&h[40..44])), (4036, 4000));
        assert_eq!(u16::from_le_bytes([h[22], h[23]]), 2, "channels");
        assert_eq!(le32(&h[24..28]), 16_000);
        assert_eq!(le32(&h[28..32]), 64_000, "bytes a second: 16 kHz, two 16-bit channels");
        assert_eq!(u16::from_le_bytes([h[32], h[33]]), 4, "bytes a frame");
        assert_eq!(le32(&wav_header(1, 32_000)[28..32]), 32_000, "mono is half");
        assert_eq!(le32(&wav_header(2, u32::MAX)[4..8]), u32::MAX, "the size saturates, it does not wrap");
    }

    #[test]
    fn a_cut_off_file_is_sized_by_its_whole_frames() {
        assert_eq!(data_bytes_of(44 + 40, 2), 40);
        assert_eq!(data_bytes_of(44 + 43, 2), 40, "a frame cut off midway is not one");
        assert_eq!(data_bytes_of(44 + 3, 1), 2);
        assert_eq!(data_bytes_of(20, 2), 0, "less than a header is nothing");
        assert_eq!(data_bytes_of(u64::MAX, 2), max_data_bytes(2), "never past what a WAV holds");
        assert_eq!(max_data_bytes(2) % 4, 0);
        assert!(max_data_bytes(2) as u64 + 36 <= u32::MAX as u64);
    }

    #[test]
    fn a_meter_reads_the_loudest_and_a_file_its_length() {
        assert_eq!(peak(&[0.1, -0.6, 0.3]), 0.6);
        assert_eq!(peak(&[]), 0.0);
        assert_eq!(peak(&[3.0]), 1.0, "past full is full");
        assert_eq!(seconds_of(64_000 * 90), 90.0);
        assert_eq!(seconds_of(0), 0.0);
    }

    #[test]
    fn channels_are_averaged_into_one() {
        assert_eq!(mono(&[1.0, 0.0, 0.5, 0.5], 2).collect::<Vec<_>>(), [0.5, 0.5]);
        assert_eq!(mono(&[0.25, -0.25], 1).collect::<Vec<_>>(), [0.25, -0.25]);
        assert_eq!(mono(&[0.25, 0.5, 0.75], 3).collect::<Vec<_>>(), [0.5]);
        assert_eq!(mono(&[0.5], 0).collect::<Vec<_>>(), [0.5], "no channels is taken as one");
    }

    #[test]
    fn what_both_tracks_have_is_written_and_a_silent_one_holds_nothing_back() {
        const S: u64 = 1_000_000_000;
        let (now, early) = (Some(3 * S), Some(2 * S));
        assert_eq!(frames_ready((1000, now), (990, now), 16_000, false), 990, "the rest waits for its other half");
        assert_eq!(frames_ready((990, now), (1000, now), 16_000, false), 990);
        assert_eq!(frames_ready((40_000, now), (0, early), 16_000, false), 24_000, "a track whose sound ended a second before is silence");
        assert_eq!(frames_ready((0, early), (40_000, now), 16_000, false), 24_000);
        assert_eq!(frames_ready((40_000, now), (0, None), 16_000, false), 24_000, "so is one never heard");
        assert_eq!(frames_ready((40_000, now), (0, Some(2 * S + 1)), 16_000, false), 0, "a moment short of a second it is waited for");
        // Its clock slower, a second short in frames while heard to the same moment: waited for, no hole.
        assert_eq!(frames_ready((40_000, now), (20_000, now), 16_000, false), 20_000, "drift is not silence");
        assert_eq!(frames_ready((40_000, None), (0, now), 16_000, false), 0, "the longer unheard is no reason to pad");
        assert_eq!(frames_ready((1000, now), (990, now), 16_000, true), 1000, "a stop writes it all");
    }

    #[test]
    fn sound_after_a_gap_lands_where_it_was_heard() {
        const S: u64 = 1_000_000_000;
        assert_eq!(silence_before(2 * S, None, 0), 32_000, "a device first heard two seconds in starts there");
        assert_eq!(silence_before(0, None, 0), 0);
        assert_eq!(silence_before(5 * S, Some(S), 16_000), 64_000, "four seconds of nothing from it are silence");
        assert_eq!(silence_before(5 * S, Some(S), 64_000), 16_000, "what was padded while it lagged is not put twice");
        assert_eq!(silence_before(5 * S, Some(S), 90_000), 0, "never back over what is written");
        assert_eq!(silence_before(S + GAP_NANOS - 1, Some(S), 0), 0, "within the gap it follows on: drift is not a hole");
        assert_eq!(silence_before(S + GAP_NANOS, Some(S), 0), 16_800, "a gap is a gap");
    }

    #[test]
    fn frames_are_left_then_right_and_a_short_track_is_silence() {
        let bytes = interleave(&[0.5, -1.0], &[1.0], 2);
        let samples: Vec<i16> = bytes.chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        assert_eq!(samples, [16383, 32767, -32767, 0]);
        assert_eq!(interleave(&[2.0], &[-2.0], 1), [0xff, 0x7f, 0x00, 0x80], "clipped: `as` saturates");
        assert!(interleave(&[0.1], &[0.1], 0).is_empty());
    }
}
