//! `humanise` of `lib/run.sh`: what broke, turned into what to do about it.
//!
//! The raw alert is a shell message: it names what broke in the vocabulary of
//! the thing that broke, gives no next step, and spends two thirds of its width
//! on an absolute path. The desktop toast says the next step instead. A message
//! that no pattern matches falls through verbatim, because a wrong translation is
//! worse than an untranslated string.
//!
//! The bash matches with `case` and glob patterns, first match wins. The order of
//! [`humanise`] is that order.

/// `*"a"*b*`: the text holds `a` and, somewhere after it, `b`.
fn contains_in_order(text: &str, first: &str, second: &str) -> bool {
    text.find(first).is_some_and(|at| text[at + first.len()..].contains(second))
}

/// Turns a raw alert into `sentence` or `sentence\nfix  command`, with no trailing
/// newline. `lock_wait` is the profile's `LOCK_WAIT` and `account` its
/// `GH_ACCOUNT`.
#[must_use]
pub fn humanise(raw: &str, lock_wait: &str, account: &str) -> String {
    if contains_in_order(raw, "unusable: missing", "settings")
        || contains_in_order(raw, "unusable: missing", "opencode")
    {
        "Claude Code has no settings file for this profile.\nfix  ./install.sh".to_string()
    } else if raw.contains("not on PATH") {
        format!("{raw}\nfix  install it, then: maintainer-doctor")
    } else if raw.contains("refresh failed") {
        "The checkout could not be refreshed, so no agent ran.\nfix  maintainer-doctor".to_string()
    } else if raw.contains("held the lock") {
        format!(
            "Another run held the shared tree for over {lock_wait}s.\nfix  maintainer status, then wait or kill it"
        )
    } else if raw.contains("authenticated as") {
        format!("{raw}\nfix  gh auth switch --user {account}")
    } else if raw.contains("produced no report") {
        "The run ended without writing a report, so it is not auditable.\nfix  read the log below"
            .to_string()
    } else if raw.contains("OAuth session expired")
        || raw.contains("Failed to authenticate")
        || raw.contains("failed to authenticate")
    {
        "The backend login has expired, so every slot fails until you renew it.\nfix  claude /login"
            .to_string()
    } else if raw.contains("credit balance is too low") {
        "The backend account is out of credit. No retry pays an invoice.\nfix  top up the account, then: maintainer-doctor"
            .to_string()
    } else if raw.contains("only you can clear") {
        format!("{raw}\nfix  claude /login, then: maintainer-doctor")
    } else if raw.contains("cannot enforce POST=off") {
        format!("{raw}\nfix  set BACKEND=claude or BACKEND=opencode in profile.env")
    } else if raw.contains("no rehearsal wall") {
        "POST=off, but no rehearsal deny wall is deployed for this profile.\nfix  ./install.sh"
            .to_string()
    } else {
        raw.to_string()
    }
}

/// The body of a toast: the lines joined with newlines, the way `printf '%s\n'`
/// inside `$(...)` joins them. The substitution drops every trailing newline, so
/// a last line that is empty disappears with the one after it.
#[must_use]
pub fn toast_body(lines: &[&str]) -> String {
    lines.join("\n").trim_end_matches('\n').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(raw: &str) -> String {
        humanise(raw, "3600", "fxacct")
    }

    #[test]
    fn a_missing_settings_file_is_not_shown_with_its_path() {
        assert_eq!(
            h("backend claude unusable: missing /home/u/some/long/path/settings.json"),
            "Claude Code has no settings file for this profile.\nfix  ./install.sh"
        );
        assert_eq!(
            h("backend opencode unusable: missing /p/opencode.json"),
            "Claude Code has no settings file for this profile.\nfix  ./install.sh"
        );
    }

    #[test]
    fn missing_something_else_is_not_the_settings_message() {
        let raw = "backend cursor unusable: missing /p/cli-config.json";
        assert_eq!(h(raw), raw);
    }

    #[test]
    fn the_order_inside_the_missing_pattern_matters() {
        let raw = "settings and opencode, then backend x unusable: missing something";
        assert_eq!(h(raw), raw);
    }

    #[test]
    fn a_tool_not_on_path_repeats_the_message_and_adds_the_install_fix() {
        assert_eq!(
            h("backend claude unusable: claude not on PATH"),
            "backend claude unusable: claude not on PATH\nfix  install it, then: maintainer-doctor"
        );
    }

    #[test]
    fn a_failed_refresh_says_no_agent_ran() {
        assert_eq!(
            h("refresh failed, no agent run started. See /l"),
            "The checkout could not be refreshed, so no agent ran.\nfix  maintainer-doctor"
        );
    }

    #[test]
    fn a_held_lock_names_the_wait_from_the_profile() {
        assert_eq!(
            humanise("another run held the lock for over 5s", "5", "x"),
            "Another run held the shared tree for over 5s.\nfix  maintainer status, then wait or kill it"
        );
    }

    #[test]
    fn a_wrong_account_names_the_switch_command_for_the_profile_account() {
        assert_eq!(
            h("gh is authenticated as 'o', not fxacct; refusing to run. See /l"),
            "gh is authenticated as 'o', not fxacct; refusing to run. See /l\nfix  gh auth switch --user fxacct"
        );
    }

    #[test]
    fn a_missing_report_says_the_run_is_not_auditable() {
        assert_eq!(
            h("2026-01-01T00-00-review produced no report; the run is not auditable. See /l"),
            "The run ended without writing a report, so it is not auditable.\nfix  read the log below"
        );
    }

    #[test]
    fn the_three_login_phrasings_share_one_fix() {
        let want = "The backend login has expired, so every slot fails until you renew it.\nfix  claude /login";
        assert_eq!(h("OAuth session expired. Please log in"), want);
        assert_eq!(h("Failed to authenticate. API Error: 401"), want);
        assert_eq!(h("failed to authenticate"), want);
    }

    #[test]
    fn an_empty_credit_balance_says_no_retry_pays_an_invoice() {
        assert_eq!(
            h("Your credit balance is too low to access the API"),
            "The backend account is out of credit. No retry pays an invoice.\nfix  top up the account, then: maintainer-doctor"
        );
    }

    #[test]
    fn a_cause_only_the_owner_can_clear_gets_the_login_fix_with_the_whole_message() {
        assert_eq!(
            h("the claude backend stopped on something only you can clear (Invalid API key)"),
            "the claude backend stopped on something only you can clear (Invalid API key)\nfix  claude /login, then: maintainer-doctor"
        );
    }

    #[test]
    fn a_backend_that_cannot_rehearse_points_at_the_profile() {
        assert_eq!(
            h(
                "backend 'codex' cannot enforce POST=off (no per-command deny list); use claude or opencode"
            ),
            "backend 'codex' cannot enforce POST=off (no per-command deny list); use claude or opencode\nfix  set BACKEND=claude or BACKEND=opencode in profile.env"
        );
    }

    #[test]
    fn a_missing_rehearsal_wall_points_at_install() {
        assert_eq!(
            h("POST=off but backend 'claude' has no rehearsal wall rendered for 'fx'"),
            "POST=off, but no rehearsal deny wall is deployed for this profile.\nfix  ./install.sh"
        );
    }

    #[test]
    fn a_message_no_pattern_matches_passes_through_verbatim() {
        assert_eq!(h("something unknown broke"), "something unknown broke");
        assert_eq!(h(""), "");
    }

    #[test]
    fn the_earlier_pattern_wins_when_two_match() {
        // "not on PATH" comes before "authenticated as".
        assert_eq!(
            h("claude not on PATH, authenticated as x"),
            "claude not on PATH, authenticated as x\nfix  install it, then: maintainer-doctor"
        );
    }

    #[test]
    fn a_toast_body_joins_lines_without_a_trailing_newline() {
        assert_eq!(toast_body(&["a", "b"]), "a\nb");
        assert_eq!(toast_body(&["only"]), "only");
        assert_eq!(toast_body(&["a", ""]), "a");
        assert_eq!(toast_body(&["a", "", "b"]), "a\n\nb");
    }
}
