//! The container a verify runs in, as data.
//!
//! Every control of the bash `run_in_container` is measured by
//! `tests/containment-probe.sh` with a real adversarial payload, not assumed from
//! the flag being present. This type carries the flag set so a test can pin it,
//! and so no caller can build a run line without it.

use crate::{ExtraArgs, Limits, SandboxError, Timeout};
use mx_core::NonEmpty;
use mx_suite::{ImageRef, Runtime};
use std::path::{Path, PathBuf};

/// Which of the two `podman run` lines this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// The run that executes the pull request's tests: no network, a CPU cap.
    Verify,
    /// The fetch that precedes it for a suite that asks (`suite_prefetch_command`):
    /// a network, no CPU cap, and a log that replaces the last one.
    Prefetch,
}

/// How the run's output reaches its log file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogMode {
    /// `>>"$log" 2>&1`: the clean run and the mutated run each append to their own log.
    Append,
    /// `>"$work/prefetch.log" 2>&1`.
    Truncate,
}

/// What a run needs that the flag set does not fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerInputs {
    pub runtime: Runtime,
    /// The extracted tree on the host, mounted at `/repo`. Absolute, no `:`.
    pub work: PathBuf,
    /// `suite_image`.
    pub image: ImageRef,
    /// `suite_command <filter>` (or `suite_prefetch_command`), the text `sh -c` runs.
    pub command: String,
    /// The words of `suite_podman_args`.
    pub extra_args: ExtraArgs,
    /// `VERIFY_TIMEOUT`, or `PREFETCH_TIMEOUT` for the fetch.
    pub timeout: Timeout,
    /// The resources. A prefetch ignores `cpus`.
    pub limits: Limits,
}

/// One container run: a purpose and its inputs, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSpec {
    purpose: Purpose,
    work: String,
    inputs: ContainerInputs,
}

fn check(inputs: &ContainerInputs) -> Result<String, SandboxError> {
    let work = inputs.work.to_str().ok_or(SandboxError::WorkNotText)?;
    if !work.starts_with('/') {
        return Err(SandboxError::WorkNotAbsolute { path: work.to_string() });
    }
    if work.contains(':') {
        return Err(SandboxError::WorkHasColon { path: work.to_string() });
    }
    if inputs.command.is_empty() {
        return Err(SandboxError::EmptyCommand);
    }
    Ok(work.to_string())
}

impl ContainerSpec {
    /// The run of the pull request's tests.
    pub fn verify(inputs: ContainerInputs) -> Result<Self, SandboxError> {
        let work = check(&inputs)?;
        Ok(Self { purpose: Purpose::Verify, work, inputs })
    }

    /// The networked fetch before it.
    pub fn prefetch(inputs: ContainerInputs) -> Result<Self, SandboxError> {
        let work = check(&inputs)?;
        Ok(Self { purpose: Purpose::Prefetch, work, inputs })
    }

    /// Which run this is.
    #[must_use]
    pub fn purpose(&self) -> Purpose {
        self.purpose
    }

    /// Whether the container gets a network: only the prefetch does.
    #[must_use]
    pub fn has_network(&self) -> bool {
        self.purpose == Purpose::Prefetch
    }

    /// How the output reaches the log.
    #[must_use]
    pub fn log_mode(&self) -> LogMode {
        match self.purpose {
            Purpose::Verify => LogMode::Append,
            Purpose::Prefetch => LogMode::Truncate,
        }
    }

    /// The runtime this run starts.
    #[must_use]
    pub fn runtime(&self) -> Runtime {
        self.inputs.runtime
    }

    /// The tree mounted at `/repo`.
    #[must_use]
    pub fn work(&self) -> &Path {
        &self.inputs.work
    }

    /// The whole command line, the runtime first, flag for flag as the bash
    /// writes it, in its order.
    ///
    /// ```text
    /// <rt> --log-level=error run --rm --timeout <T> [--network=none]
    ///   --cap-drop=ALL --security-opt=no-new-privileges --read-only
    ///   --tmpfs /tmp:rw,nosuid,size=<tmpfs> --pids-limit <pids> --memory <mem> [--cpus <cpus>]
    ///   -v <work>:/repo:z <extra args...> -w /repo <image> sh -c <command>
    /// ```
    ///
    /// The bracketed flags are the verify run's; the prefetch has neither.
    /// `--log-level=error` comes before the subcommand because the runtime writes
    /// its own warnings to the stream the container uses, and the log is read as
    /// evidence. `/tmp` is `nosuid` and not `noexec` on purpose: a test that
    /// writes and runs a stub there failed under `noexec` and the gate blamed the
    /// pull request.
    #[must_use]
    pub fn argv(&self) -> NonEmpty<String> {
        let inputs = &self.inputs;
        let mut words = vec![
            "--log-level=error".to_string(),
            "run".to_string(),
            "--rm".to_string(),
            "--timeout".to_string(),
            inputs.timeout.secs().to_string(),
        ];
        if self.purpose == Purpose::Verify {
            words.push("--network=none".to_string());
        }
        words.extend(
            ["--cap-drop=ALL", "--security-opt=no-new-privileges", "--read-only", "--tmpfs"]
                .map(str::to_string),
        );
        words.push(format!("/tmp:rw,nosuid,size={}", inputs.limits.tmpfs.as_str()));
        words.extend(["--pids-limit".to_string(), inputs.limits.pids.as_str().to_string()]);
        words.extend(["--memory".to_string(), inputs.limits.memory.as_str().to_string()]);
        if self.purpose == Purpose::Verify {
            words.extend(["--cpus".to_string(), inputs.limits.cpus.as_str().to_string()]);
        }
        words.extend(["-v".to_string(), format!("{}:/repo:z", self.work)]);
        words.extend(inputs.extra_args.as_slice().iter().cloned());
        words.extend(["-w".to_string(), "/repo".to_string(), inputs.image.as_str().to_string()]);
        words.extend(["sh".to_string(), "-c".to_string(), inputs.command.clone()]);
        NonEmpty::new(inputs.runtime.as_str().to_string(), words)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> ContainerInputs {
        ContainerInputs {
            runtime: Runtime::Podman,
            work: PathBuf::from("/state/verify-work/ab12cd34"),
            image: ImageRef::parse("docker.io/library/rust:1-slim").unwrap(),
            command: "cargo test --offline --workspace guard".to_string(),
            extra_args: ExtraArgs::split("-v\n/home/u/.cargo:/cargo:O\n-e CARGO_HOME=/cargo"),
            timeout: Timeout::from_secs(1800).unwrap(),
            limits: Limits {
                tmpfs: crate::Limit::parse(mx_suite::LimitKey::Tmpfs, "256m").unwrap(),
                pids: crate::Limit::parse(mx_suite::LimitKey::Pids, "512").unwrap(),
                memory: crate::Limit::parse(mx_suite::LimitKey::Memory, "6g").unwrap(),
                cpus: crate::Limit::parse(mx_suite::LimitKey::Cpus, "2").unwrap(),
            },
        }
    }

    fn words(spec: &ContainerSpec) -> Vec<String> {
        spec.argv().into_vec()
    }

    #[test]
    fn the_verify_run_has_exactly_the_flags_of_run_in_container_in_their_order() {
        // bin/maintainer-merge, run_in_container:
        //   "$rt" --log-level=error run --rm --timeout "$VERIFY_TIMEOUT" --network=none
        //   --cap-drop=ALL --security-opt=no-new-privileges --read-only
        //   --tmpfs "/tmp:rw,nosuid,size=..." --pids-limit .. --memory .. --cpus ..
        //   -v "$work:/repo:z" $(suite_extra_args) -w /repo "$(suite_image)" sh -c "$(suite_command ..)"
        let spec = ContainerSpec::verify(inputs()).unwrap();
        assert_eq!(
            words(&spec),
            [
                "podman",
                "--log-level=error",
                "run",
                "--rm",
                "--timeout",
                "1800",
                "--network=none",
                "--cap-drop=ALL",
                "--security-opt=no-new-privileges",
                "--read-only",
                "--tmpfs",
                "/tmp:rw,nosuid,size=256m",
                "--pids-limit",
                "512",
                "--memory",
                "6g",
                "--cpus",
                "2",
                "-v",
                "/state/verify-work/ab12cd34:/repo:z",
                "-v",
                "/home/u/.cargo:/cargo:O",
                "-e",
                "CARGO_HOME=/cargo",
                "-w",
                "/repo",
                "docker.io/library/rust:1-slim",
                "sh",
                "-c",
                "cargo test --offline --workspace guard",
            ]
        );
    }

    #[test]
    fn the_prefetch_run_has_a_network_and_no_cpu_cap() {
        // bin/maintainer-merge, the suite_prefetch_command block: the same line
        // with --timeout "$PREFETCH_TIMEOUT", no --network=none and no --cpus.
        let mut fetch = inputs();
        fetch.timeout = Timeout::from_secs(900).unwrap();
        fetch.command = "CARGO_NET_OFFLINE=false cargo fetch --locked".to_string();
        let spec = ContainerSpec::prefetch(fetch).unwrap();
        assert_eq!(
            words(&spec),
            [
                "podman",
                "--log-level=error",
                "run",
                "--rm",
                "--timeout",
                "900",
                "--cap-drop=ALL",
                "--security-opt=no-new-privileges",
                "--read-only",
                "--tmpfs",
                "/tmp:rw,nosuid,size=256m",
                "--pids-limit",
                "512",
                "--memory",
                "6g",
                "-v",
                "/state/verify-work/ab12cd34:/repo:z",
                "-v",
                "/home/u/.cargo:/cargo:O",
                "-e",
                "CARGO_HOME=/cargo",
                "-w",
                "/repo",
                "docker.io/library/rust:1-slim",
                "sh",
                "-c",
                "CARGO_NET_OFFLINE=false cargo fetch --locked",
            ]
        );
        assert!(spec.has_network());
        assert!(!words(&spec).iter().any(|w| w == "--network=none" || w == "--cpus"));
    }

    #[test]
    fn only_the_verify_run_is_cut_off_from_the_network() {
        let verify = ContainerSpec::verify(inputs()).unwrap();
        assert!(!verify.has_network());
        assert!(words(&verify).contains(&"--network=none".to_string()));
    }

    #[test]
    fn the_containment_flags_are_on_both_runs() {
        for spec in
            [ContainerSpec::verify(inputs()).unwrap(), ContainerSpec::prefetch(inputs()).unwrap()]
        {
            let flags = words(&spec);
            for must in
                ["--rm", "--cap-drop=ALL", "--security-opt=no-new-privileges", "--read-only"]
            {
                assert!(flags.contains(&must.to_string()), "{:?} lacks {must}", spec.purpose());
            }
            assert!(!flags.contains(&"--privileged".to_string()));
        }
    }

    #[test]
    fn the_tmpfs_is_nosuid_and_exec_capable() {
        // The comment above run_in_container: noexec failed a test that runs a stub from /tmp.
        let flags = words(&ContainerSpec::verify(inputs()).unwrap());
        let at = flags.iter().position(|w| w == "--tmpfs").unwrap();
        assert_eq!(flags[at + 1], "/tmp:rw,nosuid,size=256m");
        assert!(!flags[at + 1].contains("noexec"));
    }

    #[test]
    fn the_log_level_sits_before_the_subcommand() {
        let flags = words(&ContainerSpec::verify(inputs()).unwrap());
        let level = flags.iter().position(|w| w == "--log-level=error").unwrap();
        let run = flags.iter().position(|w| w == "run").unwrap();
        assert_eq!((level, run), (1, 2));
    }

    #[test]
    fn docker_replaces_the_program_and_nothing_else() {
        let mut docker = inputs();
        docker.runtime = Runtime::Docker;
        let spec = ContainerSpec::verify(docker).unwrap();
        let mut want = words(&ContainerSpec::verify(inputs()).unwrap());
        want[0] = "docker".to_string();
        assert_eq!(words(&spec), want);
        assert_eq!(spec.runtime(), Runtime::Docker);
    }

    #[test]
    fn the_command_stays_one_argument_whatever_it_holds() {
        let mut odd = inputs();
        odd.command = "cargo test x; echo 'a b' && true".to_string();
        let flags = words(&ContainerSpec::verify(odd).unwrap());
        assert_eq!(flags.last().unwrap(), "cargo test x; echo 'a b' && true");
        assert_eq!(flags[flags.len() - 3], "sh");
    }

    #[test]
    fn a_run_without_extra_args_has_none_between_the_mount_and_the_workdir() {
        let mut bare = inputs();
        bare.extra_args = ExtraArgs::default();
        let flags = words(&ContainerSpec::verify(bare).unwrap());
        let mount = flags.iter().position(|w| w.ends_with(":/repo:z")).unwrap();
        assert_eq!(flags[mount + 1], "-w");
    }

    #[test]
    fn the_prefetch_truncates_its_log_and_the_verify_appends() {
        assert_eq!(ContainerSpec::verify(inputs()).unwrap().log_mode(), LogMode::Append);
        assert_eq!(ContainerSpec::prefetch(inputs()).unwrap().log_mode(), LogMode::Truncate);
    }

    #[test]
    fn a_relative_work_path_is_refused() {
        // P30-5: `-v name:/repo` mounts a named volume, not the tree.
        for work in ["rel/dir", "work", "./work", ""] {
            let mut bad = inputs();
            bad.work = PathBuf::from(work);
            assert_eq!(
                ContainerSpec::verify(bad.clone()),
                Err(SandboxError::WorkNotAbsolute { path: work.to_string() }),
                "{work:?}"
            );
            assert!(ContainerSpec::prefetch(bad).is_err());
        }
    }

    #[test]
    fn a_work_path_with_a_colon_is_refused() {
        let mut bad = inputs();
        bad.work = PathBuf::from("/state/a:b");
        assert_eq!(
            ContainerSpec::verify(bad.clone()),
            Err(SandboxError::WorkHasColon { path: "/state/a:b".into() })
        );
        assert!(ContainerSpec::prefetch(bad).is_err());
    }

    #[test]
    fn a_work_path_that_is_not_utf8_is_refused() {
        use std::os::unix::ffi::OsStrExt;
        let mut bad = inputs();
        bad.work = PathBuf::from(std::ffi::OsStr::from_bytes(b"/state/\xff"));
        assert_eq!(ContainerSpec::verify(bad), Err(SandboxError::WorkNotText));
    }

    #[test]
    fn an_empty_command_is_refused() {
        let mut bad = inputs();
        bad.command = String::new();
        assert_eq!(ContainerSpec::verify(bad.clone()), Err(SandboxError::EmptyCommand));
        assert_eq!(ContainerSpec::prefetch(bad), Err(SandboxError::EmptyCommand));
    }

    #[test]
    fn the_refusal_text_names_the_path() {
        assert_eq!(
            SandboxError::WorkNotAbsolute { path: "rel".into() }.to_string(),
            "the container's work directory 'rel' is not an absolute path"
        );
        assert_eq!(
            SandboxError::WorkHasColon { path: "/a:b".into() }.to_string(),
            "the container's work directory '/a:b' has a ':', which the runtime reads as the end of the host path"
        );
    }

    #[test]
    fn the_work_directory_is_the_tree_that_was_given() {
        assert_eq!(
            ContainerSpec::verify(inputs()).unwrap().work(),
            Path::new("/state/verify-work/ab12cd34")
        );
    }
}
