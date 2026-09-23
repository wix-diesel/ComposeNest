use std::ffi::{CString, OsStr};
use std::fs::{self, File, Metadata};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use composenest_application::state_store::TemplateFile;

use super::{invalid, read_capped};

pub(super) struct SafePackage {
    root: File,
    package: File,
    versions: Option<File>,
    root_path: PathBuf,
    package_path: PathBuf,
    files: Vec<(String, FileStamp)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    identity: (u64, u64),
    length: u64,
    changed: (i64, i64),
    modified: (i64, i64),
}

impl FileStamp {
    fn of(metadata: &Metadata) -> Self {
        Self {
            identity: (metadata.dev(), metadata.ino()),
            length: metadata.len(),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
        }
    }
}

pub(super) fn is_link(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink()
}

impl SafePackage {
    pub(super) fn open(root_path: &Path, name: &OsStr) -> io::Result<Self> {
        let root = open_path(root_path, libc::O_DIRECTORY)?;
        let package = open_at(&root, name, libc::O_DIRECTORY)?;
        let package_path = root_path.join(name);
        verify_directory(root_path, &root)?;
        verify_directory(&package_path, &package)?;
        Ok(Self {
            root,
            package,
            versions: None,
            root_path: root_path.to_owned(),
            package_path,
            files: Vec::new(),
        })
    }

    pub(super) fn read(&mut self, relative: &str, total: &mut usize) -> io::Result<TemplateFile> {
        let (directory, name) = if relative == "template.yaml" {
            (&self.package, "template.yaml")
        } else {
            let name = relative
                .strip_prefix("versions/")
                .filter(|name| !name.contains('/'))
                .ok_or_else(|| invalid("invalid version file path"))?;
            if self.versions.is_none() {
                let versions = open_at(&self.package, OsStr::new("versions"), libc::O_DIRECTORY)?;
                verify_directory(&self.package_path.join("versions"), &versions)?;
                self.versions = Some(versions);
            }
            (
                self.versions
                    .as_ref()
                    .ok_or_else(|| invalid("versions directory missing"))?,
                name,
            )
        };
        let mut file = open_at(directory, OsStr::new(name), libc::O_NONBLOCK)?;
        let before = file.metadata()?;
        if !before.is_file() {
            return Err(invalid("referenced entry is not a regular file"));
        }
        let stamp = FileStamp::of(&before);
        if self
            .files
            .iter()
            .any(|(_, existing)| existing.identity == stamp.identity)
        {
            return Err(invalid("two documents reference the same physical file"));
        }
        let contents = read_capped(&mut file, total)?;
        if stamp != FileStamp::of(&file.metadata()?) {
            return Err(invalid("file changed while being read; reload the package"));
        }
        self.files.push((relative.to_owned(), stamp));
        Ok(TemplateFile {
            relative_path: relative.to_owned(),
            contents,
        })
    }

    pub(super) fn finish(&self) -> io::Result<()> {
        verify_directory(&self.root_path, &self.root)?;
        verify_directory(&self.package_path, &self.package)?;
        if let Some(versions) = &self.versions {
            verify_directory(&self.package_path.join("versions"), versions)?;
        }
        for (relative, stamp) in &self.files {
            let (directory, name) = if relative == "template.yaml" {
                (&self.package, "template.yaml")
            } else {
                (
                    self.versions
                        .as_ref()
                        .ok_or_else(|| invalid("versions directory missing"))?,
                    relative
                        .strip_prefix("versions/")
                        .ok_or_else(|| invalid("invalid path"))?,
                )
            };
            let reopened = open_at(directory, OsStr::new(name), libc::O_NONBLOCK)?;
            if *stamp != FileStamp::of(&reopened.metadata()?) {
                return Err(invalid("file changed while being read; reload the package"));
            }
        }
        Ok(())
    }
}

fn verify_directory(path: &Path, opened: &File) -> io::Result<()> {
    let current = fs::symlink_metadata(path)?;
    if is_link(&current)
        || !current.is_dir()
        || FileStamp::of(&current).identity != FileStamp::of(&opened.metadata()?).identity
    {
        return Err(invalid("package directory changed or is a link"));
    }
    Ok(())
}

fn open_path(path: &Path, flags: i32) -> io::Result<File> {
    let name = CString::new(path.as_os_str().as_bytes()).map_err(|_| invalid("NUL in path"))?;
    // SAFETY: `name` is NUL terminated; a nonnegative descriptor becomes owned by File.
    let fd = unsafe {
        libc::open(
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | flags,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `open` returned a fresh descriptor whose ownership transfers to File.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn open_at(directory: &File, name: &OsStr, flags: i32) -> io::Result<File> {
    let name = CString::new(name.as_bytes()).map_err(|_| invalid("NUL in path"))?;
    // SAFETY: the directory descriptor stays open and `name` is NUL terminated.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | flags,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `openat` returned a fresh descriptor whose ownership transfers to File.
    Ok(unsafe { File::from_raw_fd(fd) })
}
