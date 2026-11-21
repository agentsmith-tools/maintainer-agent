//! Finding the tree the program was installed into.
//!
//! In the repository `run.sh` is `lib/run.sh`, so `profiles/` lives one level up;
//! installed, it sits beside `profiles/`. Checking both is what makes
//! `./install.sh` and a git checkout behave the same, and getting it wrong is
//! silent: the unit exits 64 and the timer logs nothing.
//!
//! The location is a logical path, the way `cd "$(dirname "$0")" && pwd` makes
//! one: relative paths are joined to the working directory, `.` and `..` are
//! resolved by name, and symbolic links are not followed.

use std::path::{Component, Path, PathBuf};

/// The directories a run reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    /// Where the program sits.
    pub here: PathBuf,
    /// The directory that holds `profiles/`.
    pub root: PathBuf,
    /// Where the backends are.
    pub backend_dir: PathBuf,
    /// `profile.sh`, which holds the shared helpers.
    pub profile_lib: PathBuf,
}

/// Why the tree is unusable. Each is exit 64.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeError {
    NoProfiles { here: PathBuf },
    NoBackends { root: PathBuf },
    NoProfileLib { root: PathBuf },
}

impl TreeError {
    /// The line the bash prints on standard error, with its newline.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::NoProfiles { here } => format!(
                "run.sh: cannot find a profiles/ directory beside or above {}\n",
                here.display()
            ),
            Self::NoBackends { root } => {
                format!("run.sh: cannot find a backends/ directory under {}\n", root.display())
            }
            Self::NoProfileLib { root } => {
                format!("run.sh: cannot find profile.sh beside or under {}\n", root.display())
            }
        }
    }
}

/// `cd "$dir" && pwd`: an absolute path, with `.` and `..` resolved by name.
#[must_use]
pub fn logical(dir: &Path, cwd: &Path) -> PathBuf {
    let joined = if dir.is_absolute() { dir.to_path_buf() } else { cwd.join(dir) };
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() { PathBuf::from("/") } else { out }
}

fn is_dir(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_dir())
}

fn is_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
}

/// Locates the tree from the path of the program.
pub fn locate(program: &Path, cwd: &Path) -> Result<Tree, TreeError> {
    let here = logical(program.parent().unwrap_or_else(|| Path::new(".")), cwd);
    let root = if is_dir(&here.join("profiles")) {
        here.clone()
    } else if is_dir(&here.join("../profiles")) {
        logical(&here.join(".."), cwd)
    } else {
        return Err(TreeError::NoProfiles { here });
    };
    let backend_dir = if is_dir(&root.join("backends")) {
        root.join("backends")
    } else if is_dir(&root.join("lib/backends")) {
        root.join("lib/backends")
    } else {
        return Err(TreeError::NoBackends { root });
    };
    let profile_lib = if is_file(&root.join("lib/profile.sh")) {
        root.join("lib/profile.sh")
    } else if is_file(&root.join("profile.sh")) {
        root.join("profile.sh")
    } else {
        return Err(TreeError::NoProfileLib { root });
    };
    Ok(Tree { here, root, backend_dir, profile_lib })
}

/// `libfile`: a file of the library, under `lib/` in the repository and beside
/// the program once installed.
#[must_use]
pub fn libfile(root: &Path, name: &str) -> Option<PathBuf> {
    let under_lib = root.join("lib").join(name);
    if is_file(&under_lib) {
        return Some(under_lib);
    }
    let beside = root.join(name);
    is_file(&beside).then_some(beside)
}

/// Whether `profile.sh` defines the function that scrubs the pinned token. The
/// bash sources the file and asks `declare -F`; here the text is searched for a
/// definition line, because the program does not run shell code of its own.
#[must_use]
pub fn defines_scrub_function(profile_lib_text: &[u8]) -> bool {
    const NAME: &str = "maintainer_scrub_secret";
    String::from_utf8_lossy(profile_lib_text).lines().any(|line| {
        let line = line.trim_start();
        let (keyword, line) = match line.strip_prefix("function ") {
            Some(after) => (true, after.trim_start()),
            None => (false, line),
        };
        let Some(rest) = line.strip_prefix(NAME) else {
            return false;
        };
        let rest = rest.trim_start();
        rest.starts_with('(') || (keyword && (rest.is_empty() || rest.starts_with('{')))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mx-run-tree-{tag}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    #[test]
    fn the_repository_layout_finds_profiles_one_level_up() {
        let root = scratch("repo");
        std::fs::create_dir_all(root.join("profiles")).unwrap();
        std::fs::create_dir_all(root.join("lib/backends")).unwrap();
        touch(&root.join("lib/profile.sh"));
        let tree = locate(&root.join("lib/run.sh"), Path::new("/")).unwrap();
        assert_eq!(tree.here, root.join("lib"));
        assert_eq!(tree.root, root);
        assert_eq!(tree.backend_dir, root.join("lib/backends"));
        assert_eq!(tree.profile_lib, root.join("lib/profile.sh"));
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn the_installed_layout_finds_everything_beside_profiles() {
        let root = scratch("installed");
        std::fs::create_dir_all(root.join("profiles")).unwrap();
        std::fs::create_dir_all(root.join("backends")).unwrap();
        touch(&root.join("profile.sh"));
        let tree = locate(&root.join("run.sh"), Path::new("/")).unwrap();
        assert_eq!(tree.here, root);
        assert_eq!(tree.root, root);
        assert_eq!(tree.backend_dir, root.join("backends"));
        assert_eq!(tree.profile_lib, root.join("profile.sh"));
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn no_profiles_directory_names_where_it_looked() {
        let root = scratch("none");
        let error = locate(&root.join("lonely/run.sh"), Path::new("/")).unwrap_err();
        assert_eq!(
            error.message(),
            format!(
                "run.sh: cannot find a profiles/ directory beside or above {}/lonely\n",
                root.display()
            )
        );
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn a_missing_backends_directory_and_a_missing_profile_library_are_each_named() {
        let root = scratch("partial");
        std::fs::create_dir_all(root.join("profiles")).unwrap();
        let error = locate(&root.join("run.sh"), Path::new("/")).unwrap_err();
        assert_eq!(
            error.message(),
            format!("run.sh: cannot find a backends/ directory under {}\n", root.display())
        );
        std::fs::create_dir_all(root.join("backends")).unwrap();
        let error = locate(&root.join("run.sh"), Path::new("/")).unwrap_err();
        assert_eq!(
            error.message(),
            format!("run.sh: cannot find profile.sh beside or under {}\n", root.display())
        );
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn a_relative_program_path_is_joined_to_the_working_directory_by_name() {
        assert_eq!(logical(Path::new("a/../b/./c"), Path::new("/w")), PathBuf::from("/w/b/c"));
        assert_eq!(logical(Path::new("/x/y/.."), Path::new("/w")), PathBuf::from("/x"));
        assert_eq!(logical(Path::new("../.."), Path::new("/w")), PathBuf::from("/"));
    }

    #[test]
    fn a_library_file_is_found_under_lib_first_and_beside_the_program_second() {
        let root = scratch("libfile");
        touch(&root.join("preamble-core.md"));
        assert_eq!(libfile(&root, "preamble-core.md"), Some(root.join("preamble-core.md")));
        touch(&root.join("lib/preamble-core.md"));
        assert_eq!(libfile(&root, "preamble-core.md"), Some(root.join("lib/preamble-core.md")));
        assert_eq!(libfile(&root, "absent.md"), None);
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn the_scrub_function_is_recognised_by_its_definition_line() {
        assert!(defines_scrub_function(b"maintainer_scrub_secret() {\n  :\n}\n"));
        assert!(defines_scrub_function(b"  maintainer_scrub_secret () {\n}\n"));
        assert!(defines_scrub_function(b"function maintainer_scrub_secret {\n}\n"));
        assert!(defines_scrub_function(b"function maintainer_scrub_secret() {\n}\n"));
        assert!(!defines_scrub_function(b"# maintainer_scrub_secret() is described here\n"));
        assert!(!defines_scrub_function(b"# a profile library with no scrub function\n"));
        assert!(!defines_scrub_function(b"maintainer_scrub_secret_other() { :; }\n"));
        assert!(!defines_scrub_function(b"echo maintainer_scrub_secret\n"));
    }
}
