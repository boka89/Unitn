//! Things Unitn asks of the operating system: keeping the computer awake
//! while music plays, and opening Unitn when the user logs in.

use unitn_core::log::log;

/// Keeps the computer from going to sleep while it's held (the screen may
/// still turn off). Released when dropped.
#[derive(Default)]
pub struct StayAwake {
    held: bool,
    #[cfg(target_os = "macos")]
    assertion: u32,
    #[cfg(target_os = "linux")]
    inhibitor: Option<std::process::Child>,
}

impl StayAwake {
    /// Holds (`true`) or releases (`false`); does nothing if already so.
    pub fn set(&mut self, on: bool) {
        if on == self.held {
            return;
        }
        self.held = on;
        if on {
            self.hold();
        } else {
            self.release();
        }
    }

    #[cfg(windows)]
    fn hold(&mut self) {
        use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};
        // SAFETY: plain flags; applies to this (the UI) thread until changed.
        unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
        log("power: staying awake while playing");
    }

    #[cfg(windows)]
    fn release(&mut self) {
        use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS};
        // SAFETY: as above; clears the request.
        unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
    }

    #[cfg(target_os = "macos")]
    fn hold(&mut self) {
        use std::ffi::{c_char, c_void, CString};
        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            fn CFStringCreateWithCString(alloc: *const c_void, s: *const c_char, encoding: u32) -> *const c_void;
            fn CFRelease(cf: *const c_void);
        }
        #[link(name = "IOKit", kind = "framework")]
        extern "C" {
            fn IOPMAssertionCreateWithName(kind: *const c_void, level: u32, name: *const c_void, id: *mut u32) -> i32;
        }
        const UTF8: u32 = 0x0800_0100;
        const LEVEL_ON: u32 = 255;
        let kind = CString::new("PreventUserIdleSystemSleep").unwrap();
        let name = CString::new("Unitn is playing music").unwrap();
        // SAFETY: the CFStrings are created from valid C strings and released
        // after the call; `assertion` receives the ID.
        unsafe {
            let k = CFStringCreateWithCString(std::ptr::null(), kind.as_ptr(), UTF8);
            let n = CFStringCreateWithCString(std::ptr::null(), name.as_ptr(), UTF8);
            if k.is_null() || n.is_null() {
                return;
            }
            let result = IOPMAssertionCreateWithName(k, LEVEL_ON, n, &mut self.assertion);
            CFRelease(k);
            CFRelease(n);
            if result != 0 {
                log(format!("power: could not stay awake ({result})"));
                self.assertion = 0;
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn release(&mut self) {
        #[link(name = "IOKit", kind = "framework")]
        extern "C" {
            fn IOPMAssertionRelease(id: u32) -> i32;
        }
        if self.assertion != 0 {
            // SAFETY: releases the assertion created in `hold`.
            unsafe { IOPMAssertionRelease(self.assertion) };
            self.assertion = 0;
        }
    }

    /// Linux: systemd-logind's sleep lock, held by a helper process.
    #[cfg(target_os = "linux")]
    fn hold(&mut self) {
        let child = std::process::Command::new("systemd-inhibit")
            .args(["--what=sleep", "--who=Unitn", "--why=Playing music", "--mode=block", "sleep", "infinity"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match child {
            Ok(c) => self.inhibitor = Some(c),
            Err(e) => log(format!("power: systemd-inhibit not available: {e}")),
        }
    }

    #[cfg(target_os = "linux")]
    fn release(&mut self) {
        if let Some(mut c) = self.inhibitor.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    fn hold(&mut self) {}

    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    fn release(&mut self) {}
}

impl Drop for StayAwake {
    fn drop(&mut self) {
        self.set(false);
    }
}

/// Whether Unitn opens when the user logs in.
pub fn launch_at_login() -> bool {
    login::enabled()
}

/// Turns opening at login on or off. Errors are logged and returned as a
/// short message for the UI.
pub fn set_launch_at_login(on: bool) -> Result<(), String> {
    let result = if on { login::enable() } else { login::disable() };
    result.map_err(|e| {
        log(format!("startup: {e:#}"));
        e.to_string()
    })
}

/// The program to start: the AppImage when running from one, else this
/// executable.
#[cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]
fn program() -> std::io::Result<std::path::PathBuf> {
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        return Ok(appimage.into());
    }
    std::env::current_exe()
}

#[cfg(windows)]
mod login {
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
    };

    const KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const NAME: &str = "Unitn";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn enabled() -> bool {
        let (key, name) = (wide(KEY), wide(NAME));
        let mut size = 0u32;
        // SAFETY: NUL-terminated strings; only asks for the value's size.
        let err = unsafe {
            RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut size)
        };
        err == 0
    }

    pub fn enable() -> anyhow::Result<()> {
        let exe = super::program()?;
        let value = wide(&format!("\"{}\"", exe.display()));
        let (key, name) = (wide(KEY), wide(NAME));
        // SAFETY: NUL-terminated UTF-16 strings that outlive the call; size
        // in bytes including the terminator.
        let err = unsafe {
            RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), REG_SZ, value.as_ptr().cast(), (value.len() * 2) as u32)
        };
        anyhow::ensure!(err == 0, "could not add Unitn to the programs started at login (error {err})");
        Ok(())
    }

    pub fn disable() -> anyhow::Result<()> {
        let (key, name) = (wide(KEY), wide(NAME));
        // SAFETY: NUL-terminated UTF-16 strings that outlive the call.
        let err = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
        // 2: it wasn't there.
        anyhow::ensure!(err == 0 || err == 2, "could not remove Unitn from the programs started at login (error {err})");
        Ok(())
    }
}

/// macOS: a LaunchAgent in ~/Library/LaunchAgents.
#[cfg(target_os = "macos")]
mod login {
    use std::path::PathBuf;

    fn file() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/LaunchAgents/net.bokors.unitn.plist"))
    }

    pub fn enabled() -> bool {
        file().is_some_and(|f| f.is_file())
    }

    pub fn enable() -> anyhow::Result<()> {
        let file = file().ok_or_else(|| anyhow::anyhow!("no home folder"))?;
        let exe = super::program()?;
        // Inside Unitn.app: open the app (one Dock icon), else the program.
        let args: Vec<String> = match exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")) {
            Some(app) => vec!["/usr/bin/open".into(), app.display().to_string()],
            None => vec![exe.display().to_string()],
        };
        let args: String = args.iter().map(|a| format!("        <string>{}</string>\n", xml(a))).collect();
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n    <key>Label</key>\n    <string>net.bokors.unitn</string>\n\
             \x20   <key>ProgramArguments</key>\n    <array>\n{args}    </array>\n    <key>RunAtLoad</key>\n    <true/>\n</dict>\n</plist>\n"
        );
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&file, plist)?;
        Ok(())
    }

    pub fn disable() -> anyhow::Result<()> {
        match file() {
            Some(f) if f.exists() => Ok(std::fs::remove_file(f)?),
            _ => Ok(()),
        }
    }

    fn xml(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }
}

/// Linux and other Unix desktops: an XDG autostart entry.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
mod login {
    use std::path::PathBuf;

    fn file() -> Option<PathBuf> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(config.join("autostart/unitn.desktop"))
    }

    pub fn enabled() -> bool {
        file().is_some_and(|f| f.is_file())
    }

    pub fn enable() -> anyhow::Result<()> {
        let file = file().ok_or_else(|| anyhow::anyhow!("no home folder"))?;
        let exe = super::program()?;
        let quoted = format!("\"{}\"", exe.display().to_string().replace('\\', "\\\\").replace('"', "\\\""));
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName=Unitn\nComment=Music player\nExec={quoted}\nIcon=unitn\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        );
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&file, entry)?;
        Ok(())
    }

    pub fn disable() -> anyhow::Result<()> {
        match file() {
            Some(f) if f.exists() => Ok(std::fs::remove_file(f)?),
            _ => Ok(()),
        }
    }
}

#[cfg(target_os = "android")]
mod login {
    pub fn enabled() -> bool {
        false
    }
    pub fn enable() -> anyhow::Result<()> {
        anyhow::bail!("not on Android")
    }
    pub fn disable() -> anyhow::Result<()> {
        Ok(())
    }
}
