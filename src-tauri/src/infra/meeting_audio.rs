//! `meeting://`: a meeting's recording for the page's player — `/<id>` is
//! that meeting's `audio.wav` (`meeting_store`), nothing else. A media element
//! asks for byte ranges as it plays and seeks, and is answered in them.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use tauri::http::Response;

use crate::infra::meeting_store;

pub const SCHEME: &str = "meeting";

/// A range is answered this much at a time, at most: the player asks for the
/// rest as it goes, and an hour's 115 MB is never read whole. Chromium
/// (WebView2) asks `bytes=N-`; WebKit's player (macOS) names the end, often the
/// file's last byte. A shorter answer than asked is fine by both: they read
/// `Content-Range`.
const CHUNK: u64 = 1 << 20;

/// The response to a request for `path`, with its `Range` header if it had one:
/// without one, the whole file — what a media element never asks for.
pub fn respond(path: &str, range: Option<&str>) -> Response<Vec<u8>> {
    let id = path.trim_start_matches('/');
    let Some((mut file, total)) = meeting_store::audio(id)
        .ok()
        .and_then(|path| File::open(path).ok())
        .and_then(|file| file.metadata().ok().map(|meta| (file, meta.len())))
    else {
        return plain(404, "not found");
    };
    let Some(range) = range else {
        let mut body = Vec::new();
        if file.read_to_end(&mut body).is_err() {
            return plain(500, "could not read the recording");
        }
        return Response::builder()
            .status(200)
            .header("Content-Type", "audio/wav")
            .header("Accept-Ranges", "bytes")
            .header("Content-Length", body.len())
            .body(body)
            .unwrap_or_default();
    };
    let Some((start, end)) = bytes(range, total) else {
        return Response::builder()
            .status(416)
            .header("Content-Range", format!("bytes */{total}"))
            .body(Vec::new())
            .unwrap_or_default();
    };
    let mut body = vec![0; (end - start + 1) as usize];
    if file.seek(SeekFrom::Start(start)).and_then(|_| file.read_exact(&mut body)).is_err() {
        return plain(500, "could not read the recording");
    }
    Response::builder()
        .status(206)
        .header("Content-Type", "audio/wav")
        .header("Accept-Ranges", "bytes")
        .header("Content-Range", format!("bytes {start}-{end}/{total}"))
        .header("Content-Length", body.len())
        .body(body)
        .unwrap_or_default()
}

fn plain(status: u16, text: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(text.as_bytes().to_vec())
        .unwrap_or_default()
}

/// The first range of a `Range` header, inclusive, within `total` bytes;
/// `None` when it asks for none of them.
fn bytes(header: &str, total: u64) -> Option<(u64, u64)> {
    let spec = header.trim().strip_prefix("bytes=")?.split(',').next()?.trim();
    let (from, to) = spec.split_once('-')?;
    let last = total.checked_sub(1)?;
    let (start, end) = match (from.trim(), to.trim()) {
        ("", suffix) => (total.saturating_sub(suffix.parse().ok()?), last),
        (from, "") => (from.parse().ok()?, last),
        (from, to) => (from.parse().ok()?, to.parse::<u64>().ok()?.min(last)),
    };
    (start <= end).then_some((start, end.min(start.saturating_add(CHUNK - 1))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

    #[test]
    fn a_range_is_what_it_asks_for_within_the_file() {
        assert_eq!(bytes("bytes=0-1", 100), Some((0, 1)));
        assert_eq!(bytes("bytes=90-200", 100), Some((90, 99)), "past the end is to the end");
        assert_eq!(bytes("bytes=-10", 100), Some((90, 99)));
        assert_eq!(bytes("bytes=-500", 100), Some((0, 99)));
        assert_eq!(bytes("bytes=10-", 100), Some((10, 99)));
        assert_eq!(bytes("bytes=10-", 10 * CHUNK), Some((10, 10 + CHUNK - 1)), "an open end is a chunk");
        assert_eq!(bytes(&format!("bytes=10-{}", 10 * CHUNK - 1), 10 * CHUNK), Some((10, 10 + CHUNK - 1)), "so is a far one");
        assert_eq!(bytes("bytes=-100", 10 * CHUNK), Some((10 * CHUNK - 100, 10 * CHUNK - 1)));
        assert_eq!(bytes("bytes=5-6, 9-10", 100), Some((5, 6)));
        for unsatisfiable in ["bytes=100-", "bytes=7-3", "items=0-1", "bytes=a-b", "bytes=0"] {
            assert_eq!(bytes(unsatisfiable, 100), None, "{unsatisfiable}");
        }
        assert_eq!(bytes("bytes=0-", 0), None, "an empty file has no bytes");
    }

    #[test]
    fn a_meetings_recording_is_served_in_the_ranges_asked_for() {
        with_app_dir("meeting-audio", || {
            let path = meeting_store::audio("ab").unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"RIFF0123456789").unwrap();

            let part = respond("/ab", Some("bytes=4-7"));
            assert_eq!(part.status(), 206);
            assert_eq!(part.body(), b"0123");
            assert_eq!(part.headers()["Content-Range"], "bytes 4-7/14");
            assert_eq!(part.headers()["Content-Type"], "audio/wav");
            let whole = respond("/ab", None);
            assert_eq!((whole.status().as_u16(), whole.body().as_slice()), (200, b"RIFF0123456789".as_slice()), "no range is the whole file");
            assert_eq!(respond("/ab", Some("bytes=20-")).status(), 416);
            assert_eq!(respond("/cd", None).status(), 404);
            assert_eq!(respond("/../settings.json", None).status(), 404);
        });
    }
}
