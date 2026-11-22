//! Performing a plan, through a sink that either does each step or says what it would do.
//!
//! [`apply`] is one loop for a real run and a dry one. The steps that only talk (`Say`,
//! the list of failed steps, the lines of the kept trail) are the same in both; each step
//! that touches the machine goes to the [`Sink`], and the [`DrySink`] answers every one of
//! them with `would: ...` and changes nothing. A test gives the dry sink a machine that
//! panics on any change to prove it.

use crate::console::{Console, Exit, Flow};
use crate::env::Env;
use crate::machine::{Machine, glob_children};
use crate::plan::{Cmd, Layout, OnFail, Plan, RustProgram, Step};
use crate::tools::{self, COMMAND, QUICK};
use crate::util::{basename, cat, parent_name, raw};
use mx_ports::Proc;
use std::path::PathBuf;

/// What touches the machine.
pub trait Sink {
    /// A command of the bash, through `run`. Returns the status it ended with.
    fn command(&mut self, cmd: &Cmd, con: &mut Console) -> Flow<i32>;
    /// The VERSION stamp.
    fn stamp(&mut self, con: &mut Console) -> Flow;
    /// The receipt-signing key, when it is absent.
    fn mint_key(&mut self, con: &mut Console) -> Flow;
    /// The deny walls of the deployed profiles.
    fn render_walls(&mut self, con: &mut Console) -> Flow;
    /// A real run needs a profile where `run.sh` looks for one.
    fn landing_check(&mut self, con: &mut Console) -> Flow;
    /// One program replaced by its Rust build.
    fn swap(&mut self, program: &RustProgram, con: &mut Console) -> Flow;
    /// The stamp of a Rust install.
    fn write_rust_stamp(&mut self, programs: &[RustProgram], con: &mut Console) -> Flow;
}

/// Performs `plan`. Returns the status the program ends with.
pub fn apply(plan: &Plan, sink: &mut dyn Sink, layout: &Layout, con: &mut Console) -> Flow<i32> {
    let mut failed: Vec<Vec<u8>> = Vec::new();
    for step in &plan.steps {
        match step {
            Step::Say(text) => con.say(text)?,
            Step::Run(cmd, on_fail) => {
                let status = sink.command(cmd, con)?;
                if status != 0 {
                    match on_fail {
                        // `set -e`: the run ends with the status of the command.
                        OnFail::Abort => return Err(Exit(status)),
                        OnFail::Record => failed.push(cat(&[b"  run ", &cmd.text()])),
                    }
                }
            }
            Step::Stamp => sink.stamp(con)?,
            Step::MintKey => sink.mint_key(con)?,
            Step::RenderWalls => sink.render_walls(con)?,
            Step::LandingCheck => sink.landing_check(con)?,
            Step::Swap(program) => sink.swap(program, con)?,
            Step::WriteRustStamp(programs) => sink.write_rust_stamp(programs, con)?,
            Step::NoTimersEnabled => {
                let line = cat(&[
                    b"install.sh: --timers matched no unit files under ",
                    raw(layout.root.as_os_str()),
                    b"/platform/linux",
                ]);
                con.echo_err(&line)?;
                return Err(Exit(1));
            }
            Step::FailureReport => {
                if !failed.is_empty() {
                    con.say(
                        b"SOME STEPS FAILED. This deployment is part-removed, and that is the",
                    )?;
                    con.say(b"state to fix rather than to leave:")?;
                    // `_failed` starts with a newline and every entry follows its own.
                    let mut text = vec![b'\n'];
                    for entry in &failed {
                        text.extend_from_slice(entry);
                        text.push(b'\n');
                    }
                    con.err(&text)?;
                    con.say(b"")?;
                }
            }
            Step::KeptLine(line) => match line {
                Ok(text) => con.say(&cat(&[b"         ", text]))?,
                Err(failure) => {
                    con.echo_err(failure.message().as_bytes())?;
                    con.say(b"         ")?;
                }
            },
        }
    }
    Ok(plan.exit)
}

/// The sink of a dry run: it reads the machine and changes nothing.
pub struct DrySink<'a> {
    pub machine: &'a dyn Machine,
    pub layout: &'a Layout,
}

impl Sink for DrySink<'_> {
    fn command(&mut self, cmd: &Cmd, con: &mut Console) -> Flow<i32> {
        con.say(&cat(&[b"would: ", &cmd.text()]))?;
        Ok(0)
    }

    fn stamp(&mut self, con: &mut Console) -> Flow {
        con.say(&cat(&[b"would stamp ", raw(self.layout.share.as_os_str()), b"/VERSION"]))
    }

    fn mint_key(&mut self, con: &mut Console) -> Flow {
        con.say(&cat(&[
            b"would mint ",
            raw(self.layout.share.as_os_str()),
            b"/receipt.key if absent",
        ]))
    }

    fn render_walls(&mut self, con: &mut Console) -> Flow {
        // The walls come from the DEPLOYED profiles. A dry run has deployed nothing, so on a
        // clean machine this prints nothing; on a machine with a deployment it lists them.
        for spec in glob_children(self.machine, &self.layout.share.join("profiles"), "deny.json") {
            if !self.machine.exists(&spec) {
                continue;
            }
            let name = parent_name(&spec);
            con.say(&cat(&[
                b"would generate ",
                raw(&name),
                b"/settings.json and settings-rehearsal.json",
            ]))?;
        }
        Ok(())
    }

    fn landing_check(&mut self, _con: &mut Console) -> Flow {
        Ok(())
    }

    fn swap(&mut self, program: &RustProgram, con: &mut Console) -> Flow {
        for cmd in swap_commands(program) {
            con.say(&cat(&[b"would: ", &cmd.text()]))?;
        }
        Ok(())
    }

    fn write_rust_stamp(&mut self, _programs: &[RustProgram], con: &mut Console) -> Flow {
        con.say(&cat(&[b"would write ", raw(self.layout.share.as_os_str()), b"/RUST_STAMP"]))
    }
}

/// The three commands that put a Rust build in place: save the bash original, make the
/// copy executable, and replace the installed file.
fn swap_commands(program: &RustProgram) -> [Cmd; 3] {
    [
        Cmd::Cp { src: program.installed.clone(), dst: program.backup.clone() },
        Cmd::ChmodX(vec![program.backup.clone()]),
        Cmd::InstallBinary { src: program.source.clone(), dst: program.installed.clone() },
    ]
}

/// The sink of a real run.
pub struct LiveSink<'a> {
    pub machine: &'a dyn Machine,
    pub proc: &'a dyn Proc,
    pub env: &'a Env,
    pub layout: &'a Layout,
}

impl LiveSink<'_> {
    /// A redirection that failed: `install.sh: line N: PATH: reason`, and the run ends
    /// with status 1 (`set -e`).
    fn redirect_failed(
        &self,
        con: &mut Console,
        line: u32,
        path: &std::path::Path,
        error: &std::io::Error,
    ) -> Flow {
        let text = format!(
            "{}: line {line}: {}: {}",
            self.script(),
            path.display(),
            crate::coreutils::strerror(error)
        );
        con.echo_err(text.as_bytes())?;
        Err(Exit(1))
    }

    /// `$0`: the name bash puts in front of its own error messages.
    fn script(&self) -> String {
        self.env.argv0.to_string_lossy().into_owned()
    }

    fn captured(&self, program: &str, args: &[&str]) -> Option<Vec<u8>> {
        let ran = tools::call(self.proc, &tools::spec(self.env, program, args, QUICK));
        ran.succeeded().then(|| crate::util::strip_trailing_newlines(ran.stdout().to_vec()))
    }
}

impl Sink for LiveSink<'_> {
    fn command(&mut self, cmd: &Cmd, con: &mut Console) -> Flow<i32> {
        match cmd {
            Cmd::Systemctl(args) | Cmd::Launchctl(args) => {
                let program =
                    if matches!(cmd, Cmd::Systemctl(_)) { "systemctl" } else { "launchctl" };
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                tools::run_visible(
                    self.proc,
                    con,
                    &tools::spec(self.env, program, &args, COMMAND),
                    &self.script(),
                    28,
                )
            }
            Cmd::CronRemove { script, profile } => {
                let mut spec =
                    tools::spec(self.env, &script.to_string_lossy(), &["--remove"], COMMAND);
                spec = spec.env("MAINTAINER_PROFILE", profile.to_string_lossy().into_owned());
                tools::run_visible(self.proc, con, &spec, &self.script(), 28)
            }
            file_command => {
                let outcome = self.machine.exec(file_command);
                con.err(&outcome.diag)?;
                Ok(outcome.status)
            }
        }
    }

    fn stamp(&mut self, con: &mut Console) -> Flow {
        // `stamp_version > "$share/VERSION"` opens the file before the function runs, so a
        // file that cannot be opened ends the run before git or date is asked.
        let path = self.layout.share.join("VERSION");
        if let Err(error) = self.machine.write(&path, b"") {
            return self.redirect_failed(con, 190, &path, &error);
        }
        let root = self.layout.root.to_string_lossy().into_owned();
        let version = self
            .captured("git", &["-C", &root, "describe", "--tags", "--always", "--dirty"])
            .unwrap_or_else(|| b"unknown".to_vec());
        let commit = self
            .captured("git", &["-C", &root, "rev-parse", "--short", "HEAD"])
            .unwrap_or_else(|| b"unknown".to_vec());
        let date_spec = tools::spec(self.env, "date", &["-Is"], QUICK);
        let date = match tools::call(self.proc, &date_spec) {
            tools::Ran::Done(output) => {
                con.err(&output.stderr)?;
                crate::util::strip_trailing_newlines(output.stdout)
            }
            tools::Ran::Missing => {
                con.echo_err(
                    format!("{}: line 143: date: command not found", self.script()).as_bytes(),
                )?;
                Vec::new()
            }
            _ => Vec::new(),
        };
        let text = cat(&[
            b"version=",
            &version,
            b"\ncommit=",
            &commit,
            b"\ninstalled_at=",
            &date,
            b"\ninstalled_from=",
            raw(self.layout.root.as_os_str()),
            b"\n",
        ]);
        match self.machine.write(&path, &text) {
            Ok(()) => Ok(()),
            Err(error) => self.redirect_failed(con, 190, &path, &error),
        }
    }

    fn mint_key(&mut self, con: &mut Console) -> Flow {
        let path = self.layout.share.join("receipt.key");
        if self.machine.is_file(&path) {
            return Ok(());
        }
        let key = match self.machine.random_hex() {
            Ok(key) => key,
            Err(error) => return self.redirect_failed(con, 200, &path, &error),
        };
        // The file is mode 600 from the moment it exists; the key is never printed.
        if let Err(error) = self.machine.write_private(&path, key.as_bytes()) {
            return self.redirect_failed(con, 200, &path, &error);
        }
        con.say(b"minted a receipt-signing key")
    }

    fn render_walls(&mut self, con: &mut Console) -> Flow {
        let script = self.layout.root.join("scripts").join("render-settings.py");
        for spec in glob_children(self.machine, &self.layout.share.join("profiles"), "deny.json") {
            if !self.machine.exists(&spec) {
                continue;
            }
            let dir = spec.parent().map(PathBuf::from).unwrap_or_default();
            let home = self.layout.home.to_string_lossy().into_owned();
            let call = tools::spec(
                self.env,
                "python3",
                &[&script.to_string_lossy(), &dir.to_string_lossy(), &home],
                COMMAND,
            );
            let status = match tools::call(self.proc, &call) {
                tools::Ran::Done(output) => {
                    con.out(&output.stdout)?;
                    con.err(&output.stderr)?;
                    tools::Ran::Done(output).status()
                }
                tools::Ran::Missing => {
                    con.echo_err(
                        format!("{}: line 233: python3: command not found", self.script())
                            .as_bytes(),
                    )?;
                    127
                }
                other => other.status(),
            };
            if status != 0 {
                con.echo_err(b"install.sh: generating the deny wall failed")?;
                return Err(Exit(1));
            }
        }
        Ok(())
    }

    fn landing_check(&mut self, con: &mut Console) -> Flow {
        let landed =
            !glob_children(self.machine, &self.layout.share.join("profiles"), "profile.env")
                .is_empty();
        if landed {
            return Ok(());
        }
        con.echo_err(b"install.sh: profiles did not land where run.sh expects")?;
        Err(Exit(1))
    }

    fn swap(&mut self, program: &RustProgram, con: &mut Console) -> Flow {
        for cmd in swap_commands(program) {
            let outcome = self.machine.exec(&cmd);
            con.err(&outcome.diag)?;
            if outcome.status != 0 {
                return Err(Exit(outcome.status));
            }
        }
        let hash = match self.machine.sha256(&program.installed) {
            Ok(hash) => hash,
            Err(error) => return self.redirect_failed(con, 0, &program.installed, &error),
        };
        con.say(&cat(&[
            b"rust: ",
            raw(&basename(&program.installed)),
            b" is the Rust build (sha256 ",
            &hash.as_bytes()[..12],
            b")",
        ]))
    }

    fn write_rust_stamp(&mut self, programs: &[RustProgram], con: &mut Console) -> Flow {
        // One line per program, in the format `sha256sum -c` reads. Each hash is read from
        // the installed file, so the stamp records what is there and not what was meant.
        let mut text = Vec::new();
        for program in programs {
            let hash = match self.machine.sha256(&program.installed) {
                Ok(hash) => hash,
                Err(error) => return self.redirect_failed(con, 0, &program.installed, &error),
            };
            text.extend(cat(&[hash.as_bytes(), b"  ", raw(program.installed.as_os_str()), b"\n"]));
        }
        let path = self.layout.share.join("RUST_STAMP");
        match self.machine.write(&path, &text) {
            Ok(()) => con.say(&cat(&[b"rust: stamp written to ", raw(path.as_os_str())])),
            Err(error) => self.redirect_failed(con, 0, &path, &error),
        }
    }
}
