//! "Launch at startup" — register the app to run at Windows login.
//!
//! Uses the per-user `Run` key
//! (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`), so no admin
//! rights are needed. The registry is the single source of truth for this
//! setting — it is queried on launch and written the moment the toggle
//! changes, rather than being mirrored into `config.json`.

/// Registry value name under the `Run` key. Uniquely identifies our entry.
#[cfg(windows)]
const VALUE_NAME: &str = "ClaudeTimerReset";

#[cfg(windows)]
const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

// ── Windows implementation ───────────────────────────────────────────────────

#[cfg(windows)]
mod imp {
    use super::{RUN_SUBKEY, VALUE_NAME};
    use std::ptr;
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
    };

    /// UTF-16, null-terminated — the encoding every `*W` Win32 API expects.
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Open the `Run` key with the given access rights. Caller must
    /// `RegCloseKey` the returned handle.
    fn open_run_key(access: u32) -> Result<HKEY, String> {
        let subkey = wide(RUN_SUBKEY);
        let mut hkey: HKEY = ptr::null_mut();
        // SAFETY: valid null-terminated subkey; hkey is a live out-param.
        // The 3rd arg (ulOptions) is reserved for RegOpenKeyExW and must be 0.
        let status = unsafe {
            RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, access, &mut hkey)
        };
        if status == ERROR_SUCCESS {
            Ok(hkey)
        } else {
            Err(format!("RegOpenKeyExW failed (code {status})"))
        }
    }

    /// Absolute path to the running executable, wrapped in quotes so a path
    /// containing spaces survives the shell that Windows uses at login.
    fn quoted_exe_path() -> Result<String, String> {
        let exe = std::env::current_exe()
            .map_err(|e| format!("current_exe failed: {e}"))?;
        Ok(format!("\"{}\"", exe.display()))
    }

    pub fn is_enabled() -> bool {
        let Ok(hkey) = open_run_key(KEY_QUERY_VALUE) else {
            return false;
        };
        let name = wide(VALUE_NAME);
        // Query with a null data buffer — we only care whether the value
        // exists, not its contents.
        // SAFETY: hkey is valid; name is null-terminated; all data out-params null.
        let status = unsafe {
            RegQueryValueExW(
                hkey,
                name.as_ptr(),
                ptr::null(), // lpReserved is *const u32
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        // SAFETY: hkey came from a successful open.
        unsafe { RegCloseKey(hkey) };
        status == ERROR_SUCCESS
    }

    pub fn set_enabled(enable: bool) -> Result<(), String> {
        if enable {
            enable_entry()
        } else {
            disable_entry()
        }
    }

    fn enable_entry() -> Result<(), String> {
        let value = quoted_exe_path()?;
        let hkey = open_run_key(KEY_SET_VALUE)?;
        let name = wide(VALUE_NAME);
        let data = wide(&value);
        // REG_SZ byte count includes the trailing null (u16 → 2 bytes each).
        let cb = (data.len() * std::mem::size_of::<u16>()) as u32;
        // SAFETY: hkey valid; name null-terminated; data points to `cb` valid bytes.
        let status = unsafe {
            RegSetValueExW(
                hkey,
                name.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr() as *const u8,
                cb,
            )
        };
        // SAFETY: hkey came from a successful open.
        unsafe { RegCloseKey(hkey) };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("RegSetValueExW failed (code {status})"))
        }
    }

    fn disable_entry() -> Result<(), String> {
        let hkey = open_run_key(KEY_SET_VALUE)?;
        let name = wide(VALUE_NAME);
        // SAFETY: hkey valid; name null-terminated.
        let status = unsafe { RegDeleteValueW(hkey, name.as_ptr()) };
        // SAFETY: hkey came from a successful open.
        unsafe { RegCloseKey(hkey) };
        // A missing value means we were already disabled — treat as success.
        if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!("RegDeleteValueW failed (code {status})"))
        }
    }
}

// ── Non-Windows stubs ────────────────────────────────────────────────────────
//
// The feature is Windows-only; these keep the app compiling on macOS.

#[cfg(not(windows))]
mod imp {
    pub fn is_enabled() -> bool {
        false
    }

    pub fn set_enabled(_enable: bool) -> Result<(), String> {
        Err("Launch at startup is only supported on Windows".into())
    }
}

/// Whether the app is currently registered to launch at login.
pub fn is_enabled() -> bool {
    imp::is_enabled()
}

/// Register (`true`) or unregister (`false`) the app for launch at login.
pub fn set_enabled(enable: bool) -> Result<(), String> {
    imp::set_enabled(enable)
}
