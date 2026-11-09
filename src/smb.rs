//! SMB support through the native Windows networking stack (WNet / NetApi).
//!
//! Once a share is connected, plain UNC paths (`\\server\share\...`) work with `std::fs`,
//! the shell thumbnail cache and "open with default app" — no custom protocol code needed.

use windows::Win32::Foundation::{ERROR_SESSION_CREDENTIAL_CONFLICT, NO_ERROR};
use windows::Win32::NetworkManagement::NetManagement::{MAX_PREFERRED_LENGTH, NetApiBufferFree};
use windows::Win32::NetworkManagement::WNet::{
    CONNECT_INTERACTIVE, CONNECT_TEMPORARY, NET_CONNECT_FLAGS, NETRESOURCEW, RESOURCETYPE_DISK,
    WNetAddConnection2W,
};
use windows::Win32::Storage::FileSystem::{NetShareEnum, SHARE_INFO_1, STYPE_DISKTREE};
use windows::core::{HSTRING, PCWSTR, PWSTR};

/// Normalizes user input like `nas`, `\\nas`, `smb://nas/share` into `\\nas` / `\\nas\share`.
pub fn normalize_unc(input: &str) -> String {
    let s = input.trim().trim_start_matches("smb:").replace('/', "\\");
    let s = s.trim_start_matches('\\').trim_end_matches('\\');
    format!("\\\\{s}")
}

/// Connects to `\\server\share`. With empty credentials the current Windows login (or saved
/// credentials) is tried first, then Windows' own credential prompt is shown.
pub fn connect(unc: &str, user: &str, password: &str) -> Result<(), String> {
    let remote: Vec<u16> = unc.encode_utf16().chain(Some(0)).collect();
    let res = NETRESOURCEW {
        dwType: RESOURCETYPE_DISK,
        lpRemoteName: PWSTR(remote.as_ptr() as *mut u16),
        ..Default::default()
    };
    let call = |user: Option<&str>, pass: Option<&str>, flags: NET_CONNECT_FLAGS| unsafe {
        let u = user.map(HSTRING::from);
        let p = pass.map(HSTRING::from);
        WNetAddConnection2W(
            &res,
            p.as_ref().map(|h| PCWSTR(h.as_ptr())).unwrap_or(PCWSTR::null()),
            u.as_ref().map(|h| PCWSTR(h.as_ptr())).unwrap_or(PCWSTR::null()),
            flags,
        )
    };

    let err = if user.is_empty() && password.is_empty() {
        let e = call(None, None, CONNECT_TEMPORARY);
        if e == NO_ERROR { e } else { call(None, None, CONNECT_TEMPORARY | CONNECT_INTERACTIVE) }
    } else {
        call(Some(user), Some(password), CONNECT_TEMPORARY)
    };

    if err == NO_ERROR || err == ERROR_SESSION_CREDENTIAL_CONFLICT {
        // A credential conflict means we are already connected with other credentials: usable.
        Ok(())
    } else {
        Err(windows::core::Error::from_hresult(err.to_hresult()).message())
    }
}

/// Lists the disk shares a server exposes (`\\server`).
pub fn list_shares(server: &str) -> Result<Vec<String>, String> {
    let server = HSTRING::from(server);
    let mut buf: *mut u8 = std::ptr::null_mut();
    let mut read = 0u32;
    let mut total = 0u32;
    let status = unsafe {
        NetShareEnum(&server, 1, &mut buf, MAX_PREFERRED_LENGTH, &mut read, &mut total, None)
    };
    if status != 0 {
        return Err(windows::core::Error::from_hresult(
            windows::Win32::Foundation::WIN32_ERROR(status).to_hresult(),
        )
        .message());
    }
    let mut out = Vec::new();
    unsafe {
        let items = std::slice::from_raw_parts(buf as *const SHARE_INFO_1, read as usize);
        for it in items {
            // Strip the special bit (admin shares like C$, IPC$).
            if it.shi1_type.0 & 0xFF == STYPE_DISKTREE.0 && (it.shi1_type.0 & 0x8000_0000) == 0 {
                if let Ok(name) = it.shi1_netname.to_string() {
                    out.push(name);
                }
            }
        }
        NetApiBufferFree(Some(buf as *const _));
    }
    out.sort_by_key(|s| s.to_lowercase());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize() {
        assert_eq!(normalize_unc("nas"), r"\\nas");
        assert_eq!(normalize_unc("smb://nas/share/"), r"\\nas\share");
        assert_eq!(normalize_unc(r"\\nas\share"), r"\\nas\share");
    }

    /// Needs a reachable SMB server; run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn localhost_admin_share() {
        list_shares(r"\\localhost").expect("list shares");
        connect(r"\\localhost\C$", "", "").expect("connect");
        assert!(std::fs::read_dir(r"\\localhost\C$\Windows").is_ok());
    }
}
