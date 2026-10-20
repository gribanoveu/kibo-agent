//! The Meetings page: recordings made into transcripts and notes. Processing
//! reports on [`MEETING_EVENT`], each event naming its meeting.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::domain::meeting::{Meeting, MeetingEvent, MeetingEventSink, MeetingInfo};
use crate::domain::voices::VoiceInfo;
use crate::infra::{meeting_store, speech};
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
pub async fn meeting_delete(id: String) -> Result<(), String> {
    blocking(move || meeting_notes::delete_meeting(&id)).await
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

#[tauri::command]
pub fn meeting_stop(notes: State<'_, Arc<MeetingNotes>>) {
    notes.stop();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channel_name_is_the_one_the_frontend_listens_on() {
        assert_eq!(MEETING_EVENT, "meeting:event");
    }
}
