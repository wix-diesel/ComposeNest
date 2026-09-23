//! Windows management root discovery for the host adapter.

use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramData, SHGetKnownFolderPath};

/// Resolves the management root from the operating system's ProgramData known folder.
///
/// This function only discovers a path. Initial setup must grant the selected user
/// access before the normal GUI creates or opens any managed files.
pub fn management_root() -> io::Result<PathBuf> {
    let mut raw_path = std::ptr::null_mut();
    // SAFETY: The known-folder ID and output pointer are valid. The API allocates
    // the returned UTF-16 buffer, which is released with CoTaskMemFree below.
    let result = unsafe {
        SHGetKnownFolderPath(
            &FOLDERID_ProgramData,
            0,
            std::ptr::null_mut(),
            &mut raw_path,
        )
    };
    if result < 0 {
        return Err(io::Error::other(format!(
            "Could not resolve the ProgramData known folder (HRESULT 0x{:08X})",
            result as u32
        )));
    }
    if raw_path.is_null() {
        return Err(io::Error::other("ProgramData returned an empty path"));
    }

    // SAFETY: A successful SHGetKnownFolderPath returns a NUL-terminated buffer.
    // The length scan stays within that allocation, then CoTaskMemFree releases it.
    let path = unsafe {
        let mut length = 0;
        while *raw_path.add(length) != 0 {
            length += 1;
        }
        let path = std::ffi::OsString::from_wide(std::slice::from_raw_parts(raw_path, length));
        CoTaskMemFree(raw_path.cast::<c_void>());
        path
    };
    if path.is_empty() {
        return Err(io::Error::other("ProgramData returned an empty path"));
    }
    Ok(PathBuf::from(path).join("ComposeNest"))
}

#[cfg(test)]
mod tests {
    use super::management_root;

    #[test]
    fn root_is_under_program_data_known_folder() {
        let root = management_root().expect("ProgramData must exist on Windows");
        assert_eq!(root.file_name().unwrap(), "ComposeNest");
        assert!(root.is_absolute());
    }
}
