//! What PodStack asks of the operating system beyond files and the network: one running copy, waiting for an old
//! copy to exit, and the desktop's accent color and light/dark preference. Linux gets these from GTK, D-Bus and the
//! theme tools; on Windows they come from Win32 and the registry.

/// The result of claiming the single running copy.
pub enum Instance {
    /// This is the only copy. On Windows, `raised` yields whenever another launch asks this copy to show itself.
    First { raised: Option<std::sync::mpsc::Receiver<()>> },
    /// Another copy is running and has been asked to show its window; this one should exit.
    AlreadyRunning,
}

/// Claims the single running copy. On Linux GTK does this over D-Bus, so this always answers `First`.
pub fn claim_instance(name: &str) -> Instance {
    imp::claim_instance(name)
}

/// Blocks until the process `pid` has exited (or 30 seconds pass), so a restarted PodStack doesn't overlap the old
/// one's last writes.
pub fn wait_for_exit(pid: u32) {
    imp::wait_for_exit(pid)
}

/// Gives this process the app id that the Start menu shortcut carries, so the taskbar and notifications show it as
/// PodStack. Nothing to do on Linux.
pub fn set_app_id(app_id: &str) {
    imp::set_app_id(app_id)
}

/// The desktop's accent color as `0xRRGGBB`, where the desktop has one PodStack can read (Windows).
pub fn accent_color() -> Option<u32> {
    imp::accent_color()
}

/// Whether the desktop asks apps for dark mode, where PodStack can read it without GTK (Windows).
pub fn prefers_dark() -> Option<bool> {
    imp::prefers_dark()
}

#[cfg(not(windows))]
mod imp {
    use super::Instance;

    pub fn claim_instance(_name: &str) -> Instance {
        Instance::First { raised: None }
    }

    pub fn wait_for_exit(pid: u32) {
        let proc = std::path::PathBuf::from(format!("/proc/{pid}"));
        for _ in 0..150 {
            if !proc.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    pub fn set_app_id(_app_id: &str) {}

    pub fn accent_color() -> Option<u32> {
        None
    }

    pub fn prefers_dark() -> Option<bool> {
        None
    }
}

#[cfg(windows)]
mod imp {
    use super::Instance;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, WAIT_FAILED};
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateMutexW, OpenProcess, SetEvent, WaitForSingleObject, INFINITE,
    };
    use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

    /// The access right to wait on a process (`SYNCHRONIZE`).
    const SYNCHRONIZE: u32 = 0x0010_0000;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn claim_instance(name: &str) -> Instance {
        // A named mutex marks the running copy (Windows releases it when the process ends, even on a crash), and a
        // named event is how a second launch asks that copy to show itself. "Local\" keeps both per user session.
        let mutex_name = wide(&format!("Local\\{name}.instance"));
        let event_name = wide(&format!("Local\\{name}.raise"));
        unsafe {
            let mutex = CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr());
            let existed = GetLastError() == ERROR_ALREADY_EXISTS;
            let event = CreateEventW(std::ptr::null(), 0, 0, event_name.as_ptr());
            if mutex.is_null() || event.is_null() {
                tracing::warn!("couldn't claim the single instance; running anyway");
                return Instance::First { raised: None };
            }
            if existed {
                SetEvent(event);
                CloseHandle(event);
                CloseHandle(mutex);
                return Instance::AlreadyRunning;
            }
            // The mutex stays open for the life of the process; the event is waited on by a thread of its own.
            let event = event as usize;
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || loop {
                if WaitForSingleObject(event as _, INFINITE) == WAIT_FAILED || tx.send(()).is_err() {
                    break;
                }
            });
            Instance::First { raised: Some(rx) }
        }
    }

    pub fn wait_for_exit(pid: u32) {
        unsafe {
            let process = OpenProcess(SYNCHRONIZE, 0, pid);
            if !process.is_null() {
                WaitForSingleObject(process, 30_000);
                CloseHandle(process);
            }
        }
    }

    pub fn set_app_id(app_id: &str) {
        let id = wide(app_id);
        unsafe {
            SetCurrentProcessExplicitAppUserModelID(id.as_ptr());
        }
    }

    fn dword(key: &str, value: &str) -> Option<u32> {
        let (key, value) = (wide(key), wide(value));
        let mut data: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut data as *mut u32).cast(),
                &mut size,
            )
        };
        (status == 0).then_some(data)
    }

    pub fn accent_color() -> Option<u32> {
        // Stored as 0xAABBGGRR.
        let abgr = dword("Software\\Microsoft\\Windows\\DWM", "AccentColor")?;
        let (r, g, b) = (abgr & 0xff, (abgr >> 8) & 0xff, (abgr >> 16) & 0xff);
        Some((r << 16) | (g << 8) | b)
    }

    pub fn prefers_dark() -> Option<bool> {
        dword("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize", "AppsUseLightTheme").map(|v| v == 0)
    }
}
