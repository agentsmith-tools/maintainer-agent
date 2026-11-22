//! The modes the file commands give depend on the caller's umask. This file holds ONE test
//! because it changes the umask of the process, which every other test would see.

use mx_install::coreutils::{chmod_x, cp, mkdir_p, touch};
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn under_a_umask_of_077_cp_chmod_x_touch_and_mkdir_leave_no_bit_for_group_or_others() {
    let dir = std::env::temp_dir().join(format!("mx-install-umask-{}", std::process::id()));
    drop(fs::remove_dir_all(&dir));
    fs::create_dir_all(&dir).unwrap();
    let old = rustix::process::umask(rustix::fs::Mode::from_bits_truncate(0o077));
    let mode = |name: &str| fs::metadata(dir.join(name)).unwrap().permissions().mode() & 0o7777;

    fs::write(dir.join("src"), "x").unwrap();
    fs::set_permissions(dir.join("src"), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(cp(&dir.join("src"), &dir.join("copy")).status, 0);
    assert_eq!(mode("copy"), 0o700, "a new file takes its source's mode under the umask");

    fs::write(dir.join("plain"), "x").unwrap();
    fs::set_permissions(dir.join("plain"), fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(chmod_x(&[dir.join("plain")]).status, 0);
    assert_eq!(mode("plain"), 0o700, "chmod +x adds only the execute bits the umask allows");

    assert_eq!(touch(&dir.join("stamp")).status, 0);
    assert_eq!(mode("stamp"), 0o600);
    assert_eq!(mkdir_p(&[dir.join("made")]).status, 0);
    assert_eq!(mode("made"), 0o700);
    assert_eq!(mx_install::coreutils::umask(), 0o077);

    rustix::process::umask(old);
    drop(fs::remove_dir_all(&dir));
}
