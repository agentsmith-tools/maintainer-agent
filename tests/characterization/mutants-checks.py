#!/usr/bin/env python3
"""Replay the mutation proof for the checks-family characterization cases.

  python3 tests/characterization/mutants-checks.py                  # every mutant
  python3 tests/characterization/mutants-checks.py claims-exit      # named mutants
  python3 tests/characterization/mutants-checks.py --check          # only validate the list

Each mutant is applied to a fresh, touched copy of the files the cases read (the
checkout itself is never edited). The old text must occur exactly once in its file,
the mutated file is read back and parsed, and the mutant runs against the case file
named for the program. A mutant is CAUGHT when the runner printed its summary and at
least one case printed FAIL. A run that printed no summary, ran out of time or address
space, or crashed is reported as its own outcome and never counts as caught.

The pristine copy runs first and last, and its tree hash is compared. A run takes 5 to
15 seconds, so the full list takes about 15 minutes. Run it as one heavy job.

One mutant of the list has no test that can kill it and is left out on purpose: dropping the `|| exit 1`
after the sweep's `cd` changes nothing observable, because the directory is the script's own and cannot be
missing while the script runs.

The mutants name the five programs under test: scripts/check_claims.sh,
scripts/verify-action-pins.sh, scripts/shellcheck-sweep.sh, evals/run-evals.sh and
tests/containment-probe.sh.
"""
import hashlib, os, re, shutil, subprocess, sys, tempfile, time

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
ITEMS = ["bin", "lib", "scripts", "evals", "profiles", "docs", "tests", "README.md", "CONTRIBUTING.md"]
CL, PN, SW, EV, PR = "scripts/check_claims.sh", "scripts/verify-action-pins.sh", "scripts/shellcheck-sweep.sh", "evals/run-evals.sh", "tests/containment-probe.sh"
CLC, PNC, SWC, EVC, PRC = "checks-claims.sh", "checks-pins.sh", "checks-sweep.sh", "checks-evals.sh", "checks-probe.sh"

# id, file, old text (exactly once), new text, case file
M = []
def m(mid, f, old, new, case): M.append((mid, f, old, new, case))

# --- check_claims.sh ---
m("claims-red-exit", CL, "head -10 >&2\n    exit 1", "head -10 >&2\n    exit 0", CLC)
m("claims-red-limit", CL, "head -10 >&2", "head -11 >&2", CLC)
m("claims-red-pattern", CL, "grep -E '^  FAIL' \"$suite_out\"", "grep -E '^ *FAIL' \"$suite_out\"", CLC)
m("claims-red-text", CL, "bad \"the test suite is RED; fix that before believing any number here\"", "bad \"the test suite is RED\"", CLC)
m("claims-suite-status-ignored", CL, "if ! bash \"$root/tests/run-tests.sh\" >\"$suite_out\" 2>&1; then", "if bash \"$root/tests/run-tests.sh\" >\"$suite_out\" 2>&1 && false; then", CLC)
m("claims-suite-stderr", CL, ">\"$suite_out\" 2>&1; then", ">\"$suite_out\"; then", CLC)
m("claims-no-summary-exit", CL, "refusing to pass over nothing\"; exit 1; }", "refusing to pass over nothing\"; exit 0; }", CLC)
m("claims-no-summary-text", CL, "could not measure the test count; refusing to pass over nothing", "could not measure the test count", CLC)
m("claims-pass-anchor", CL, "'s/^ *\\([0-9]\\+\\) passed.*/\\1/p'", "'s/ *\\([0-9]\\+\\) passed.*/\\1/p'", CLC)
m("claims-pass-first", CL, "passed.*/\\1/p' \"$suite_out\" | tail -1)", "passed.*/\\1/p' \"$suite_out\" | head -1)", CLC)
m("claims-skip-ignored", CL, "tests_actual=$(( tests_pass + ${tests_skip:-0} ))", "tests_actual=$(( tests_pass ))", CLC)
m("claims-skip-regex", CL, "'s/.*, \\([0-9]\\+\\) skipped.*/\\1/p'", "'s/.*, \\([0-9]\\+\\) skipped.*/\\1/'", CLC)
m("claims-skip-first", CL, "skipped.*/\\1/p' \"$suite_out\" | tail -1)", "skipped.*/\\1/p' \"$suite_out\" | head -1)", CLC)
m("claims-skip-note", CL, "skipped as the suite size", "skipped as the size", CLC)
m("claims-skip-note-condition", CL, "[ -n \"${tests_skip:-}\" ] && note", "[ \"${tests_skip:-0}\" -gt 0 ] && note", CLC)
m("claims-profile-glob", CL, "for pdj in \"$root\"/profiles/*/deny.json; do", "for pdj in \"$root\"/profiles/sysknife/deny.json; do", CLC)
m("claims-profile-home", CL, "\"$gen_dir/$pn\" /home/claimcheck >/dev/null 2>&1", "\"$gen_dir/$pn\" /home/other >/dev/null 2>&1", CLC)
m("claims-profile-fail-text", CL, "bad \"profile '$pn' does not generate a deny wall\"", "bad \"profile '$pn' fails\"", CLC)
m("claims-profile-fail-ignored", CL, "|| bad \"profile '$pn' does not generate a deny wall\"", "|| true", CLC)
m("claims-profile-note", CL, "note \"profile $pn: $(python3", "note \"profile $pn; $(python3", CLC)
m("claims-rules-note", CL, "those verbs generate $rules_here rules on this machine", "these verbs generate $rules_here rules on this machine", CLC)
m("claims-combined-source", CL, "cp \"$root/profiles/sysknife/deny.json\" \"$gen_dir/\"", "cp \"$root/profiles/magent/deny.json\" \"$gen_dir/\"", CLC)
m("claims-combined-home", CL, "python3 \"$root/scripts/render-settings.py\" \"$gen_dir\" /home/claimcheck", "python3 \"$root/scripts/render-settings.py\" \"$gen_dir\" /home/other", CLC)
m("claims-check-args", CL, "--check --tests \"$tests_actual\"", "--check --no-tests", CLC)
m("claims-values-args", CL, "--values --tests \"$tests_actual\"", "--values", CLC)
m("claims-figs-rc", CL, "if [ \"$figs_rc\" != 0 ]; then", "if [ \"$figs_rc\" -gt 1 ]; then", CLC)
m("claims-figs-report-stream", CL, "printf '%s\\n' \"$figs_out\" >&2\n    bad \"a figure", "printf '%s\\n' \"$figs_out\"\n    bad \"a figure", CLC)
m("claims-figs-text", CL, "run scripts/render_figures.py --write\"", "run scripts/render_figures.py --check\"", CLC)
m("claims-values-join", CL, "sed 's/,/, /g'", "sed 's/,/,/g'", CLC)
m("claims-values-fail-text", CL, "bad \"could not print the measured figures\"", "bad \"could not print the figures\"", CLC)
m("claims-values-fail-ignored", CL, "|| { printf '%s\\n' \"$vals\" >&2; bad \"could not print the measured figures\"; }", "|| true", CLC)
m("claims-exit", CL, "exit \"$fail\"", "exit 0", CLC)
m("claims-note-indent", CL, "note() { printf '  %s\\n' \"$1\"; }", "note() { printf ' %s\\n' \"$1\"; }", CLC)
m("claims-bad-stream", CL, "bad()  { printf 'FAIL: %s\\n' \"$1\" >&2; fail=1; }", "bad()  { printf 'FAIL: %s\\n' \"$1\"; fail=1; }", CLC)
m("claims-bad-no-fail", CL, "bad()  { printf 'FAIL: %s\\n' \"$1\" >&2; fail=1; }", "bad()  { printf 'FAIL: %s\\n' \"$1\" >&2; }", CLC)
m("claims-leak-removed", CL, "trap 'rm -rf \"$gen_dir\"' EXIT", "trap 'rm -rf \"$gen_dir\" \"$suite_out\"' EXIT", CLC)
m("claims-gen-kept", CL, "trap 'rm -rf \"$gen_dir\"' EXIT", "trap 'true' EXIT", CLC)
m("claims-root-physical", CL, "root=\"$(cd \"$(dirname \"${BASH_SOURCE[0]}\")/..\" && pwd)\"", "root=\"$(cd \"$(dirname \"${BASH_SOURCE[0]}\")/..\" && pwd -P)\"", CLC)

# --- verify-action-pins.sh ---
m("pins-no-workflows-exit", PN, "no .github/workflows under $root\" >&2; exit 2; }", "no .github/workflows under $root\" >&2; exit 1; }", PNC)
m("pins-no-workflows-text", PN, "verify-action-pins: no .github/workflows under", "verify-action-pins: no workflows under", PNC)
m("pins-no-files-exit", PN, "so nothing was scanned\" >&2\n    exit 2", "so nothing was scanned\" >&2\n    exit 1", PNC)
m("pins-no-composite-exit", PN, "so the composite scan read nothing\" >&2\n        exit 2", "so the composite scan read nothing\" >&2\n        exit 1", PNC)
m("pins-workflow-depth", PN, "find \"$root/.github/workflows\" -maxdepth 1 -type f", "find \"$root/.github/workflows\" -type f", PNC)
m("pins-workflow-yaml", PN, "-type f \\( -name '*.yml' -o -name '*.yaml' \\) | sort)", "-type f -name '*.yml' | sort)", PNC)
m("pins-actions-yaml", PN, "-name 'action.yml' -o -name 'action.yaml'", "-name 'action.yml'", PNC)
m("pins-actions-skipped", PN, "sources+=(\"${found[@]}\")", "true", PNC)
m("pins-examples-depth", PN, "find \"$root/examples\" -maxdepth 1 -type f", "find \"$root/examples\" -type f", PNC)
m("pins-examples-skipped", PN, "find \"$root/examples\" -maxdepth 1 -type f -name '*.yml' 2>/dev/null | sort", "true", PNC)
m("pins-unparsed-comment", PN, "*# *\\S+$'", "*# *\\S*$'", PNC)
m("pins-unparsed-dash", PN, "'^[[:space:]]*(-[[:space:]]*)?uses:'", "'^[[:space:]]*uses:'", PNC)
m("pins-unparsed-text", PN, "not SHA-pinned with a version comment, so nothing below checks them:", "not pinned, so nothing below checks them:", PNC)
m("pins-unparsed-indent", PN, "sed 's/^/            /' >&2", "sed 's/^/          /' >&2", PNC)
m("pins-unparsed-no-fail", PN, ">&2\n    fail=1\nfi", ">&2\nfi", PNC)
m("pins-unparsed-dedupe", PN, "| sort -u \\\n            | grep -vE", "| sort \\\n            | grep -vE", PNC)
m("pins-unparsed-trim", PN, "sed 's/[[:space:]]*$//'", "sed 's/ *$//'", PNC)
m("pins-annotated-test", PN, "if [ \"$type\" = tag ]; then", "if [ \"$type\" != tag ]; then", PNC)
m("pins-deref-fail-ignored", PN, "--jq '.object.sha' 2>/dev/null)\" || return 1", "--jq '.object.sha' 2>/dev/null)\" || true", PNC)
m("pins-deref-stderr", PN, "--jq '.object.sha' 2>/dev/null)\" || return 1", "--jq '.object.sha')\" || return 1", PNC)
m("pins-type-stderr", PN, "--jq '.object.type' 2>/dev/null)", "--jq '.object.type')", PNC)
m("pins-tagobj-stderr", PN, "gh api \"repos/$1/git/tags/$obj\" --jq '.object.sha' 2>/dev/null", "gh api \"repos/$1/git/tags/$obj\" --jq '.object.sha'", PNC)
m("pins-branch-master", PN, "if [ \"$claim\" = main ] || [ \"$claim\" = master ]; then", "if [ \"$claim\" = main ]; then", PNC)
m("pins-branch-word", PN, "if [ \"$claim\" = main ] || [ \"$claim\" = master ]; then", "if [[ \"$claim\" = main* ]] || [ \"$claim\" = master ]; then", PNC)
m("pins-branch-text", PN, "tracks %s by design", "tracks %s", PNC)
m("pins-branch-pad", PN, "'  branch  %-46s tracks", "'  branch  %-40s tracks", PNC)
m("pins-repo-cut", PN, "cut -d/ -f1,2", "cut -d/ -f1", PNC)
m("pins-ok-text", PN, "'  ok      %-46s %s is %s\\n'", "'  ok      %-46s %s = %s\\n'", PNC)
m("pins-ok-sha-cut", PN, "\"$action\" \"${sha:0:8}\" \"$claim\"\n    else", "\"$action\" \"${sha:0:7}\" \"$claim\"\n    else", PNC)
m("pins-notag-text", PN, "which is not a tag in %s", "which is no tag in %s", PNC)
m("pins-notag-no-fail", PN, "\"$action\" \"$claim\" \"$repo\"; fail=1", "\"$action\" \"$claim\" \"$repo\"", PNC)
m("pins-mismatch-cut", PN, "\"${sha:0:8}\" \"$claim\" \"${real:0:8}\"", "\"${sha:0:8}\" \"$claim\" \"${real:0:7}\"", PNC)
m("pins-mismatch-no-fail", PN, "\"${real:0:8}\"; fail=1", "\"${real:0:8}\"", PNC)
m("pins-final-ok", PN, "echo \"  every pin is the tag it claims\"", "echo \"  every pin is its tag\"", PNC)
m("pins-final-bad-stream", PN, "echo \"  a pin does not match its comment\" >&2", "echo \"  a pin does not match its comment\"", PNC)
m("pins-exit", PN, "exit \"$fail\"", "exit 0", PNC)
m("pins-final-sort", PN, "| sed 's/^uses: //' | sort -u)", "| sed 's/^uses: //')", PNC)
m("pins-root-argument", PN, "root=\"${1:-$(cd \"$(dirname \"${BASH_SOURCE[0]}\")/..\" && pwd)}\"", "root=\"$(cd \"$(dirname \"${BASH_SOURCE[0]}\")/..\" && pwd)\"", PNC)
m("pins-claim-extract", PN, "sed 's/.*# *//'", "sed 's/.*# //'", PNC)
m("pins-sha-extract", PN, "sha=\"${rest%% *}\"", "sha=\"${rest%%#*}\"", PNC)

# --- shellcheck-sweep.sh ---
m("sweep-shebang-sh", SW, "|*\"#!/bin/sh\"*) printf", ") printf", SWC)
m("sweep-shebang-bin-bash", SW, "|*\"#!/bin/bash\"*", "", SWC)
m("sweep-shebang-env", SW, "*\"#!/usr/bin/env bash\"*|", "", SWC)
m("sweep-head-wide", SW, "head -c 80", "head -c 120", SWC)
m("sweep-head-narrow", SW, "head -c 80", "head -c 60", SWC)
m("sweep-nul-kept", SW, "| tr -d '\\0')\"", "| cat)\"", SWC)
m("sweep-verify-d-glob", SW, "profiles/*/verify.d/*.sh) printf", "profiles/*/verify.d/*.bash) printf", SWC)
m("sweep-verify-d-fixed", SW, "profiles/*/verify.d/*.sh) printf", "profiles/p/verify.d/*.sh) printf", SWC)
m("sweep-unique", SW, "done | sort -u", "done | sort", SWC)
m("sweep-count-text", SW, "'checking %d scripts\\n'", "'checking %d script(s)\\n'", SWC)
m("sweep-empty-exit", SW, "this gate inspected nothing\" >&2; exit 1; }", "this gate inspected nothing\" >&2; exit 0; }", SWC)
m("sweep-empty-gate", SW, "[ \"${#files[@]}\" -gt 0 ] ||", "[ \"${#files[@]}\" -ge 0 ] ||", SWC)
m("sweep-no-linter-exit", SW, "    exit 127", "    exit 126", SWC)
m("sweep-no-linter-hint", SW, "'CI runs it either way, so a push will find what this did not.'", "'CI runs it anyway.'", SWC)
m("sweep-no-linter-stream", SW, "this did not.' >&2", "this did not.'", SWC)
m("sweep-need-default", SW, "${MAGENT_SWEEP_MIN_AVAIL_KB:-10485760}", "${MAGENT_SWEEP_MIN_AVAIL_KB:-10485761}", SWC)
m("sweep-need-compare", SW, "[ \"$have\" -ge \"$need_kb\" ] && break", "[ \"$have\" -gt \"$need_kb\" ] && break", SWC)
m("sweep-wait-compare", SW, "if [ \"$waited\" -ge \"$wait_s\" ]; then", "if [ \"$waited\" -gt \"$wait_s\" ]; then", SWC)
m("sweep-giveup-exit", SW, "set MAGENT_SWEEP_MIN_AVAIL_KB.\\n' >&2\n                exit 75", "set MAGENT_SWEEP_MIN_AVAIL_KB.\\n' >&2\n                exit 74", SWC)
m("sweep-noavail-exit", SW, "\"$meminfo\" \"$need_kb\" >&2\n                    exit 75 ;;", "\"$meminfo\" \"$need_kb\" >&2\n                    exit 74 ;;", SWC)
m("sweep-giveup-text", SW, "gave up after %ss.", "gave up after %s s.", SWC)
m("sweep-noavail-text", SW, "no usable MemAvailable in %s", "no MemAvailable in %s", SWC)
m("sweep-nonnumeric", SW, "''|*[!0-9]*)", "'')", SWC)
m("sweep-poll-step", SW, "waited=$((waited + poll_s))", "waited=$((waited + 1))", SWC)
m("sweep-poll-sleep", SW, "sleep \"$poll_s\"", "sleep 1", SWC)
m("sweep-first-line", SW, "then printf '%s' \"$v\"; return 0; fi", "then printf '%s' \"$v\"; fi", SWC)
m("sweep-meminfo-unreadable", SW, "is not readable here; not checking free memory.", "is not readable here.", SWC)
m("sweep-ci-empty", SW, "if [ -z \"${CI:-}\" ]; then\n    if [ -r \"$meminfo\" ]; then", "if [ -z \"${CI+x}\" ]; then\n    if [ -r \"$meminfo\" ]; then", SWC)
m("sweep-ci-cap", SW, "if [ -z \"${CI:-}\" ] && [ -z \"${MAGENT_SWEEP_NO_CAP:-}\" ]; then", "if [ -z \"${MAGENT_SWEEP_NO_CAP:-}\" ]; then", SWC)
m("sweep-no-cap-empty", SW, "[ -z \"${MAGENT_SWEEP_NO_CAP:-}\" ]; then", "[ -z \"${MAGENT_SWEEP_NO_CAP+x}\" ]; then", SWC)
m("sweep-cap-default", SW, "${MAGENT_SWEEP_MEM_MAX:-12G}", "${MAGENT_SWEEP_MEM_MAX:-13G}", SWC)
m("sweep-probe-swap", SW, "-p \"MemoryMax=$cap\" -p MemorySwapMax=0 true", "-p \"MemoryMax=$cap\" true", SWC)
m("sweep-runner-swap", SW, "runner=(systemd-run --user --scope --quiet -p \"MemoryMax=$cap\" -p MemorySwapMax=0)", "runner=(systemd-run --user --scope --quiet -p \"MemoryMax=$cap\")", SWC)
m("sweep-no-cap-text", SW, "relying on the free-memory check.", "relying on the memory check.", SWC)
m("sweep-nice-value", SW, "runner+=(nice -n 15)", "runner+=(nice -n 10)", SWC)
m("sweep-nice-test", SW, "if command -v nice >/dev/null 2>&1; then runner+=(nice -n 15); fi", "if false; then runner+=(nice -n 15); fi", SWC)
m("sweep-severity", SW, "--severity=warning", "--severity=error", SWC)
m("sweep-rc-max", SW, "[ \"$r\" -gt \"$rc\" ] && rc=$r", "rc=$r", SWC)
m("sweep-rc-137", SW, "if [ \"$r\" -eq 137 ]; then", "if [ \"$r\" -ge 137 ]; then", SWC)
m("sweep-rc-137-break", SW, "\"$f\" \"$cap\" >&2\n        break", "\"$f\" \"$cap\" >&2\n        :", SWC)
m("sweep-kill-text", SW, "most likely by the MemoryMax=%s cap", "most likely by the cap %s", SWC)
m("sweep-cd-physical", SW, "cd \"$(dirname \"$(readlink -f \"${BASH_SOURCE[0]}\")\")/..\" || exit 1", "cd \"$(dirname \"${BASH_SOURCE[0]}\")/..\" || exit 1", SWC)

# --- evals/run-evals.sh ---
m("evals-corpus-deny", EV, "cat \"$pdir/deny.json\" 2>/dev/null", "true", EVC)
m("evals-corpus-settings", EV, "cat \"$pdir/settings.json\" 2>/dev/null)\"", "true)\"", EVC)
m("evals-corpus-rehearsal", EV, "cat \"$pdir/settings.json\" 2>/dev/null)\"", "cat \"$pdir\"/settings*.json 2>/dev/null)\"", EVC)
m("evals-corpus-stderr", EV, "--show-prompt \"$profile\" review 2>/dev/null", "--show-prompt \"$profile\" review", EVC)
m("evals-corpus-task", EV, "--show-prompt \"$profile\" review", "--show-prompt \"$profile\" issues", EVC)
m("evals-empty-corpus-exit", EV, "echo \"run-evals: empty corpus for profile '$profile'\" >&2; exit 1; }", "echo \"run-evals: empty corpus for profile '$profile'\" >&2; exit 0; }", EVC)
m("evals-profile-default-empty", EV, "${MAINTAINER_PROFILE:-sysknife}", "${MAINTAINER_PROFILE-sysknife}", EVC)
m("evals-na-before-needle", EV, "if [ -n \"${NA[$id]:-}\" ]; then", "if [ -n \"${NA[$id]:-}\" ] && [ -z \"$needle\" ]; then", EVC)
m("evals-underscore", EV, "if k.startswith('_'): continue", "if k.startswith('__'): continue", EVC)
m("evals-separator", EV, "IFS=$'\\x1f' read -r id needle na", "IFS=$'\\t' read -r id needle na", EVC)
m("evals-needle-null", EV, "v.get('needle') or ''", "v.get('needle') or 'None'", EVC)
m("evals-na-null", EV, "v.get('n/a') or ''", "v.get('n/a') or 'None'", EVC)
m("evals-retirable", EV, "RETIRABLE=\"05-reserved-issue\"", "RETIRABLE=\"05-reserved-issue 01-alpha\"", EVC)
m("evals-ghost-na", EV, "for id in \"${!RULE[@]}\" \"${!NA[@]}\"; do", "for id in \"${!RULE[@]}\"; do", EVC)
m("evals-ghost-needle", EV, "for id in \"${!RULE[@]}\" \"${!NA[@]}\"; do", "for id in \"${!NA[@]}\"; do", EVC)
m("evals-fail-stream", EV, "bad() { printf '  FAIL  %s\\n' \"$1\" >&2;", "bad() { printf '  FAIL  %s\\n' \"$1\";", EVC)
m("evals-summary-text", EV, "'\\n  %d passed, %d failed\\n'", "'\\n  %d ok, %d failed\\n'", EVC)
m("evals-static-exit", EV, "[ \"$fail\" -eq 0 ] ;;", "true ;;", EVC)
m("evals-na-text", EV, "not applicable to '$profile': ${NA[$id]}", "n/a for '$profile': ${NA[$id]}", EVC)
m("evals-usage-exit", EV, "usage: run-evals.sh [--static|--live [id]]\" >&2; exit 64", "usage: run-evals.sh [--static|--live [id]]\" >&2; exit 2", EVC)
m("evals-static-arg", EV, "\"\"|--static) static;", "\"\"|--static|static) static;", EVC)
m("evals-live-filter", EV, "[[ \"$id\" != *\"$only\"* ]] && continue", "[[ \"$id\" != $only ]] && continue", EVC)
m("evals-live-model", EV, "\"${MODEL_review:-opus}\"", "\"${MODEL_review:-sonnet}\"", EVC)
m("evals-live-task", EV, "MAINTAINER_TASK=\"${MAINTAINER_TASK:-review}\"", "MAINTAINER_TASK=\"review\"", EVC)
m("evals-live-cut", EV, "sed '/^## Where the rule lives/,$d'", "sed '/^## Why this scenario/,$d'", EVC)
m("evals-live-words", EV, "Answer in under 200 words.", "Answer in under 100 words.", EVC)
m("evals-live-cleanup", EV, "rm -f \"$out\"", "true", EVC)
m("evals-live-note", EV, "Live mode prints answers; it does not score them. Read them against the", "Live mode prints answers. Read them against the", EVC)
m("evals-live-fail-ignored", EV, "|| bad \"$id: backend failed\"", "|| true", EVC)
m("evals-live-backend-default", EV, "${BACKEND:-claude}", "${BACKEND:-codex}", EVC)
m("evals-live-header", EV, "printf '\\n--- %s ---\\n' \"$id\"", "printf -- '--- %s ---\\n' \"$id\"", EVC)
m("evals-live-preamble", EV, "cat \"$pdir/prompts/common-preamble.md\"; printf '\\n\\n'", "cat \"$pdir/prompts/common-preamble.md\"; printf '\\n'", EVC)
m("evals-live-style", EV, "cat \"$root/lib/prose-style.md\"; printf '\\n\\n'", "printf '\\n\\n'", EVC)
m("evals-live-root-arg", EV, "/dev/stdout \"$root\"", "/dev/stdout \"$pdir\"", EVC)
m("evals-live-dest", EV, "\"${MODEL_review:-opus}\" /dev/stdout", "\"${MODEL_review:-opus}\" /dev/stderr", EVC)

# --- containment-probe.sh ---
m("probe-tcp-verdict", PR, "grep -q 'rc=0' && bad \"outbound TCP", "grep -q 'rc=9' && bad \"outbound TCP", PRC)
m("probe-dns-verdict", PR, "grep -q 'rc=0' && bad \"DNS resolution\"", "grep -q 'rc=9' && bad \"DNS resolution\"", PRC)
m("probe-etc-verdict", PR, "grep -q 'rc=0' && bad \"writing to /etc", "grep -q 'rc=9' && bad \"writing to /etc", PRC)
m("probe-balloon-verdict", PR, "grep -q 'rc=0' && bad \"allocating 4G", "grep -q 'rc=9' && bad \"allocating 4G", PRC)
m("probe-exec-anchor", PR, "grep -q '^ran'", "grep -q 'ran'", PRC)
m("probe-caps-end", PR, "CapEff:\\s*0+$", "CapEff:\\s*0+", PRC)
m("probe-caps-expr", PR, "CapEff:\\s*0+$", "CapEff:\\s*0*$", PRC)
m("probe-caps-cut", PR, "cut -c1-60", "cut -c1-40", PRC)
m("probe-nnp-pattern", PR, "'NoNewPrivs:.*1'", "'NoNewPrivs:.*[01]'", PRC)
m("probe-nnp-text", PR, "NoNewPrivs is not set: $out", "NoNewPrivs is off: $out", PRC)
m("probe-fork-boundary", PR, "[ \"$spawned\" -lt 1000 ]", "[ \"$spawned\" -lt 1001 ]", PRC)
m("probe-fork-boundary-low", PR, "[ \"$spawned\" -lt 1000 ]", "[ \"$spawned\" -lt 999 ]", PRC)
m("probe-fork-last", PR, "\\1/p' | tail -1)", "\\1/p' | head -1)", PRC)
m("probe-fork-empty", PR, "if [ -z \"$spawned\" ]; then", "if [ \"$spawned\" = x ]; then", PRC)
m("probe-creds-pattern", PR, "grep -q PROBECANARY", "grep -q PROBECANARX", PRC)
m("probe-creds-env", PR, "GH_TOKEN=ghp_PROBECANARY0000 ANTHROPIC_API_KEY=sk-PROBECANARY", "GH_TOKEN=ghp_PROBECANARY0000", PRC)
m("probe-escape-path", PR, "[ -e \"$work/../escape\" ]", "[ -e \"$work/escape\" ]", PRC)
m("probe-ps-compare", PR, "[ \"$after\" -le \"$before\" ]", "[ \"$after\" -lt \"$before\" ]", PRC)
m("probe-ps-count", PR, "$((after-before))", "$((after))", PRC)
m("probe-sleep", PR, "\nsleep 3\n", "\nsleep 2\n", PRC)
m("probe-timeout", PR, "\"${PROBE_TIMEOUT:-15}\"", "\"${PROBE_TIMEOUT:-16}\"", PRC)
m("probe-image-default", PR, "${PROBE_IMAGE:-docker.io/library/bash:5}", "${PROBE_IMAGE:-docker.io/library/bash:4}", PRC)
m("probe-pull-quiet", PR, "pull -q \"$IMG\"", "pull \"$IMG\"", PRC)
m("probe-no-runtime-exit", PR, "this probe measured nothing\" >&2; exit 1", "this probe measured nothing\" >&2; exit 0", PRC)
m("probe-obtain-exit", PR, "cannot obtain $IMG\" >&2; exit 1", "cannot obtain $IMG\" >&2; exit 0", PRC)
m("probe-flag-network", PR, "        --network=none \\\n", "", PRC)
m("probe-flag-caps", PR, "        --cap-drop=ALL \\\n", "", PRC)
m("probe-flag-privs", PR, "        --security-opt=no-new-privileges \\\n", "", PRC)
m("probe-flag-readonly", PR, "        --read-only \\\n", "", PRC)
m("probe-flag-tmpfs", PR, "rw,noexec,nosuid,size=256m", "rw,nosuid,size=256m", PRC)
m("probe-flag-pids", PR, "--pids-limit 512", "--pids-limit 513", PRC)
m("probe-flag-memory", PR, "--memory 2g", "--memory 4g", PRC)
m("probe-flag-cpus", PR, "--cpus 2", "--cpus 3", PRC)
m("probe-mount-flag", PR, "-v \"$work:/repo:z\"", "-v \"$work:/repo\"", PRC)
m("probe-workdir", PR, "-w /repo \"$IMG\"", "-w /tmp \"$IMG\"", PRC)
m("probe-stderr-merge", PR, "sh -c \"$1\" 2>&1", "sh -c \"$1\"", PRC)
m("probe-repofile", PR, "printf 'host file\\n' > \"$work/repofile\"", "printf 'other\\n' > \"$work/repofile\"", PRC)
m("probe-cleanup", PR, "trap 'rm -rf \"$work\"' EXIT", "trap 'true' EXIT", PRC)
m("probe-payload-tcp", PR, "timeout 5 bash -c", "timeout 6 bash -c", PRC)
m("probe-payload-balloon", PR, "head -c 4G /dev/zero", "head -c 3G /dev/zero", PRC)
m("probe-payload-fork", PR, "-lt 4000", "-lt 3000", PRC)
m("probe-gate-flags-list", PR, "'--pids-limit' '--memory' '--cpus'; do", "'--pids-limit' '--memory'; do", PRC)
m("probe-gate-file", PR, "/../bin/maintainer-merge", "/../bin/maintainer-doctor", PRC)
m("probe-gate-text", PR, "this probe is measuring something else", "this probe measures something else", PRC)
m("probe-totals", PR, "'\\n  %d stopped, %d got through\\n'", "'\\n  %d ok, %d got through\\n'", PRC)
m("probe-exit", PR, "[ \"$fail\" -eq 0 ]\n", "true\n", PRC)
m("probe-stopped-colour", PR, "\\033[32mstopped", "\\033[33mstopped", PRC)
m("probe-through-colour", PR, "\\033[31mGOT THROUGH", "\\033[33mGOT THROUGH", PRC)
m("probe-heading", PR, "echo \"== what an adversarial pull request would try, inside the gate ==\"", "echo \"== what an adversarial pull request would try ==\"", PRC)
m("probe-docker-preferred", PR, "command -v podman >/dev/null 2>&1 && rt=podman", "command -v docker >/dev/null 2>&1 && rt=docker", PRC)


def tree_hash(root):
    h = hashlib.sha256()
    for it in ITEMS:
        p = os.path.join(root, it)
        files = [p] if os.path.isfile(p) else sorted(os.path.join(d, f) for d, _, fs in os.walk(p) for f in fs)
        for f in files:
            if os.path.islink(f):
                continue
            h.update(f[len(root):].encode()); h.update(open(f, "rb").read())
    return h.hexdigest()


def fresh(work):
    shutil.rmtree(work, ignore_errors=True)
    os.makedirs(work)
    for it in ITEMS:
        src, dst = os.path.join(ROOT, it), os.path.join(work, it)
        if os.path.isdir(src):
            shutil.copytree(src, dst, symlinks=True)
        else:
            shutil.copy2(src, dst)
    shutil.rmtree(os.path.join(work, "tests", "diff"), ignore_errors=True)
    for base, _, fs in os.walk(work):
        for f in fs:
            if not os.path.islink(os.path.join(base, f)):
                os.utime(os.path.join(base, f))
    time.sleep(1.1)


def run(work, case_file):
    env = {k: v for k, v in os.environ.items() if k != "MX_TREE"}
    try:
        cp = subprocess.run(["bash", "-c", "ulimit -v 8000000; exec bash tests/run-characterization.sh " + case_file],
                            cwd=work, capture_output=True, text=True, env=env, timeout=300)
    except subprocess.TimeoutExpired:
        return "TIMEOUT", []
    out = cp.stdout + cp.stderr
    found = re.findall(r"^(\d+) cases, (\d+) passed, (\d+) failed", out, re.M)
    fails = [l[6:] for l in out.splitlines() if l.startswith("FAIL  ")]
    summary = re.search(r"^files run: \d+$", out, re.M)
    if not found or not summary:
        return "NO-SUMMARY", fails
    return tuple(map(int, found[-1])), fails


def parses(path):
    return subprocess.run(["bash", "-n", path], capture_output=True).returncode == 0


def main():
    args = sys.argv[1:]
    only_check = "--check" in args
    want = [a for a in args if not a.startswith("--")]
    ids = [x[0] for x in M]
    dupes = {i for i in ids if ids.count(i) > 1}
    if dupes:
        sys.exit("duplicate mutant ids: " + ", ".join(sorted(dupes)))
    unknown = [w for w in want if w not in ids]
    if unknown:
        sys.exit("unknown mutant: " + ", ".join(unknown))
    tmp = tempfile.mkdtemp(prefix="mut-checks-", dir=os.path.expanduser("~/.cache/magent-wt-scratch") if os.path.isdir(os.path.expanduser("~/.cache/magent-wt-scratch")) else None)
    work = os.path.join(tmp, "w")
    # Every assertion comes before the first edit.
    bad = 0
    for mid, path, old, new, case_file in M:
        s = open(os.path.join(ROOT, path)).read()
        n = s.count(old)
        if n != 1:
            print("!! %s: the old text occurs %d times in %s" % (mid, n, path)); bad += 1
        elif old == new:
            print("!! %s: the new text equals the old" % mid); bad += 1
        elif not os.path.isfile(os.path.join(ROOT, "tests/characterization", case_file)):
            print("!! %s: no case file %s" % (mid, case_file)); bad += 1
    if bad:
        sys.exit("%d mutant definitions are wrong; nothing was run" % bad)
    print("%d mutants defined, every old text occurs exactly once" % len(M))
    if only_check:
        shutil.rmtree(tmp, ignore_errors=True)
        return 0
    fresh(work)
    pristine_hash = tree_hash(work)
    base = {}
    for cf in sorted({x[4] for x in M}):
        base[cf] = run(work, cf)
        print("pristine control before, %s: %s" % (cf, base[cf][0]))
        if not (isinstance(base[cf][0], tuple) and base[cf][0][0] > 0 and base[cf][0][2] == 0):
            bad += 1
    print("%-30s %-34s %s" % ("mutant", "file:line", "result (first red case)"))
    survived = 0; uncounted = 0
    for mid, path, old, new, case_file in M:
        if want and mid not in want:
            continue
        fresh(work)
        p = os.path.join(work, path)
        s = open(p).read()
        assert s.count(old) == 1, mid
        line = s[:s.index(old)].count("\n") + 1
        open(p, "w").write(s.replace(old, new))
        back = open(p).read()
        assert back != s and new in back, mid + ": the mutation did not land"
        if not parses(p):
            print("%-30s %-34s %s" % (mid, "%s:%d" % (path.split("/")[-1], line), "DOES NOT PARSE")); bad += 1; continue
        r, fails = run(work, case_file)
        if isinstance(r, str):
            uncounted += 1
            print("%-30s %-34s %s" % (mid, "%s:%d" % (path.split("/")[-1], line), r + " (not counted as caught)"), flush=True)
            continue
        caught = r[2] > 0 and bool(fails)
        if not caught:
            survived += 1
        print("%-30s %-34s %s" % (mid, "%s:%d" % (path.split("/")[-1], line),
              ("CAUGHT by %s (%d red) %s" % (case_file, r[2], fails[0][:64])) if caught else "SURVIVED (%s)" % (r,)), flush=True)
    fresh(work)
    for cf in sorted({x[4] for x in M}):
        res = run(work, cf)
        ok = isinstance(res[0], tuple) and res[0][2] == 0
        print("pristine control after, %s: %s" % (cf, res[0]))
        if not ok:
            bad += 1
    same = tree_hash(work) == pristine_hash
    print("tree hash unchanged by the controls: %s" % same)
    if not same:
        bad += 1
    print("mutants run: %d, survived: %d, not counted: %d" % (len(M) if not want else len(want), survived, uncounted))
    shutil.rmtree(tmp, ignore_errors=True)
    return 1 if (bad or survived or uncounted) else 0


if __name__ == "__main__":
    sys.exit(main())
