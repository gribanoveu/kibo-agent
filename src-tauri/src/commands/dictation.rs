//! Dictation into the message box. What it hears is the data itself and only
//! that box wants it, so it goes down the `Channel` [`dictation_start`] was
//! given, not on an event (`docs/52-voice-input.md`).

use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, Runtime, State};

use crate::domain::dictation::{DictationEvent, DictationEventSink};
use crate::infra::speech;
use crate::services::dictation::Dictation;

use super::blocking;

fn channel_sink(channel: Channel<DictationEvent>) -> DictationEventSink {
    Arc::new(move |event| {
        // A channel whose window is gone: nobody is left to tell.
        let _ = channel.send(event);
    })
}

/// A dictation begun, unless it is refused: everything after that, a failure
/// too, comes down `on_event`.
#[tauri::command]
pub async fn dictation_start<R: Runtime>(app: AppHandle<R>, dictation: State<'_, Arc<Dictation>>, on_event: Channel<DictationEvent>) -> Result<(), String> {
    let dictation = dictation.inner().clone();
    let models = speech::models_dir(app.path().resource_dir().ok().as_deref());
    let exe = std::env::current_exe().map_err(|e| format!("where this app is: {e}"))?;
    blocking(move || dictation.start(exe, models, channel_sink(on_event))).await
}

/// What was said stays: the rest of it comes down the channel, then its end.
#[tauri::command]
pub fn dictation_stop(dictation: State<'_, Arc<Dictation>>) {
    dictation.stop();
}

/// What was said is thrown away.
#[tauri::command]
pub fn dictation_cancel(dictation: State<'_, Arc<Dictation>>) {
    dictation.cancel();
}
