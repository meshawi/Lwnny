//! Administrator rights and start-with-Windows.
//!
//! The RAM sticks sit on the SMBus, which only a kernel driver can reach, so
//! Lwnny asks for elevation at start. An elevated program cannot be started
//! from the registry "Run" key (Windows silently skips it), so autostart is a
//! scheduled task with highest privileges instead.

use anyhow::{bail, Result};
use std::process::Command;

const TASK_NAME: &str = "Lwnny";

#[cfg(windows)]
pub fn is_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elev = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elev as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        ok != 0 && elev.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    true
}

/// Re-launch this executable elevated (UAC prompt). Returns Ok(true) when the
/// elevated copy started and the caller should exit, Ok(false) when the user
/// declined.
#[cfg(windows)]
pub fn relaunch_elevated() -> Result<bool> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let exe = std::env::current_exe()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let verb = wide("runas");
    let file = wide(&exe.to_string_lossy());
    let params = wide(&args.join(" "));
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), params.as_ptr(), std::ptr::null(), SW_SHOWNORMAL) };
    // ShellExecute returns a value > 32 on success.
    Ok(r as usize > 32)
}

#[cfg(not(windows))]
pub fn relaunch_elevated() -> Result<bool> {
    Ok(false)
}

fn schtasks(args: &[&str]) -> Result<String> {
    let out = Command::new("schtasks").args(args).output()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        bail!("schtasks failed: {}", text.trim());
    }
    Ok(text)
}

pub fn autostart_enabled() -> bool {
    schtasks(&["/Query", "/TN", TASK_NAME]).is_ok()
}

pub fn set_autostart(enabled: bool) -> Result<bool> {
    if enabled {
        let exe = std::env::current_exe()?;
        let tr = format!("\"{}\" --minimized", exe.to_string_lossy());
        schtasks(&["/Create", "/F", "/TN", TASK_NAME, "/SC", "ONLOGON", "/RL", "HIGHEST", "/IT", "/TR", &tr])?;
    } else if autostart_enabled() {
        schtasks(&["/Delete", "/F", "/TN", TASK_NAME])?;
    }
    Ok(autostart_enabled())
}
