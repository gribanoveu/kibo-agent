//! Whether the system lets the app record a call, and asking it. macOS asks
//! per app: the microphone through AVFoundation, the system's sound through
//! TCC's `kTCCServiceAudioCapture` — which has no public call, so the private
//! TCC framework is opened as AudioCap does (`docs/36`, "Запись созвона"); if
//! a macOS without it comes, that access is [`Access::Unknown`] and the page's
//! silence hint is what is left. Elsewhere nothing is asked here.

use crate::domain::recording::{Access, RecordingAccess, Source};

pub fn status() -> RecordingAccess {
    RecordingAccess { microphone: imp::status(Source::Microphone), call: imp::status(Source::Call) }
}

/// The system's prompt for `source`, answered; what access is after. One
/// refused before shows no prompt: it stays [`Access::Denied`].
pub fn request(source: Source) -> Access {
    imp::request(source);
    imp::status(source)
}

/// Where in System Settings `source` is turned on, for one refused.
pub fn settings_url(source: Source) -> Option<&'static str> {
    cfg!(target_os = "macos").then_some(match source {
        Source::Microphone => "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
        Source::Call => "x-apple.systempreferences:com.apple.preference.security?Privacy_AudioCapture",
    })
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{c_char, c_void};
    use std::sync::mpsc;
    use std::sync::OnceLock;
    use std::time::Duration;

    use block2::{Block, RcBlock};
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, msg_send};

    use crate::domain::recording::{Access, Source};

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        static AVMediaTypeAudio: *const AnyObject;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(allocator: *const c_void, text: *const c_char, encoding: u32) -> *const c_void;
        fn CFRelease(object: *const c_void);
    }
    const UTF8: u32 = 0x0800_0100;

    type Preflight = unsafe extern "C" fn(service: *const c_void, options: *const c_void) -> i32;
    type Request = unsafe extern "C" fn(service: *const c_void, options: *const c_void, done: &Block<dyn Fn(Bool)>);

    /// A prompt left unanswered this long is taken as no answer.
    const ANSWER: Duration = Duration::from_secs(600);

    pub fn status(source: Source) -> Access {
        match source {
            // AVAuthorizationStatus: not determined, restricted, denied, authorized.
            Source::Microphone => match unsafe { msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: AVMediaTypeAudio] } {
                0isize => Access::NotAsked,
                1 | 2 => Access::Denied,
                3 => Access::Granted,
                _ => Access::Unknown,
            },
            // TCCAccessPreflight: 0 granted, 1 denied, anything else not decided.
            Source::Call => match tcc() {
                Some((preflight, _)) => match with_service(|service| unsafe { preflight(service, std::ptr::null()) }) {
                    0 => Access::Granted,
                    1 => Access::Denied,
                    _ => Access::NotAsked,
                },
                None => Access::Unknown,
            },
        }
    }

    /// Asks, and waits for the answer.
    pub fn request(source: Source) {
        let (answered, answer) = mpsc::channel::<bool>();
        let done = RcBlock::new(move |granted: Bool| {
            let _ = answered.send(granted.as_bool());
        });
        match source {
            Source::Microphone => unsafe {
                let _: () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType: AVMediaTypeAudio, completionHandler: &*done];
            },
            Source::Call => match tcc() {
                Some((_, request)) => with_service(|service| unsafe { request(service, std::ptr::null(), &done) }),
                None => return,
            },
        }
        let _ = answer.recv_timeout(ANSWER);
    }

    /// `kTCCServiceAudioCapture` as the CFString TCC takes.
    fn with_service<T>(f: impl FnOnce(*const c_void) -> T) -> T {
        let service = unsafe { CFStringCreateWithCString(std::ptr::null(), c"kTCCServiceAudioCapture".as_ptr(), UTF8) };
        let out = f(service);
        unsafe { CFRelease(service) };
        out
    }

    /// The private framework's two calls, if this macOS has them.
    fn tcc() -> Option<(Preflight, Request)> {
        static TCC: OnceLock<Option<(usize, usize)>> = OnceLock::new();
        let (preflight, request) = (*TCC.get_or_init(|| unsafe {
            let handle = libc::dlopen(c"/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC".as_ptr(), libc::RTLD_NOW);
            if handle.is_null() {
                return None;
            }
            let preflight = libc::dlsym(handle, c"TCCAccessPreflight".as_ptr());
            let request = libc::dlsym(handle, c"TCCAccessRequest".as_ptr());
            (!preflight.is_null() && !request.is_null()).then_some((preflight as usize, request as usize))
        }))?;
        // Addresses dlsym found under these names, with these C signatures (AudioCap's).
        Some(unsafe { (std::mem::transmute::<usize, Preflight>(preflight), std::mem::transmute::<usize, Request>(request)) })
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use crate::domain::recording::{Access, Source};

    // ponytail: Windows' "Let desktop apps access your microphone" is not read; R-3 reads it from the registry.
    pub fn status(_: Source) -> Access {
        Access::Unknown
    }

    pub fn request(_: Source) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asks this machine, without prompting: whatever the answer, it is one.
    #[test]
    fn the_system_answers_without_a_prompt() {
        let now = status();
        if cfg!(target_os = "macos") {
            assert_ne!(now.microphone, Access::Unknown, "AVFoundation always says");
            assert_ne!(now.call, Access::Unknown, "this macOS has TCC's calls");
        } else {
            assert_eq!(now, RecordingAccess { microphone: Access::Unknown, call: Access::Unknown });
        }
    }

    #[test]
    fn a_refused_source_has_its_settings_pane() {
        let urls = [settings_url(Source::Microphone), settings_url(Source::Call)];
        if cfg!(target_os = "macos") {
            assert!(urls[0].is_some_and(|u| u.ends_with("Privacy_Microphone")));
            assert!(urls[1].is_some_and(|u| u.ends_with("Privacy_AudioCapture")));
        } else {
            assert_eq!(urls, [None, None]);
        }
    }
}
