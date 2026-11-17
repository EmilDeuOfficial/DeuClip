//! Single instance: a second start of DeuClip only brings the running one to the front.

use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
    INFINITE,
};

#[derive(Clone, Copy)]
struct SendHandle(HANDLE);
unsafe impl Send for SendHandle {}

/// Keeps the named mutex alive for the whole process lifetime.
pub struct Instance {
    _mutex: Option<SendHandle>,
    event: Option<SendHandle>,
}

/// Returns `None` when DeuClip is already running. In that case the running instance
/// has been asked to show its window and this process should exit.
pub fn acquire() -> Option<Instance> {
    unsafe {
        let mutex = match CreateMutexW(None, true, w!("Local\\DeuClipSingleInstance")) {
            Ok(handle) => handle,
            Err(e) => {
                log::warn!("Single-instance check unavailable: {e}");
                return Some(Instance {
                    _mutex: None,
                    event: None,
                });
            }
        };
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, w!("Local\\DeuClipShowWindow")) {
                let _ = SetEvent(event);
                let _ = CloseHandle(event);
            }
            let _ = CloseHandle(mutex);
            log::info!("DeuClip is already running, asked it to show its window");
            return None;
        }
        let event = CreateEventW(None, false, false, w!("Local\\DeuClipShowWindow")).ok();
        Some(Instance {
            _mutex: Some(SendHandle(mutex)),
            event: event.map(SendHandle),
        })
    }
}

impl Instance {
    /// Calls `on_show` (in its own thread) every time another start of DeuClip asks for it.
    pub fn listen(&self, on_show: impl Fn() + Send + 'static) {
        let Some(event) = self.event else { return };
        let spawned = std::thread::Builder::new()
            .name("deuclip-instance".into())
            .spawn(move || {
                let event = event;
                loop {
                    let result = unsafe { WaitForSingleObject(event.0, INFINITE) };
                    if result != WAIT_OBJECT_0 {
                        break;
                    }
                    on_show();
                }
            });
        if let Err(e) = spawned {
            log::warn!("Could not start the instance listener: {e}");
        }
    }
}
