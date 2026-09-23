use std::ffi::OsStr;
use std::fs::{self, File, Metadata};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Path, PathBuf};

use composenest_application::state_store::TemplateFile;
use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetFileInformationByHandle, GetFinalPathNameByHandleW, OPEN_EXISTING,
};

use super::{invalid, read_capped};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

pub(super) struct SafePackage {
    root_path: PathBuf,
    package_path: PathBuf,
    canonical_root: PathBuf,
    root: File,
    package: File,
    versions: Option<File>,
    files: Vec<(String, FileStamp)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    identity: (u64, u64),
    length: u64,
    changed: u64,
}

impl FileStamp {
    fn of(file: &File) -> io::Result<Self> {
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // SAFETY: the file handle is valid and `info` is writable for this API's output.
        let success =
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) };
        if success == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a successful call initializes every field of BY_HANDLE_FILE_INFORMATION.
        let info = unsafe { info.assume_init() };
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(invalid("package entry is a symlink or junction"));
        }
        Ok(Self {
            identity: (
                u64::from(info.dwVolumeSerialNumber),
                (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            ),
            length: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
            changed: (u64::from(info.ftLastWriteTime.dwHighDateTime) << 32)
                | u64::from(info.ftLastWriteTime.dwLowDateTime),
        })
    }
}

pub(super) fn is_link(metadata: &Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

impl SafePackage {
    pub(super) fn open(root_path: &Path, name: &OsStr) -> io::Result<Self> {
        let root = open_checked(root_path, true)?;
        let canonical_root = fs::canonicalize(root_path)?;
        if !same_windows_path(&final_path(&root)?, &canonical_root) {
            return Err(invalid("root directory changed while opening"));
        }
        let package_path = root_path.join(name);
        let package = open_checked(&package_path, true)?;
        if !same_windows_path(&final_path(&package)?, &canonical_root.join(name)) {
            return Err(invalid("package directory is outside the resource root"));
        }
        Ok(Self {
            root_path: root_path.to_owned(),
            package_path,
            canonical_root,
            root,
            package,
            versions: None,
            files: Vec::new(),
        })
    }

    pub(super) fn read(&mut self, relative: &str, total: &mut usize) -> io::Result<TemplateFile> {
        if relative != "template.yaml" {
            relative
                .strip_prefix("versions/")
                .filter(|name| !name.is_empty() && !name.contains('/'))
                .ok_or_else(|| invalid("invalid version file path"))?;
            if self.versions.is_none() {
                let versions = open_checked(&self.package_path.join("versions"), true)?;
                if !same_windows_path(
                    &final_path(&versions)?,
                    &self.expected_package()?.join("versions"),
                ) {
                    return Err(invalid("versions directory is outside the package"));
                }
                self.versions = Some(versions);
            }
        }
        let mut file = self.open_file(relative)?;
        let stamp = FileStamp::of(&file)?;
        if self
            .files
            .iter()
            .any(|(_, existing)| existing.identity == stamp.identity)
        {
            return Err(invalid("two documents reference the same physical file"));
        }
        let contents = read_capped(&mut file, total)?;
        if stamp != FileStamp::of(&file)? {
            return Err(invalid("file changed while being read; reload the package"));
        }
        self.files.push((relative.to_owned(), stamp));
        Ok(TemplateFile {
            relative_path: relative.to_owned(),
            contents,
        })
    }

    pub(super) fn finish(&self) -> io::Result<()> {
        self.verify_directory(&self.root_path, &self.root, &self.canonical_root)?;
        let expected = self.expected_package()?;
        self.verify_directory(&self.package_path, &self.package, &expected)?;
        if let Some(versions) = &self.versions {
            self.verify_directory(
                &self.package_path.join("versions"),
                versions,
                &expected.join("versions"),
            )?;
        }
        for (relative, stamp) in &self.files {
            if *stamp != FileStamp::of(&self.open_file(relative)?)? {
                return Err(invalid("file changed while being read; reload the package"));
            }
        }
        Ok(())
    }

    fn expected_package(&self) -> io::Result<PathBuf> {
        Ok(self.canonical_root.join(
            self.package_path
                .file_name()
                .ok_or_else(|| invalid("invalid package path"))?,
        ))
    }

    fn verify_directory(&self, path: &Path, original: &File, expected: &Path) -> io::Result<()> {
        let reopened = open_checked(path, true)?;
        if FileStamp::of(original)?.identity != FileStamp::of(&reopened)?.identity
            || !same_windows_path(&final_path(&reopened)?, expected)
        {
            return Err(invalid("package directory changed while being read"));
        }
        Ok(())
    }

    fn open_file(&self, relative: &str) -> io::Result<File> {
        let file = open_checked(&self.package_path.join(relative), false)?;
        if !same_windows_path(
            &final_path(&file)?,
            &self.expected_package()?.join(relative),
        ) {
            return Err(invalid("opened file is outside the package"));
        }
        Ok(file)
    }
}

fn open_checked(path: &Path, directory: bool) -> io::Result<File> {
    let metadata = fs::symlink_metadata(path)?;
    if is_link(&metadata) || metadata.is_dir() != directory || metadata.is_file() == directory {
        return Err(invalid(
            "package entry is a link or has the wrong file type",
        ));
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let flags = FILE_FLAG_OPEN_REPARSE_POINT
        | if directory {
            FILE_FLAG_BACKUP_SEMANTICS
        } else {
            0
        };
    // SAFETY: `wide` is NUL terminated; all pointer arguments are valid for the call.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            flags,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateFileW returned a fresh owned handle.
    let file = unsafe { File::from_raw_handle(handle) };
    FileStamp::of(&file)?;
    Ok(file)
}

fn same_windows_path(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .replace('/', "\\")
        .eq_ignore_ascii_case(&right.to_string_lossy().replace('/', "\\"))
}

fn final_path(file: &File) -> io::Result<PathBuf> {
    let handle = file.as_raw_handle();
    // SAFETY: `handle` is valid while `file` lives; the null call requests the required length.
    let needed = unsafe { GetFinalPathNameByHandleW(handle, std::ptr::null_mut(), 0, 0) };
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut buffer = vec![0u16; needed as usize + 1];
    // SAFETY: `buffer` is writable for the supplied length and `handle` remains valid.
    let written =
        unsafe { GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, 0) };
    if written == 0 || written as usize >= buffer.len() {
        return Err(io::Error::last_os_error());
    }
    Ok(PathBuf::from(
        String::from_utf16(&buffer[..written as usize])
            .map_err(|_| invalid("opened file path is not valid UTF-16"))?,
    ))
}
