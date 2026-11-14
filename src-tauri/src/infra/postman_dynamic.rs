//! Postman's dynamic variables — `{{$randomEmail}}` and the rest — made by Postman's own
//! code: `postman-collection` with faker is inside the sandbox's bootcode. One sandbox,
//! on a thread of its own (a QuickJS runtime is not `Send`), booted for the first such
//! variable and gone after a minute without one.

use std::sync::mpsc::{self, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde_json::{json, Value};

use super::postman_sandbox::{Sandbox, LIMITS};

/// How long the sandbox waits for the next variable before it goes.
const IDLE: Duration = Duration::from_secs(60);

type Job = (String, SyncSender<Option<String>>);

/// Who makes the values, while its thread is there. Held while sending, so the
/// thread, which takes it before going, never leaves a variable sent to it unmade.
static MAKER: Mutex<Option<Sender<Job>>> = Mutex::new(None);

/// `{{$name}}` made as Postman makes it, `name` without its `$`; `None` for a name
/// Postman does not know, or when the sandbox could not make it.
pub fn make(name: &str) -> Option<String> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let (reply, answer) = mpsc::sync_channel(1);
    {
        let mut maker = MAKER.lock().unwrap_or_else(PoisonError::into_inner);
        // A thread gone since — its sandbox past a limit — hands the job back.
        let unsent = match maker.as_ref() {
            Some(running) => running.send((name.to_string(), reply)).err().map(|mpsc::SendError(job)| job),
            None => Some((name.to_string(), reply)),
        };
        if let Some(job) = unsent {
            let started = started();
            started.send(job).ok()?;
            *maker = Some(started);
        }
    }
    answer.recv_timeout(LIMITS.whole).ok().flatten()
}

fn started() -> Sender<Job> {
    let (jobs, inbox) = mpsc::channel::<Job>();
    std::thread::spawn(move || {
        let Ok(sandbox) = Sandbox::boot(LIMITS) else { return };
        let mut job = inbox.recv_timeout(IDLE);
        loop {
            match job {
                Ok((name, reply)) => {
                    let made = made(&sandbox, &name);
                    let broken = made.is_err();
                    let _ = reply.send(made.ok().flatten());
                    // Past a limit the sandbox is not used again: another takes what is queued, and
                    // what comes after — under the lock, so nothing sent meanwhile is lost.
                    if broken {
                        let mut maker = MAKER.lock().unwrap_or_else(PoisonError::into_inner);
                        let queued: Vec<Job> = inbox.try_iter().collect();
                        *maker = None;
                        if !queued.is_empty() {
                            let next = started();
                            queued.into_iter().for_each(|job| drop(next.send(job)));
                            *maker = Some(next);
                        }
                        return;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {
                    let mut maker = MAKER.lock().unwrap_or_else(PoisonError::into_inner);
                    if let Ok(late) = inbox.try_recv() {
                        drop(maker);
                        job = Ok(late);
                        continue;
                    }
                    *maker = None;
                    return;
                }
            }
            job = inbox.recv_timeout(IDLE);
        }
    });
    jobs
}

/// `Ok(None)`: the name is not Postman's, and comes back as written.
fn made(sandbox: &Sandbox, name: &str) -> Result<Option<String>, ()> {
    let code = format!("pm.environment.set('made', pm.variables.replaceIn('{{{{${name}}}}}'));");
    let ran = sandbox.run("prerequest", &code, "dynamic", json!({}), &mut |_| Err(crate::domain::http_client::HttpError::Script("no requests".into()))).map_err(|_| ())?;
    let values = ran.result["environment"]["values"].as_array().cloned().unwrap_or_default();
    let value = values.iter().find(|v| v["key"] == "made").and_then(|v| v["value"].as_str()).map(String::from);
    Ok(value.filter(|v| *v != format!("{{{{${name}}}}}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every name Postman lists, made by the sandbox: what the `{{` hint offers is sent.
    #[test]
    fn every_dynamic_variable_postman_lists_is_made() {
        let listed: Vec<Value> = serde_json::from_str(include_str!("../../../src/lib/postmanDynamic.json")).unwrap();
        assert!(listed.len() > 100, "{}", listed.len());
        for entry in &listed {
            let name = entry["name"].as_str().unwrap().strip_prefix('$').unwrap();
            let made = make(name);
            assert!(made.as_deref().is_some_and(|v| !v.is_empty() && !v.contains("{{")), "${name}: {made:?}");
        }
        assert_eq!(make("randomNothing"), None, "a name Postman does not know");
        // A name is a name: one that closes the string and sets the value itself is refused.
        assert_eq!(make("randomUUID}}')); pm.environment.set('made', 'written'); (('"), None, "only a name");
        assert_ne!(make("randomUUID"), make("randomUUID"), "made anew each time");
    }
}
