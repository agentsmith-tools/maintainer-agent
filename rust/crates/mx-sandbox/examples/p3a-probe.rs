//! The probe behind `tests/differential.sh`: one operation of the real suite host, selector
//! and sandbox per run, printed in a form the harness can compare with the bash function.
//!
//! This is test tooling and not part of the gate. It reads the process environment on
//! purpose (the harness sets the variables the bash functions read), and it never starts a
//! container.
//!
//! ```text
//! p3a-probe glob                              stdin: PATTERN TAB PATH per line -> 0 or 1 per line
//! p3a-probe realpath PATH...                  one canonical path per line
//! p3a-probe escapes ROOT                      the lines of `tree_escapes`
//! p3a-probe pick DIR GLOBS NAMED|- PATH...    suite name on stdout, notes on stderr
//! p3a-probe cover DIR GLOBS NAMES PATH...     status, reasons on stderr
//! p3a-probe map DIR PATH...                   the lines of `coverage_map`
//! p3a-probe ran DIR SUITE LOG                 the count `suite_ran` gives
//! p3a-probe extract REPO REV DEST             status, message on stderr, files counted on stdout
//! p3a-probe mutate ROOT EXPR GLOB...          the changed paths, one per line
//! p3a-probe flags DIR SUITE RUNTIME WORK SCRATCH FILTER verify|prefetch   the argv, NUL-ended
//! ```

use mx_core::NonEmpty;
use mx_ports::{RealPipe, RealProc};
use mx_sandbox::{
    ContainedTree, ContainerInputs, ContainerSpec, ExtraArgs, LimitSource, Limits, RealSandbox,
    Sandbox, SedExpression, Timeout, realpath,
};
use mx_suite::{
    HostSelector, LimitKey, MutateGlob, ProdGlobs, RealSuiteHost, Runtime, SUITE_ENV_NAMES,
    SuiteDirs, SuiteHost, SuiteName, SuiteSelector, TestFilter,
};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

const SEARCH: &str = "/usr/bin:/bin";

fn proc() -> RealProc {
    let mut names: Vec<&str> = SUITE_ENV_NAMES.to_vec();
    names.extend(["GIT_CONFIG_GLOBAL", "GIT_CONFIG_NOSYSTEM", "LANG", "LC_CTYPE"]);
    RealProc::from_process_env(SEARCH, &names)
}

fn host(dir: &str) -> RealSuiteHost {
    // `dir` is a verify.d; its parent is the PROFILE_DIR the bash gate reads.
    let parent = Path::new(dir).parent().map_or(String::new(), |p| p.display().to_string());
    RealSuiteHost::new(
        Arc::new(proc()),
        SuiteDirs {
            profile_dir: parent,
            home: "/nonexistent".into(),
            profile: String::new(),
            here: PathBuf::from("/nonexistent"),
        },
    )
}

fn sandbox() -> RealSandbox {
    RealSandbox::new(
        Arc::new(proc()),
        Arc::new(RealPipe::new(SEARCH, None)),
        Runtime::Podman,
        Vec::new(),
    )
    .expect("podman sandbox")
}

fn die(message: &str) -> ExitCode {
    eprintln!("maintainer-merge: {message}");
    ExitCode::from(1)
}

fn names(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

fn paths(args: &[String]) -> Result<NonEmpty<String>, String> {
    NonEmpty::from_vec(args.to_vec()).map_err(|_| "no paths".to_string())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(op) = args.first().map(String::as_str) else {
        return die("no operation");
    };
    let rest = &args[1..];
    if let Some(code) = path_ops(op, rest) {
        return code;
    }
    if let Some(code) = selection_ops(op, rest) {
        return code;
    }
    sandbox_ops(op, rest)
        .unwrap_or_else(|| die(&format!("unknown operation or arguments: {args:?}")))
}

fn path_ops(op: &str, rest: &[String]) -> Option<ExitCode> {
    if !["glob", "realpath", "escapes"].contains(&op) {
        return None;
    }
    let run = || match (op, rest) {
        ("glob", []) => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input).expect("stdin");
            let mut out = String::new();
            for line in input.lines() {
                let (pattern, path) = line.split_once('\t').expect("PATTERN TAB PATH");
                let globs = ProdGlobs::from_profile_value(pattern);
                // A pattern with a blank is several globs; one per line is what the harness sends.
                let hit = globs.is_ok_and(|g| g.matches(path));
                out.push_str(if hit { "1\n" } else { "0\n" });
            }
            print!("{out}");
            ExitCode::SUCCESS
        }
        ("realpath", list) => {
            for path in list {
                println!("{}", realpath(Path::new(path)).display());
            }
            ExitCode::SUCCESS
        }
        ("escapes", [root]) => {
            let tree = mx_sandbox::ExtractedTree::counted(PathBuf::from(root), "r", 1, 1)
                .expect("a counted tree");
            match sandbox().tree_escapes(&tree) {
                Ok(found) => {
                    let lines: Vec<String> = found.iter().map(ToString::to_string).collect();
                    println!("{}", lines.join("\n"));
                    ExitCode::SUCCESS
                }
                Err(e) => die(&e.to_string()),
            }
        }
        _ => die(&format!("bad arguments for {op}: {rest:?}")),
    };
    Some(run())
}

fn selection_ops(op: &str, rest: &[String]) -> Option<ExitCode> {
    if !["pick", "cover", "map", "ran"].contains(&op) {
        return None;
    }
    let run = || match (op, rest) {
        ("pick", [dir, globs, named, list @ ..]) => {
            let host = host(dir);
            let selector = HostSelector::new(&host);
            let named = if named == "-" { None } else { SuiteName::parse(named).ok() };
            let Ok(changed) = paths(list) else { return die("no paths") };
            let picked = match ProdGlobs::from_profile_value(globs) {
                Ok(globs) => selector.pick_suite(&globs, named.as_ref(), &changed),
                Err(_) => selector.pick_suite_without_prod_globs(named.as_ref(), &changed),
            };
            match picked {
                Ok(picked) => {
                    eprint!("{}", picked.notes);
                    print!("{}", picked.suite);
                    ExitCode::SUCCESS
                }
                Err(refusal) => die(&refusal.to_string()),
            }
        }
        ("cover", [dir, globs, suites, list @ ..]) => {
            let Ok(globs) = ProdGlobs::from_profile_value(globs) else {
                eprint!("{}", mx_suite::CoverageFailure::NoProdGlobs.stderr_lines());
                return ExitCode::from(1);
            };
            let host = host(dir);
            let selector = HostSelector::new(&host);
            let Ok(changed) = paths(list) else { return die("no paths") };
            match selector.suites_cover_production(&globs, &names(suites), &changed) {
                Ok(()) => ExitCode::SUCCESS,
                Err(failure) => {
                    eprint!("{}", failure.stderr_lines());
                    ExitCode::from(1)
                }
            }
        }
        ("map", [dir, list @ ..]) => {
            let host = host(dir);
            match HostSelector::new(&host).coverage_map(list) {
                Ok(rows) => {
                    for row in rows {
                        println!("{}", row.line());
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => die(&e.to_string()),
            }
        }
        ("ran", [dir, suite, log]) => {
            let host = host(dir);
            let Ok(name) = SuiteName::parse(suite) else { return die("bad name") };
            match host.load(&name).and_then(|s| s.ran(Path::new(log))) {
                Ok(n) => {
                    print!("{n}");
                    ExitCode::SUCCESS
                }
                Err(e) => die(&e.to_string()),
            }
        }
        _ => die(&format!("bad arguments for {op}: {rest:?}")),
    };
    Some(run())
}

fn sandbox_ops(op: &str, rest: &[String]) -> Option<ExitCode> {
    if !["extract", "mutate", "flags"].contains(&op) {
        return None;
    }
    let run = || match (op, rest) {
        ("extract", [repo, rev, dest]) => {
            match sandbox().extract_tree(Path::new(repo), rev, Path::new(dest)) {
                Ok(tree) => {
                    println!("{}", tree.root().display());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::from(1)
                }
            }
        }
        ("mutate", [root, expr, globs @ ..]) => {
            let tree = mx_sandbox::ExtractedTree::counted(PathBuf::from(root), "r", 1, 1)
                .expect("a counted tree");
            let tree = ContainedTree::check(tree, Vec::new()).expect("contained");
            let Ok(expr) = SedExpression::parse(expr) else { return die("bad expression") };
            let parsed: Vec<MutateGlob> =
                globs.iter().filter_map(|g| MutateGlob::parse(g).ok()).collect();
            let Ok(globs) = NonEmpty::from_vec(parsed) else { return die("no globs") };
            match sandbox().apply_mutation(&tree, &globs, &expr) {
                Ok(changed) => {
                    let mut out = std::io::stdout();
                    for path in changed.paths() {
                        writeln!(out, "{path}").expect("stdout");
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => die(&e.to_string()),
            }
        }
        ("flags", [dir, suite, runtime, work, scratch, filter, kind]) => {
            flags(dir, suite, runtime, work, scratch, filter, kind)
        }
        _ => die(&format!("bad arguments for {op}: {rest:?}")),
    };
    Some(run())
}

fn flags(
    dir: &str,
    suite: &str,
    runtime: &str,
    work: &str,
    scratch: &str,
    filter: &str,
    kind: &str,
) -> ExitCode {
    let runtime = if runtime == "podman" { Runtime::Podman } else { Runtime::Docker };
    let host = host(dir);
    let (Ok(name), Ok(filter)) = (SuiteName::parse(suite), TestFilter::parse(filter)) else {
        return die("bad suite or filter");
    };
    let loaded = match host.load(&name) {
        Ok(s) => s,
        Err(e) => return die(&e.to_string()),
    };
    let env = |name: &str| std::env::var(name).ok();
    let limits = Limits::resolve(|key: LimitKey| LimitSource {
        env: env(key.env_name()),
        suite: loaded.limit(key).ok().flatten(),
        profile: env(key.defaults_key()),
    });
    let limits = match limits {
        Ok(limits) => limits,
        Err(e) => return die(&e.to_string()),
    };
    let prefetch = kind == "prefetch";
    let (command, timeout_name) = if prefetch {
        let Some(p) = loaded.spec().prefetch.clone() else { return die("no prefetch") };
        (p.command, "PREFETCH_TIMEOUT")
    } else {
        match loaded.command(&filter) {
            Ok(c) => (c, "VERIFY_TIMEOUT"),
            Err(e) => return die(&e.to_string()),
        }
    };
    let Some(timeout) = env(timeout_name).and_then(|t| Timeout::parse(&t).ok()) else {
        return die("timeout");
    };
    let extra = match loaded.extra_args(runtime, Path::new(scratch)) {
        Ok(text) => ExtraArgs::split(&text),
        Err(e) => return die(&e.to_string()),
    };
    let inputs = ContainerInputs {
        runtime,
        work: PathBuf::from(work),
        image: loaded.spec().image.clone(),
        command,
        extra_args: extra,
        timeout,
        limits,
    };
    let spec =
        if prefetch { ContainerSpec::prefetch(inputs) } else { ContainerSpec::verify(inputs) };
    match spec {
        Ok(spec) => {
            let mut out = std::io::stdout();
            for word in &spec.argv().into_vec() {
                out.write_all(word.as_bytes()).expect("stdout");
                out.write_all(&[0]).expect("stdout");
            }
            ExitCode::SUCCESS
        }
        Err(e) => die(&e.to_string()),
    }
}
