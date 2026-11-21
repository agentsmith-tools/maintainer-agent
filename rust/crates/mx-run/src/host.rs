//! The real answers to what a backend asks of the machine: `command -v`, `[ -f ]`
//! and reading a file.

use mx_backends::Host;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Answers from the file system and from a search path.
#[derive(Debug, Clone)]
pub struct RealHost {
    search_dirs: Vec<PathBuf>,
}

impl RealHost {
    /// A host that resolves programs against `search_path`. A relative or empty
    /// entry is skipped, as in the process adapters.
    #[must_use]
    pub fn new(search_path: &OsStr) -> Self {
        Self {
            search_dirs: std::env::split_paths(search_path)
                .filter(|dir| dir.is_absolute())
                .collect(),
        }
    }
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

impl Host for RealHost {
    fn on_path(&self, program: &str) -> bool {
        if program.contains('/') {
            return is_executable_file(Path::new(program));
        }
        self.search_dirs.iter().any(|dir| is_executable_file(&dir.join(program)))
    }

    fn is_file(&self, path: &Path) -> bool {
        std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
    }

    fn file_len(&self, path: &Path) -> Option<u64> {
        std::fs::metadata(path).ok().map(|meta| meta.len())
    }

    fn read(&self, path: &Path) -> Option<Vec<u8>> {
        std::fs::read(path).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mx-run-host-{tag}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    #[test]
    fn a_program_is_on_the_path_only_when_an_executable_file_of_that_name_is() {
        let dir = scratch("path");
        std::fs::write(dir.join("tool"), "#!/bin/sh\n").unwrap();
        std::fs::write(dir.join("plain"), "data").unwrap();
        std::fs::set_permissions(dir.join("tool"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(dir.join("plain"), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        std::fs::create_dir(dir.join("subdir")).unwrap();
        let host = RealHost::new(dir.as_os_str());
        assert!(host.on_path("tool"));
        assert!(!host.on_path("plain"), "a file without the execute bit is not a command");
        assert!(!host.on_path("subdir"), "a directory is not a command");
        assert!(!host.on_path("absent"));
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn any_execute_bit_makes_a_command_not_only_the_owners() {
        let dir = scratch("xbit");
        for (name, mode) in [("group", 0o010), ("other", 0o001), ("owner", 0o100), ("none", 0o600)]
        {
            std::fs::write(dir.join(name), "").unwrap();
            std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(mode))
                .unwrap();
        }
        let host = RealHost::new(dir.as_os_str());
        assert!(host.on_path("group") && host.on_path("other") && host.on_path("owner"));
        assert!(!host.on_path("none"));
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn a_relative_search_path_entry_is_not_searched() {
        let host = RealHost::new(OsStr::new(".:relative/dir"));
        assert!(!host.on_path("sh"));
    }

    #[test]
    fn file_questions_read_the_file_system() {
        let dir = scratch("files");
        std::fs::write(dir.join("f"), "12345").unwrap();
        let host = RealHost::new(OsStr::new("/usr/bin"));
        assert!(host.is_file(&dir.join("f")));
        assert!(!host.is_file(&dir));
        assert_eq!(host.file_len(&dir.join("f")), Some(5));
        assert_eq!(host.file_len(&dir.join("g")), None);
        assert_eq!(host.read(&dir.join("f")), Some(b"12345".to_vec()));
        assert_eq!(host.read(&dir.join("g")), None);
        drop(std::fs::remove_dir_all(&dir));
    }
}
