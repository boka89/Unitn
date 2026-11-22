//! Windows app identity: the AppUserModelID Windows uses to tell which app a
//! window belongs to. The media overlay (volume keys, Bluetooth headsets),
//! the taskbar and notifications take the app's name and icon from it.
//!
//! The installer's Start menu and desktop shortcuts carry the identifier
//! from Cargo.toml (`package.metadata.packager.identifier`) as their ID, so
//! the app must claim the same one; without it Windows can't link the
//! window to the shortcut and shows no name. For the portable build, which
//! has no shortcut, the ID is also registered with a name and an icon under
//! the user's registry (HKCU, no administrator rights needed).

/// Must match `identifier` in Cargo.toml's packager metadata.
#[cfg_attr(not(windows), allow(dead_code))]
pub const APP_ID: &str = "net.bokors.unitn";

/// Claims the app ID for this process. Call before any window is created.
#[cfg(windows)]
pub fn register(data_dir: &std::path::Path) {
    use windows_sys::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let id = wide(APP_ID);
    // SAFETY: `id` is a NUL-terminated UTF-16 string that outlives the call.
    let hr = unsafe { SetCurrentProcessExplicitAppUserModelID(id.as_ptr()) };
    if hr != 0 {
        unitn_core::log::log(format!("app id: SetCurrentProcessExplicitAppUserModelID failed ({hr:#x})"));
        return;
    }

    // Name and icon for the ID (used when there is no shortcut to read them from).
    let icon = data_dir.join("unitn.png");
    let png: &[u8] = include_bytes!("../icons/256x256.png");
    if std::fs::metadata(&icon).map(|m| m.len() != png.len() as u64).unwrap_or(true) {
        let _ = std::fs::create_dir_all(data_dir);
        let _ = std::fs::write(&icon, png);
    }
    let key = wide(&format!("Software\\Classes\\AppUserModelId\\{APP_ID}"));
    for (name, value) in [("DisplayName", "Unitn".to_string()), ("IconUri", icon.display().to_string())] {
        let (name, value) = (wide(name), wide(&value));
        // SAFETY: all strings are NUL-terminated UTF-16 that outlive the call;
        // the size is the data's length in bytes, terminator included.
        let err = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        };
        if err != 0 {
            unitn_core::log::log(format!("app id: could not register its name/icon (error {err})"));
        }
    }
    unitn_core::log::log(format!("app id: {APP_ID}"));
}

#[cfg(not(windows))]
pub fn register(_data_dir: &std::path::Path) {}
