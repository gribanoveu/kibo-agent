//! Selecting an installed keyboard layout, without listening to or injecting keys.

use crate::domain::keyboard_layout::KeyboardLayout;

pub const SUPPORTED: bool = cfg!(any(target_os = "macos", windows));

#[derive(Debug, thiserror::Error)]
pub enum KeyboardLayoutError {
    #[error("No enabled {0} keyboard layout was found; add one in the system settings")]
    NotInstalled(&'static str),
    #[error("The system refused to switch the keyboard layout (status {0})")]
    SelectFailed(i32),
    #[error("Switching the keyboard layout is supported on macOS and Windows only")]
    Unsupported,
}

#[cfg(target_os = "macos")]
pub fn set(layout: KeyboardLayout) -> Result<(), KeyboardLayoutError> {
    use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
    use core_foundation::string::{CFString, CFStringRef};
    use std::ffi::c_void;

    type TisInputSourceRef = *const c_void;

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        fn TISCopyInputSourceForLanguage(language: CFStringRef) -> TisInputSourceRef;
        fn TISSelectInputSource(source: TisInputSourceRef) -> i32;
    }

    let language = CFString::new(layout.language_code());
    // SAFETY: `language` is a live CFString. The Copy rule gives us ownership of
    // a returned source, which is released exactly once after selection.
    let source = unsafe { TISCopyInputSourceForLanguage(language.as_concrete_TypeRef()) };
    if source.is_null() {
        return Err(KeyboardLayoutError::NotInstalled(layout.language_code()));
    }
    let status = unsafe {
        let status = TISSelectInputSource(source);
        CFRelease(source as CFTypeRef);
        status
    };
    if status == 0 { Ok(()) } else { Err(KeyboardLayoutError::SelectFailed(status)) }
}

#[cfg(windows)]
pub fn set(layout: KeyboardLayout) -> Result<(), KeyboardLayoutError> {
    use std::ffi::c_void;

    type Hkl = *mut c_void;

    #[link(name = "user32")]
    extern "system" {
        fn GetKeyboardLayoutList(buffer_len: i32, layouts: *mut Hkl) -> i32;
        fn ActivateKeyboardLayout(layout: Hkl, flags: u32) -> Hkl;
    }

    // PRIMARYLANGID: the low ten bits of the LANGID in the HKL's low word.
    let wanted = match layout {
        KeyboardLayout::Ru => 0x19,
        KeyboardLayout::En => 0x09,
    };
    // SAFETY: the first call sizes the buffer; the second receives exactly that
    // capacity. Both calls and activation stay on Tauri's main thread: on Windows
    // the active layout belongs to the calling thread.
    unsafe {
        let count = GetKeyboardLayoutList(0, std::ptr::null_mut());
        let mut layouts = vec![std::ptr::null_mut(); count.max(0) as usize];
        let loaded = GetKeyboardLayoutList(count, layouts.as_mut_ptr()).max(0) as usize;
        layouts.truncate(loaded);
        let target = layouts
            .into_iter()
            .find(|hkl| ((*hkl as usize as u16) & 0x03ff) == wanted)
            .ok_or(KeyboardLayoutError::NotInstalled(layout.language_code()))?;
        if ActivateKeyboardLayout(target, 0).is_null() {
            return Err(KeyboardLayoutError::SelectFailed(0));
        }
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn set(_layout: KeyboardLayout) -> Result<(), KeyboardLayoutError> {
    Err(KeyboardLayoutError::Unsupported)
}
