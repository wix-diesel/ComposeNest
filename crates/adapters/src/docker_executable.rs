//! Backend-only discovery of Docker CLI installations for desktop launchers.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

/// Finds an executable Docker CLI in absolute PATH entries or standard install locations.
/// GUI launchers may omit Docker Desktop's directories from PATH.
pub fn discover(home: &Path) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH");
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    find_executable(candidates(
        paths.as_deref(),
        home,
        std::env::consts::OS,
        program_files.as_deref(),
    ))
}

fn candidates(
    paths: Option<&OsStr>,
    home: &Path,
    os: &str,
    program_files: Option<&Path>,
) -> Vec<PathBuf> {
    let filename = if os == "windows" {
        "docker.exe"
    } else {
        "docker"
    };
    let mut candidates = paths
        .into_iter()
        .flat_map(std::env::split_paths)
        .filter(|path| path.is_absolute())
        .map(|path| path.join(filename))
        .collect::<Vec<_>>();
    match os {
        "macos" => candidates.extend([
            home.join(".docker/bin/docker"),
            PathBuf::from("/usr/local/bin/docker"),
            PathBuf::from("/opt/homebrew/bin/docker"),
            PathBuf::from("/Applications/Docker.app/Contents/Resources/bin/docker"),
        ]),
        "windows" => candidates.push(
            program_files
                .unwrap_or_else(|| Path::new(r"C:\Program Files"))
                .join("Docker/Docker/resources/bin/docker.exe"),
        ),
        _ => candidates.extend([
            PathBuf::from("/usr/local/bin/docker"),
            PathBuf::from("/usr/bin/docker"),
        ]),
    }
    candidates
}

fn find_executable(candidates: Vec<PathBuf>) -> Option<PathBuf> {
    candidates
        .into_iter()
        .filter(|path| path.is_absolute())
        .find_map(|path| {
            let resolved = path.canonicalize().ok()?;
            let metadata = resolved.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o111 == 0 {
                    return None;
                }
            }
            Some(resolved)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn executable(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "fake docker").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    #[test]
    fn macos_gui_path_finds_user_install_and_lists_system_installations() {
        let home = tempfile::tempdir().unwrap();
        let docker = home.path().join(".docker/bin/docker");
        executable(&docker);
        let gui_path = std::env::join_paths([home.path().join("usr/bin")]).unwrap();
        let candidates = candidates(Some(&gui_path), home.path(), "macos", None);
        assert!(candidates.contains(&PathBuf::from("/usr/local/bin/docker")));
        assert!(candidates.contains(&PathBuf::from(
            "/Applications/Docker.app/Contents/Resources/bin/docker"
        )));
        assert_eq!(
            find_executable(candidates),
            Some(docker.canonicalize().unwrap())
        );
    }

    #[test]
    fn absolute_path_precedes_fallback_and_relative_path_is_ignored() {
        let home = tempfile::tempdir().unwrap();
        let path_docker = home.path().join("bin/docker");
        executable(&path_docker);
        executable(&home.path().join(".docker/bin/docker"));
        let paths =
            std::env::join_paths([PathBuf::from("relative"), home.path().join("bin")]).unwrap();
        let candidates = candidates(Some(&paths), home.path(), "macos", None);
        assert!(!candidates.contains(&PathBuf::from("relative/docker")));
        assert_eq!(
            find_executable(candidates),
            Some(path_docker.canonicalize().unwrap())
        );
    }

    #[test]
    fn windows_and_linux_have_standard_backend_candidates() {
        let home = Path::new("/home/test");
        let windows = candidates(None, home, "windows", Some(Path::new("D:/Programs")));
        assert_eq!(
            windows,
            [PathBuf::from(
                "D:/Programs/Docker/Docker/resources/bin/docker.exe"
            )]
        );
        let linux = candidates(None, home, "linux", None);
        assert!(linux.contains(&PathBuf::from("/usr/bin/docker")));
    }

    #[test]
    fn unusable_candidate_does_not_hide_a_later_executable() {
        let home = tempfile::tempdir().unwrap();
        let docker = home.path().join("docker");
        executable(&docker);
        assert_eq!(
            find_executable(vec![
                home.path().to_owned(),
                home.path().join("missing"),
                docker.clone()
            ]),
            Some(docker.canonicalize().unwrap())
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let unusable = home.path().join("not-executable");
            fs::write(&unusable, "fake").unwrap();
            fs::set_permissions(&unusable, fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(
                find_executable(vec![unusable, docker.clone()]),
                Some(docker.canonicalize().unwrap())
            );
        }
    }
}
