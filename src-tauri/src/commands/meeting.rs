//! The Meetings page: recordings made into transcripts and notes. Processing
//! reports on [`MEETING_EVENT`], each event naming its meeting.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::{AppHandle, Builder, Emitter, Manager, Runtime, State};

use crate::domain::meeting::{JobState, Meeting, MeetingEvent, MeetingEventSink, MeetingInfo};
use crate::domain::recording::{Access, Microphone, RecordingAccess, RecordingInfo, RecordingState, Source};
use crate::domain::voices::VoiceInfo;
use crate::infra::{meeting_audio, meeting_recorder, meeting_store, recording_access, settings_store, speech};
use crate::services::meeting_notes::{self, MeetingNotes};

use super::blocking;

/// A meeting's progress, its transcript saved, a piece of its summary.
pub const MEETING_EVENT: &str = "meeting:event";

fn meeting_event_sink<R: Runtime>(app: &AppHandle<R>) -> MeetingEventSink {
    let app = app.clone();
    Arc::new(move |event: MeetingEvent| {
        let _ = app.emit(MEETING_EVENT, event);
    })
}

#[tauri::command]
pub async fn meeting_list() -> Result<Vec<MeetingInfo>, String> {
    blocking(meeting_store::list).await
}

#[tauri::command]
pub async fn meeting_get(id: String) -> Result<Meeting, String> {
    blocking(move || meeting_store::get(&id)).await
}

/// The meeting after speaker `speaker` (from 0) is called `name`; blank gives back its number.
#[tauri::command]
pub async fn meeting_rename_speaker(id: String, speaker: usize, name: String) -> Result<Meeting, String> {
    blocking(move || meeting_notes::rename_speaker(&id, speaker, &name)).await
}

#[tauri::command]
pub async fn meeting_delete(notes: State<'_, Arc<MeetingNotes>>, id: String) -> Result<(), String> {
    let notes = notes.inner().clone();
    blocking(move || notes.delete_meeting(&id)).await
}

/// The voices the voice memory knows, by name.
#[tauri::command]
pub async fn meeting_voices() -> Result<Vec<VoiceInfo>, String> {
    blocking(meeting_notes::voices).await
}

/// The voice forgotten with every sample of it; the list after.
#[tauri::command]
pub async fn meeting_voice_delete(id: String) -> Result<Vec<VoiceInfo>, String> {
    blocking(move || meeting_notes::delete_voice(&id)).await
}

/// The recording at `path` recognized, saved and summarized; resolves with
/// the meeting's id once it is all done.
#[tauri::command]
pub async fn meeting_process<R: Runtime>(app: AppHandle<R>, notes: State<'_, Arc<MeetingNotes>>, path: PathBuf) -> Result<String, String> {
    let notes = notes.inner().clone();
    let sink = meeting_event_sink(&app);
    let models = speech::models_dir(app.path().resource_dir().ok().as_deref());
    let exe = std::env::current_exe().map_err(|e| format!("where this app is: {e}"))?;
    blocking(move || notes.process(&exe, &models, Path::new(&path), &sink)).await
}

#[tauri::command]
pub async fn meeting_summarize<R: Runtime>(app: AppHandle<R>, notes: State<'_, Arc<MeetingNotes>>, id: String) -> Result<(), String> {
    let notes = notes.inner().clone();
    let sink = meeting_event_sink(&app);
    blocking(move || notes.summarize(&id, &sink)).await
}

/// A document on `topic` written from meeting `id`; resolves once it is saved.
#[tauri::command]
pub async fn meeting_write_document<R: Runtime>(app: AppHandle<R>, notes: State<'_, Arc<MeetingNotes>>, id: String, topic: String) -> Result<(), String> {
    let notes = notes.inner().clone();
    let sink = meeting_event_sink(&app);
    blocking(move || notes.write_document(&id, &topic, &sink)).await
}

/// The meeting after its document on `topic` is gone.
#[tauri::command]
pub async fn meeting_delete_document(id: String, topic: String) -> Result<Meeting, String> {
    blocking(move || meeting_notes::delete_document(&id, &topic)).await
}

/// A kill and a wait for the process: off the main thread.
#[tauri::command]
pub async fn meeting_stop(notes: State<'_, Arc<MeetingNotes>>) -> Result<(), String> {
    let notes = notes.inner().clone();
    blocking(move || {
        notes.stop();
        Ok::<_, String>(())
    })
    .await
}

/// The call recorded from now on, both devices open once this resolves; the
/// recording's id. Its length and levels report on [`MEETING_EVENT`].
#[tauri::command]
pub async fn meeting_record_start<R: Runtime>(app: AppHandle<R>, notes: State<'_, Arc<MeetingNotes>>) -> Result<String, String> {
    let notes = notes.inner().clone();
    let sink = meeting_event_sink(&app);
    blocking(move || notes.record_start(&sink)).await
}

/// Your microphone written as silence in the recording from now on, or heard again.
#[tauri::command]
pub fn meeting_record_mute(notes: State<'_, Arc<MeetingNotes>>, on: bool) -> Result<(), String> {
    notes.record_mute(on).map_err(|e| e.to_string())
}

/// The call being recorded now, if one is — for the page opened while it records.
#[tauri::command]
pub fn meeting_recording(notes: State<'_, Arc<MeetingNotes>>) -> Option<RecordingState> {
    notes.recording()
}

/// The calls recorded and not yet transcribed, newest first.
#[tauri::command]
pub async fn meeting_recordings(notes: State<'_, Arc<MeetingNotes>>) -> Result<Vec<RecordingInfo>, String> {
    let notes = notes.inner().clone();
    blocking(move || notes.recordings()).await
}

/// The recorded call `id` made into a meeting, as an import is; resolves with
/// the meeting's id once it is all done.
#[tauri::command]
pub async fn meeting_process_recording<R: Runtime>(app: AppHandle<R>, notes: State<'_, Arc<MeetingNotes>>, id: String) -> Result<String, String> {
    let notes = notes.inner().clone();
    let sink = meeting_event_sink(&app);
    let models = speech::models_dir(app.path().resource_dir().ok().as_deref());
    let exe = std::env::current_exe().map_err(|e| format!("where this app is: {e}"))?;
    blocking(move || notes.process_recording(&exe, &models, &id, &sink)).await
}

#[tauri::command]
pub async fn meeting_recording_delete(notes: State<'_, Arc<MeetingNotes>>, id: String) -> Result<(), String> {
    let notes = notes.inner().clone();
    blocking(move || notes.delete_recording(&id)).await
}

/// Whether the system lets the app record the microphone and the call's sound.
#[tauri::command]
pub async fn meeting_recording_access() -> Result<RecordingAccess, String> {
    blocking(|| Ok::<_, String>(recording_access::status())).await
}

/// The system's prompt for `source`, waited for; the access after.
#[tauri::command]
pub async fn meeting_recording_access_request(source: Source) -> Result<Access, String> {
    blocking(move || Ok::<_, String>(recording_access::request(source))).await
}

/// System Settings opened where `source` is turned on — for one refused, which no prompt asks again.
#[tauri::command]
pub fn meeting_recording_access_settings(source: Source) -> Result<(), String> {
    let url = recording_access::settings_url(source).ok_or("this system has no such setting")?;
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| e.to_string())
}

/// Whether a recorded call's microphone is denoised when it is transcribed.
#[tauri::command]
pub fn meeting_denoise_get() -> bool {
    settings_store::load().unwrap_or_default().meeting_denoise()
}

#[tauri::command]
pub fn meeting_denoise_set(on: bool) -> Result<(), String> {
    settings_store::update(|s| s.meeting_denoise = Some(on)).map_err(|e| e.to_string())
}

/// The microphones plugged in, to choose one to record from.
#[tauri::command]
pub async fn meeting_microphones() -> Result<Vec<Microphone>, String> {
    blocking(|| meeting_recorder::microphones().map_err(|e| e.to_string())).await
}

/// The microphone a call is recorded from, by its id; `None` is the system's default.
#[tauri::command]
pub fn meeting_microphone_get() -> Option<String> {
    settings_store::load().unwrap_or_default().meeting_microphone
}

#[tauri::command]
pub fn meeting_microphone_set(id: Option<String>) -> Result<(), String> {
    settings_store::update(|s| s.meeting_microphone = id).map_err(|e| e.to_string())
}

/// Whether a recording stopped is transcribed at once.
#[tauri::command]
pub fn meeting_auto_process_get() -> bool {
    settings_store::load().map(|s| s.meeting_auto_process).unwrap_or_default()
}

#[tauri::command]
pub fn meeting_auto_process_set(on: bool) -> Result<(), String> {
    settings_store::update(|s| s.meeting_auto_process = on).map_err(|e| e.to_string())
}

/// The recording stopped and its file whole; its id.
#[tauri::command]
pub async fn meeting_record_stop(notes: State<'_, Arc<MeetingNotes>>) -> Result<String, String> {
    let notes = notes.inner().clone();
    blocking(move || notes.record_stop()).await
}

/// The job running now, if one is — for the page opened while it runs.
#[tauri::command]
pub fn meeting_job(notes: State<'_, Arc<MeetingNotes>>) -> Option<JobState> {
    notes.current()
}

/// `meeting://<id>`: the meeting's recording for the page's player.
pub fn register<R: Runtime>(builder: Builder<R>) -> Builder<R> {
    builder.register_asynchronous_uri_scheme_protocol(meeting_audio::SCHEME, |_ctx, request, responder| {
        let path = request.uri().path().to_string();
        let range = request.headers().get("range").and_then(|v| v.to_str().ok()).map(str::to_string);
        // Off the webview's thread: a range is a read from disk.
        tauri::async_runtime::spawn_blocking(move || responder.respond(meeting_audio::respond(&path, range.as_deref())));
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channel_name_is_the_one_the_frontend_listens_on() {
        assert_eq!(MEETING_EVENT, "meeting:event");
    }
}
