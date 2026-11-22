#!/usr/bin/env bash
# Characterization: scripts/verify-action-pins.sh. Every `uses:` in the workflows, the
# local composite actions and the examples must be pinned to a 40-character SHA with a
# version comment, and the comment must be the tag that SHA is. The cases pin each
# message, the stream it goes to, the exit code, the files the scan reads, and the argv
# and order of the `gh api` calls. gh is a stub that answers from a table; nothing
# reaches GitHub.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib-checks.sh"
cc_init

A=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
B=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
C=cccccccccccccccccccccccccccccccccccccccc
T1=1111111111111111111111111111111111111111

ok_line()     { printf '  ok      %-46s %s is %s' "$1" "${2:0:8}" "$3"; }
branch_line() { printf '  branch  %-46s tracks %s by design' "$1" "$2"; }
fail_mismatch() { printf '  FAIL    %-46s pinned %s but %s is %s' "$1" "${2:0:8}" "$3" "${4:0:8}"; }
fail_not_tag()  { printf '  FAIL    %-46s claims %s, which is not a tag in %s' "$1" "$2" "$3"; }
ALL_OK="  every pin is the tag it claims"
SOME_BAD="  a pin does not match its comment"
UNPARSED_HEAD="  FAIL    not SHA-pinned with a version comment, so nothing below checks them:"

# pin_case_start: a tree, the stub gh, and one workflow.
pin_start() { cc_pins_tree; cc_gh_pins; }
sha_call()  { cz_argv api "repos/$1/git/ref/tags/$2" --jq .object.sha; }
type_call() { cz_argv api "repos/$1/git/ref/tags/$2" --jq .object.type; }
tag_call()  { cz_argv api "repos/$1/git/tags/$2" --jq .object.sha; }

case_lightweight_ok() {
    pin_start
    cc_workflow .github/workflows/ci.yml "jobs:" "  a:" "    steps:" "      - uses: actions/checkout@$A # v4"
    cc_tag actions/checkout v4 "$A"
    cc_pins
    cz_expect_rc 0
    cz_expect_stderr_empty
    cz_expect_stdout "$(ok_line actions/checkout "$A" v4)" "$ALL_OK"
    cz_expect_gh_log "$(sha_call actions/checkout v4)" "$(type_call actions/checkout v4)"
}
cc_case "a pin whose tag is a lightweight tag on that SHA passes after two gh calls" case_lightweight_ok

case_annotated_ok() {
    pin_start
    cc_workflow .github/workflows/ci.yml "      - uses: actions/checkout@$A # v4"
    cc_atag actions/checkout v4 "$T1" "$A"
    cc_pins
    cz_expect_rc 0
    cz_expect_stdout "$(ok_line actions/checkout "$A" v4)" "$ALL_OK"
    cz_expect_gh_log "$(sha_call actions/checkout v4)" "$(type_call actions/checkout v4)" "$(tag_call actions/checkout "$T1")"
}
cc_case "a pin whose tag is annotated is followed to the commit through a third gh call and passes" case_annotated_ok

case_annotated_object_not_the_pin() {
    pin_start
    cc_workflow .github/workflows/ci.yml "      - uses: actions/checkout@$T1 # v4"
    cc_atag actions/checkout v4 "$T1" "$A"
    cc_pins
    cz_expect_rc 1
    cz_expect_stdout "$(fail_mismatch actions/checkout "$T1" v4 "$A")"
    cz_expect_stderr "$SOME_BAD"
}
cc_case "a pin set to the annotated tag object instead of the commit is a mismatch" case_annotated_object_not_the_pin

case_mismatch() {
    pin_start
    cc_workflow .github/workflows/ci.yml "      - uses: actions/checkout@$A # v4"
    cc_tag actions/checkout v4 "$B"
    cc_pins
    cz_expect_rc 1
    cz_expect_stdout "$(fail_mismatch actions/checkout "$A" v4 "$B")"
    cz_expect_stderr "$SOME_BAD"
}
cc_case "a pin whose comment names a tag on another SHA fails: the FAIL line is on stdout, the verdict on stderr, exit 1" case_mismatch

case_not_a_tag() {
    pin_start
    cc_workflow .github/workflows/ci.yml "      - uses: actions/checkout@$A # v9"
    cc_gh_answer repos/actions/checkout/git/ref/tags/v9 .object.sha 1 ""
    cc_pins
    cz_expect_rc 1
    cz_expect_stdout "$(fail_not_tag actions/checkout v9 actions/checkout)"
    cz_expect_stderr "$SOME_BAD"
    cz_expect_gh_log "$(sha_call actions/checkout v9)"
}
cc_case "a comment that names no tag fails after one gh call, and the type is never asked" case_not_a_tag

case_gh_stderr_hidden() {
    pin_start
    cc_workflow .github/workflows/ci.yml "      - uses: actions/checkout@$A # v9"
    cc_gh_answer repos/actions/checkout/git/ref/tags/v9 .object.sha 1 ""
    cc_pins
    cz_expect_lacks stderr "HTTP 404"
}
cc_case "the stderr of a failing gh call is discarded" case_gh_stderr_hidden

case_gh_type_stderr_hidden() {
    pin_start
    cc_workflow .github/workflows/ci.yml "      - uses: actions/checkout@$A # v4"
    cc_gh_answer repos/actions/checkout/git/ref/tags/v4 .object.sha 0 "$A"
    cc_gh_answer repos/actions/checkout/git/ref/tags/v4 .object.type 1 ""
    cc_pins
    cz_expect_rc 0
    cz_expect_lacks stderr "HTTP 404"
    cz_expect_stderr_empty
}
cc_case "the stderr of a failing type call is discarded too" case_gh_type_stderr_hidden

case_annotated_stderr_hidden() {
    pin_start
    cc_workflow .github/workflows/ci.yml "      - uses: actions/checkout@$A # v4"
    cc_gh_answer repos/actions/checkout/git/ref/tags/v4 .object.sha 0 "$T1"
    cc_gh_answer repos/actions/checkout/git/ref/tags/v4 .object.type 0 tag
    cc_gh_answer "repos/actions/checkout/git/tags/$T1" .object.sha 1 ""
    cc_pins
    cz_expect_rc 1
    cz_expect_stderr "$SOME_BAD"
}
cc_case "the stderr of a failing tag-object call is discarded too" case_annotated_stderr_hidden

case_gh_missing() {
    pin_start
    cc_drop_tool gh
    cc_workflow .github/workflows/ci.yml "      - uses: actions/checkout@$A # v4"
    cc_pins
    cz_expect_rc 1
    # Pins current behaviour: a missing gh reads as "not a tag", for every pin, and says nothing else.
    cz_expect_stdout "$(fail_not_tag actions/checkout v4 actions/checkout)"
    cz_expect_stderr "$SOME_BAD"
}
cc_case "with no gh on PATH every pin reads as not a tag (pins current behaviour)" case_gh_missing

case_type_call_fails() {
    pin_start
    cc_workflow .github/workflows/ci.yml "      - uses: actions/checkout@$A # v4"
    cc_gh_answer repos/actions/checkout/git/ref/tags/v4 .object.sha 0 "$A"
    cc_gh_answer repos/actions/checkout/git/ref/tags/v4 .object.type 1 ""
    cc_pins
    cz_expect_rc 0
    cz_expect_stdout "$(ok_line actions/checkout "$A" v4)" "$ALL_OK"
    cz_expect_gh_log "$(sha_call actions/checkout v4)" "$(type_call actions/checkout v4)"
}
cc_case "a failing type call is ignored and the ref's object is compared as it is (pins current behaviour)" case_type_call_fails

case_annotated_deref_fails() {
    pin_start
    cc_workflow .github/workflows/ci.yml "      - uses: actions/checkout@$A # v4"
    cc_gh_answer repos/actions/checkout/git/ref/tags/v4 .object.sha 0 "$T1"
    cc_gh_answer repos/actions/checkout/git/ref/tags/v4 .object.type 0 tag
    cc_gh_answer "repos/actions/checkout/git/tags/$T1" .object.sha 1 ""
    cc_pins
    cz_expect_rc 1
    cz_expect_stdout "$(fail_not_tag actions/checkout v4 actions/checkout)"
}
cc_case "an annotated tag whose object cannot be read reads as not a tag" case_annotated_deref_fails

case_other_object_type() {
    pin_start
    cc_workflow .github/workflows/ci.yml "      - uses: actions/checkout@$A # v4"
    cc_gh_answer repos/actions/checkout/git/ref/tags/v4 .object.sha 0 "$A"
    cc_gh_answer repos/actions/checkout/git/ref/tags/v4 .object.type 0 blob
    cc_pins
    cz_expect_rc 0
    cz_expect_gh_count "git/tags/" 0
}
cc_case "any object type other than 'tag' is compared directly, without a third call" case_other_object_type

case_branch_pins() {
    pin_start
    cc_workflow .github/workflows/ci.yml "      - uses: acme/rolling@$A # main" "      - uses: acme/old@$B # master"
    cc_pins
    cz_expect_rc 0
    cz_expect_stdout "$(branch_line acme/old master)" "$(branch_line acme/rolling main)" "$ALL_OK"
    cz_expect_gh_log
}
cc_case "a pin that claims main or master is reported as a branch by design and asks GitHub nothing" case_branch_pins

case_branch_word_exact() {
    pin_start
    cc_workflow .github/workflows/ci.yml "      - uses: acme/rolling@$A # mainline"
    cc_gh_answer repos/acme/rolling/git/ref/tags/mainline .object.sha 1 ""
    cc_pins
    cz_expect_rc 1
    cz_expect_stdout "$(fail_not_tag acme/rolling mainline acme/rolling)"
}
cc_case "only the exact words main and master mean a branch" case_branch_word_exact

case_subdirectory_action() {
    pin_start
    cc_workflow .github/workflows/ci.yml "      - uses: github/codeql-action/analyze@$A # v3"
    cc_tag github/codeql-action v3 "$A"
    cc_pins
    cz_expect_rc 0
    cz_expect_stdout "$(ok_line github/codeql-action/analyze "$A" v3)" "$ALL_OK"
    cz_expect_gh_log "$(sha_call github/codeql-action v3)" "$(type_call github/codeql-action v3)"
}
cc_case "an action in a subdirectory asks for the tags of its owner/repo and prints its full name" case_subdirectory_action

case_long_name_not_truncated() {
    pin_start
    local long="owner-with-a-long-name/repository-with-a-very-long-name/sub-action-path"
    cc_workflow .github/workflows/ci.yml "      - uses: $long@$A # v1"
    cc_tag owner-with-a-long-name/repository-with-a-very-long-name v1 "$A"
    cc_pins
    cz_expect_stdout "$(ok_line "$long" "$A" v1)" "$ALL_OK"
}
cc_case "an action name longer than the 46-character column is printed whole" case_long_name_not_truncated

case_duplicates_once() {
    pin_start
    cc_workflow .github/workflows/a.yml "      - uses: actions/checkout@$A # v4"
    cc_workflow .github/workflows/b.yml "  - uses: actions/checkout@$A # v4"
    cc_tag actions/checkout v4 "$A"
    cc_pins
    cz_expect_rc 0
    cz_expect_stdout "$(ok_line actions/checkout "$A" v4)" "$ALL_OK"
    cz_expect_gh_count "ref/tags/v4" 2
}
cc_case "the same pin in two files is verified once" case_duplicates_once

case_same_action_two_claims() {
    pin_start
    cc_workflow .github/workflows/a.yml "      - uses: actions/checkout@$A # v4" "      - uses: actions/checkout@$B # v3"
    cc_tag actions/checkout v4 "$A"; cc_tag actions/checkout v3 "$B"
    cc_pins
    cz_expect_rc 0
    # The sort key is the whole text, `action@sha # claim`, so the SHA decides.
    cz_expect_stdout "$(ok_line actions/checkout "$A" v4)" "$(ok_line actions/checkout "$B" v3)" "$ALL_OK"
}
cc_case "two pins of one action are two lines, sorted by their text, the SHA first" case_same_action_two_claims

case_sorted_bytewise() {
    pin_start
    cc_workflow .github/workflows/a.yml "      - uses: zeta/a@$A # v1" "      - uses: alpha/c@$B # v1" "      - uses: Alpha/b@$C # v1"
    cc_tag zeta/a v1 "$A"; cc_tag alpha/c v1 "$B"; cc_tag Alpha/b v1 "$C"
    cc_pins
    cz_expect_stdout "$(ok_line Alpha/b "$C" v1)" "$(ok_line alpha/c "$B" v1)" "$(ok_line zeta/a "$A" v1)" "$ALL_OK"
    cz_expect_gh_log "$(sha_call Alpha/b v1)" "$(type_call Alpha/b v1)" "$(sha_call alpha/c v1)" "$(type_call alpha/c v1)" "$(sha_call zeta/a v1)" "$(type_call zeta/a v1)"
}
cc_case "pins are verified in byte order of the pin text, uppercase before lowercase" case_sorted_bytewise

case_list_item_and_plain() {
    pin_start
    cc_workflow .github/workflows/a.yml "      - uses: acme/one@$A # v1" "        uses: acme/two@$B # v2" "      -   uses: acme/three@$C # v3"
    cc_tag acme/one v1 "$A"; cc_tag acme/two v2 "$B"; cc_tag acme/three v3 "$C"
    cc_pins
    cz_expect_rc 0
    cz_expect_stdout "$(ok_line acme/one "$A" v1)" "$(ok_line acme/three "$C" v3)" "$(ok_line acme/two "$B" v2)" "$ALL_OK"
}
cc_case "'- uses:', 'uses:' and '-   uses:' are all read" case_list_item_and_plain

case_comment_glued_to_sha() {
    pin_start
    cc_workflow .github/workflows/a.yml "      - uses: acme/one@$A# v1"
    cc_tag acme/one v1 "$A"
    cc_pins
    cz_expect_rc 1
    # Pins current behaviour: the pin passes the parse check, then the sha is cut at the first
    # space, so it keeps the '#'. A comment with no space before it can never match, however true.
    cz_expect_stdout "$(fail_mismatch acme/one "$A" v1 "$A")"
    cz_expect_stderr "$SOME_BAD"
}
cc_case "a comment glued to the SHA parses but never matches, even when the tag is right (pins current behaviour)" case_comment_glued_to_sha

case_comment_no_space_after_hash() {
    pin_start
    cc_workflow .github/workflows/a.yml "      - uses: acme/one@$A #v1"
    cc_tag acme/one v1 "$A"
    cc_pins
    cz_expect_rc 0
    cz_expect_stdout "$(ok_line acme/one "$A" v1)" "$ALL_OK"
}
cc_case "'#v1' with no space after the hash is a comment naming v1" case_comment_no_space_after_hash

case_trailing_space_and_crlf() {
    pin_start
    printf 'jobs:\r\n  a:\r\n    steps:\r\n      - uses: acme/one@%s # v1\r\n      - uses: acme/two@%s # v2   \n' "$A" "$B" > "$CZ_TREE/.github/workflows/a.yml"
    cc_tag acme/one v1 "$A"; cc_tag acme/two v2 "$B"
    cc_pins
    cz_expect_rc 0
    cz_expect_stdout "$(ok_line acme/one "$A" v1)" "$(ok_line acme/two "$B" v2)" "$ALL_OK"
}
cc_case "trailing spaces and carriage returns after the comment are ignored" case_trailing_space_and_crlf

# --- the line that is not a pin ---------------------------------------------------------------------

case_floating_tag() {
    pin_start
    cc_workflow .github/workflows/x.yml "jobs:" "  a:" "    steps:" "      - uses: actions/deploy-pages@v5"
    cc_pins
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr "$UNPARSED_HEAD" "            uses: actions/deploy-pages@v5" "$SOME_BAD"
    cz_expect_gh_log
}
cc_case "a floating tag is named on stderr, nothing is asked of GitHub, and the run fails" case_floating_tag

case_each_unparsed_shape() {
    pin_start
    cc_workflow .github/workflows/x.yml \
        "      - uses: actions/checkout@3d3c42e  # v7" \
        "      - uses: acme/nocomment@$A" \
        "      - uses: acme/upper@${A^^} # v1" \
        "      - uses: \"acme/quoted@$A\" # v1" \
        "      - uses: acme/twowords@$A # v1 extra" \
        "      - uses: ./.github/actions/local" \
        "      - uses: docker://alpine:3" \
        "      - uses: acme/empty@$A #"
    cc_tag acme/twowords v1 "$A"
    cc_pins
    cz_expect_rc 1
    # The two-word comment is reported as unparsed AND verified: the second extraction takes
    # the first word of the comment and does not need the line to end there. Pins current behaviour.
    cz_expect_stdout "$(ok_line acme/twowords "$A" v1)"
    cz_expect_gh_log "$(sha_call acme/twowords v1)" "$(type_call acme/twowords v1)"
    # sort -u of the text from 'uses:' on, in byte order.
    cz_expect_stderr "$UNPARSED_HEAD" \
        "            uses: \"acme/quoted@$A\" # v1" \
        "            uses: ./.github/actions/local" \
        "            uses: acme/empty@$A #" \
        "            uses: acme/nocomment@$A" \
        "            uses: acme/twowords@$A # v1 extra" \
        "            uses: acme/upper@${A^^} # v1" \
        "            uses: actions/checkout@3d3c42e  # v7" \
        "            uses: docker://alpine:3" \
        "$SOME_BAD"
}
cc_case "a short SHA, a missing comment, an upper-case SHA, a quoted ref, a two-word comment, a local action, a docker ref and an empty comment are all refused as unparsed" case_each_unparsed_shape

case_unparsed_and_good_pin() {
    pin_start
    cc_workflow .github/workflows/x.yml "      - uses: actions/deploy-pages@v5" "      - uses: actions/checkout@$A # v4"
    cc_tag actions/checkout v4 "$A"
    cc_pins
    cz_expect_rc 1
    cz_expect_stdout "$(ok_line actions/checkout "$A" v4)"
    cz_expect_stderr "$UNPARSED_HEAD" "            uses: actions/deploy-pages@v5" "$SOME_BAD"
    cz_expect_gh_log "$(sha_call actions/checkout v4)" "$(type_call actions/checkout v4)"
}
cc_case "an unparsed line fails the run while the good pins beside it are still verified, and the verdict is the failing one" case_unparsed_and_good_pin

case_unparsed_deduplicated() {
    pin_start
    cc_workflow .github/workflows/a.yml "      - uses: actions/deploy-pages@v5"
    cc_workflow .github/workflows/b.yml "  - uses: actions/deploy-pages@v5   "
    cc_pins
    cz_expect_rc 1
    cz_expect_stderr "$UNPARSED_HEAD" "            uses: actions/deploy-pages@v5" "$SOME_BAD"
}
cc_case "the same unparsed line in two files, one with trailing spaces, is named once" case_unparsed_deduplicated

case_commented_lines_ignored() {
    pin_start
    cc_workflow .github/workflows/x.yml "      # - uses: acme/old@v1" "# uses: acme/older@v1" "      - run: echo uses: acme/inline@v1"
    cc_pins
    cz_expect_rc 0
    cz_expect_stdout "$ALL_OK"
    cz_expect_gh_log
}
cc_case "a commented-out uses: and a uses: inside a run line are not scanned" case_commented_lines_ignored

case_no_uses() {
    pin_start
    cc_workflow .github/workflows/x.yml "jobs:" "  a:" "    steps:" "      - run: echo hi"
    cc_pins
    cz_expect_rc 0
    cz_expect_stdout "$ALL_OK"
    cz_expect_stderr_empty
    cz_expect_gh_log
}
cc_case "a workflow with no uses: at all passes and asks GitHub nothing" case_no_uses

# --- which files are scanned -----------------------------------------------------------------------

case_yaml_extension_and_depth() {
    pin_start
    cc_workflow .github/workflows/a.yaml "      - uses: acme/yaml@$A # v1"
    cc_workflow .github/workflows/b.yml.bak "      - uses: acme/bak@v1"
    cc_workflow .github/workflows/sub/c.yml "      - uses: acme/deep@v1"
    cc_workflow .github/workflows/notes.txt "      - uses: acme/txt@v1"
    cc_tag acme/yaml v1 "$A"
    cc_pins
    cz_expect_rc 0
    cz_expect_stdout "$(ok_line acme/yaml "$A" v1)" "$ALL_OK"
}
cc_case "workflows are the .yml and .yaml files directly under .github/workflows" case_yaml_extension_and_depth

case_composite_action_scanned() {
    pin_start
    cc_workflow .github/workflows/a.yml "      - run: echo hi"
    cc_workflow .github/actions/setup/action.yml "runs:" "  steps:" "    - uses: acme/inner@v1"
    cc_pins
    cz_expect_rc 1
    cz_expect_stderr "$UNPARSED_HEAD" "            uses: acme/inner@v1" "$SOME_BAD"
}
cc_case "a uses: inside a local composite action is scanned" case_composite_action_scanned

case_composite_action_pinned() {
    pin_start
    cc_workflow .github/workflows/a.yml "      - run: echo hi"
    cc_workflow .github/actions/a/b/action.yaml "    - uses: acme/inner@$A # v1"
    cc_workflow .github/actions/a/README.md "    - uses: acme/ignored@v1"
    cc_tag acme/inner v1 "$A"
    cc_pins
    cz_expect_rc 0
    cz_expect_stdout "$(ok_line acme/inner "$A" v1)" "$ALL_OK"
}
cc_case "action.yaml at any depth under .github/actions is scanned and other files there are not" case_composite_action_pinned

case_actions_dir_without_metadata() {
    pin_start
    cc_workflow .github/workflows/a.yml "      - run: echo hi"
    cc_workflow .github/actions/readme.md "text"
    cc_pins
    cz_expect_rc 2
    cz_expect_stdout_empty
    cz_expect_stderr "verify-action-pins: $CZ_TREE/.github/actions exists and holds no action.yml, so the composite scan read nothing"
}
cc_case "a .github/actions directory with no action.yml exits 2 saying the scan read nothing" case_actions_dir_without_metadata

case_examples_scanned() {
    pin_start
    cc_workflow .github/workflows/a.yml "      - run: echo hi"
    cc_workflow examples/one.yml "      - uses: acme/example@$A # v1"
    cc_workflow examples/two.yaml "      - uses: acme/yaml-example@v1"
    cc_workflow examples/sub/three.yml "      - uses: acme/deep-example@v1"
    cc_tag acme/example v1 "$A"
    cc_pins
    cz_expect_rc 0
    cz_expect_stdout "$(ok_line acme/example "$A" v1)" "$ALL_OK"
}
cc_case "examples/*.yml is scanned, one level deep and .yml only" case_examples_scanned

case_no_workflows_dir() {
    cc_pins_tree; cc_gh_pins
    rmdir "$CZ_TREE/.github/workflows"
    cc_pins
    cz_expect_rc 2
    cz_expect_stdout_empty
    cz_expect_stderr "verify-action-pins: no .github/workflows under $CZ_TREE"
}
cc_case "no .github/workflows exits 2 naming the root" case_no_workflows_dir

case_no_workflow_files() {
    pin_start
    cc_workflow .github/workflows/notes.txt "text"
    cc_pins
    cz_expect_rc 2
    cz_expect_stdout_empty
    cz_expect_stderr "verify-action-pins: no workflow files under $CZ_TREE/.github/workflows, so nothing was scanned"
}
cc_case "a workflows directory with no .yml or .yaml file exits 2 saying nothing was scanned" case_no_workflow_files

case_root_argument() {
    pin_start
    mkdir -p "$CZ_CASE/other/.github/workflows"
    printf '      - uses: acme/one@%s # v1\n' "$A" > "$CZ_CASE/other/.github/workflows/a.yml"
    cc_workflow .github/workflows/a.yml "      - uses: acme/own@v1"
    cc_tag acme/one v1 "$A"
    cc_pins "$CZ_CASE/other"
    cz_expect_rc 0
    cz_expect_stdout "$(ok_line acme/one "$A" v1)" "$ALL_OK"
}
cc_case "a root argument names the checkout to scan, in place of the script's own" case_root_argument

case_root_argument_echoed() {
    pin_start
    cc_pins ../tree/
    cz_expect_rc 2
    cz_expect_stderr "verify-action-pins: no workflow files under ../tree//.github/workflows, so nothing was scanned"
}
cc_case "the messages repeat the root exactly as given, relative path and trailing slash included" case_root_argument_echoed

case_root_argument_missing_dir() {
    pin_start
    cc_pins "$CZ_CASE/nowhere"
    cz_expect_rc 2
    cz_expect_stderr "verify-action-pins: no .github/workflows under $CZ_CASE/nowhere"
}
cc_case "a root that does not exist exits 2" case_root_argument_missing_dir

case_default_root_through_a_linked_prefix() {
    cc_pins_tree; cc_gh_pins
    rmdir "$CZ_TREE/.github/workflows"
    mkdir -p "$CZ_CASE/via"
    ln -s "$CZ_TREE" "$CZ_CASE/via/linked"
    cc_run "$CZ_CASE/via/linked/scripts/verify-action-pins.sh"
    cz_expect_rc 2
    cz_expect_stderr "verify-action-pins: no .github/workflows under $CZ_CASE/via/linked"
}
cc_case "with no argument the root keeps the name of a link in the program's own path (pins current behaviour)" case_default_root_through_a_linked_prefix

case_binary_workflow_contributes_nothing() {
    pin_start
    cc_workflow .github/workflows/ok.yml "      - uses: acme/one@$A # v1"
    printf '\0      - uses: acme/two@%s # v2\n      - uses: acme/floating@v1\n' "$B" > "$CZ_TREE/.github/workflows/binary.yml"
    cc_tag acme/one v1 "$A"
    cc_pins
    cz_expect_rc 0
    # grep takes a file with a NUL byte for binary and prints none of its lines (the script silences the
    # message GNU grep writes), so neither its pin nor its floating tag is seen.
    cz_expect_stdout "$(ok_line acme/one "$A" v1)" "$ALL_OK"
    cz_expect_stderr_empty
}
cc_case "a workflow with a NUL byte is binary to grep and contributes no line, pin or unparsed (pins current behaviour)" case_binary_workflow_contributes_nothing

case_scan_writes_nothing() {
    pin_start
    cc_workflow .github/workflows/ci.yml "      - uses: actions/checkout@$A # v4"
    cc_tag actions/checkout v4 "$A"
    cc_snap "$CZ_TREE" > "$CZ_CASE/before"
    cc_pins
    cc_snap "$CZ_TREE" > "$CZ_CASE/after"
    cz_expect_rc 0
    cc_expect_unchanged "the tree" "$CZ_CASE/before" "$CZ_CASE/after"
    cz_check "files in TMPDIR" "$(cc_ls "$CZ_TMP" | wc -l | tr -d ' ')" 0
    cz_check "files in cwd" "$(cc_ls "$CZ_CWD" | wc -l | tr -d ' ')" 0
}
cc_case "the scan writes no file, in the tree, in TMPDIR or in the working directory" case_scan_writes_nothing

case_gh_argv_never_shell() {
    pin_start
    cc_workflow .github/workflows/ci.yml "      - uses: acme/odd@$A # v1.0+x;y"
    cc_gh_answer "repos/acme/odd/git/ref/tags/v1.0+x;y" .object.sha 1 ""
    cc_pins
    cz_expect_rc 1
    cz_expect_gh_log "$(cz_argv api 'repos/acme/odd/git/ref/tags/v1.0+x;y' --jq .object.sha)"
}
cc_case "a tag with shell characters travels as one argument" case_gh_argv_never_shell

cz_finish
