use crate::real_proc::new_command;
use std::ffi::{OsStr, OsString};
use std::os::unix::process::CommandExt;
use std::path::Path;

/// Where the shell lives. An absolute path, so the search path cannot choose it.
const SH: &str = "/bin/sh";

/// Replaces this process with `program args...`, started by a shell that has
/// closed descriptor `fd` first, and returns only when the replacement failed.
///
/// This is how a program takes a descriptor it did not open: the standard library
/// has no safe call that closes a descriptor by number, so the program starts
/// again through `sh -c 'exec FD>&-; exec "$0" "$@"'`, and the new image has it
/// free. The environment is inherited, plus `extra_env`. `program` is run with
/// `program` as its own `argv[0]`; the caller passes the name it wants the new
/// image to use in `extra_env`.
#[must_use]
pub fn exec_with_descriptor_closed(
    fd: i32,
    program: &Path,
    args: &[OsString],
    extra_env: &[(&str, &OsStr)],
) -> std::io::Error {
    let mut command = new_command(Path::new(SH));
    command.arg("-c").arg(format!("exec {fd}>&-; exec \"$0\" \"$@\"")).arg(program).args(args);
    for (name, value) in extra_env {
        command.env(name, value);
    }
    command.exec()
}
