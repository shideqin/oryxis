//! Recognizes a password prompt sitting in front of the cursor (issue
//! #117), so the host can offer the credential the vault already holds.
//!
//! Unlike [`crate::osc`] this is not a byte sniffer. The question it
//! answers is "what is printed before the cursor right now", and the
//! grid already holds that answer with the ANSI stripped, the carriage
//! returns applied and the soft wraps resolved. That also makes the
//! read free of state: a prompt split across two PTY chunks, redrawn by
//! a `\r`, or painted in color is one string here either way.
//!
//! The reader lives in [`crate::backend`]; this module is the pure half
//! (the matching rules), which is where all the judgement calls are.
//!
//! **English only, on purpose.** `sudo` is translated, so a localized
//! prompt is missed. Matching translations would mean translating the
//! NEGATIVE list too, and a half-translated exclusion list is worse
//! than no translation at all: it would offer the login password at a
//! `passwd` prompt in that language.

/// A password prompt found in front of the cursor, with the grid
/// position that identifies it.
///
/// The position is half the identity because the text alone repeats:
/// `sudo` printing `[sudo] password for x:` twice (wrong password) is
/// two prompts, and the second one deserves the popup again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasswordPrompt {
    /// The prompt as printed, trailing whitespace trimmed.
    pub text: String,
    /// Absolute row of the logical line's FIRST physical row
    /// (`history_size + visible line`), the coordinate space
    /// [`crate::osc::PositionedShellMark`] uses.
    pub abs_line: i64,
}

/// How much of the line's tail is considered. Long enough for the
/// longest real prompt (ssh's `Enter passphrase for key '<path>':` with
/// a deep home directory), short enough that a wrapped paragraph of
/// prose ending in a colon cannot reach a `password` far behind it.
const TAIL_CHARS: usize = 160;

/// Tokens that make a line a password prompt.
const KEYWORDS: [&str; 2] = ["password", "passphrase"];

/// Tokens that take it back. Every one of these marks a prompt asking
/// for a password the user is about to SET or CONFIRM (`passwd`,
/// `ssh-keygen`), never one the vault could answer. Offering a stored
/// credential there would help the user overwrite their own password
/// with it.
///
/// Matched as whole tokens, never as substrings: `[sudo] password for
/// newton:` must not be excluded by `new`.
const NEGATIVE: [&str; 10] = [
    "new", "retype", "re-enter", "reenter", "confirm", "verify", "current", "old", "again",
    "repeat",
];

/// `ssh-keygen` creating a key. It carries no negative token of its own
/// ("empty" is too generic to blacklist on its own), and the prompt it
/// pairs with (`Enter same passphrase again:`) is already excluded, so
/// without this the first half of a key generation would offer a
/// credential and the second half would not.
const NEGATIVE_PHRASES: [&str; 1] = ["empty for no passphrase"];

/// Whether `line` (the text printed before the cursor) is a prompt
/// waiting for a password.
pub fn looks_like_password_prompt(line: &str) -> bool {
    let trimmed = line.trim_end();
    // The colon is what separates a prompt from prose that merely
    // mentions a password. Every prompt we mean to catch prints one.
    if !trimmed.ends_with(':') {
        return false;
    }
    let tail: String = {
        let count = trimmed.chars().count();
        trimmed
            .chars()
            .skip(count.saturating_sub(TAIL_CHARS))
            .collect::<String>()
            .to_lowercase()
    };
    if NEGATIVE_PHRASES.iter().any(|p| tail.contains(p)) {
        return false;
    }
    let tokens: Vec<&str> = tail
        .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.iter().any(|t| NEGATIVE.contains(t)) {
        return false;
    }
    tokens.iter().any(|t| KEYWORDS.contains(t))
        // OpenSSL and `ssh-keygen -p` spell it as two words
        // ("Enter PEM pass phrase:"), which tokenizes apart.
        || tokens.windows(2).any(|w| w == ["pass", "phrase"])
}

/// The prompts programs actually print, for the alternate screen.
///
/// Case-sensitive and anchored at both ends on purpose. The alternate
/// screen belongs to whoever drew it, and besides a multiplexer that is
/// an editor: vim in insert mode, cursor after a YAML `password:` key,
/// passes the loose rules. What tells the two apart is the shape of the
/// whole segment: su says `Password:`, never an indented lower-case key.
///
/// Sources, one per alternative: sudo, sudo-rs (Ubuntu's default since
/// 25.10), doas, su / login / PAM modules that name themselves (`LDAP
/// Password:`), MySQL, ssh (password and keyboard-interactive), git's
/// credential prompt and psql's, ssh and ssh-add passphrases, OpenSSL.
static MULTIPLEXED_SHAPES: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"^(?:\[sudo\] password for \S+:|\[sudo: authenticate\] Password:|doas \(\S+\) password:|(?:[A-Z][A-Za-z]* )?Password:|Enter password:|\S+'s password:|\(\S+\) Password:|Password for (?:user )?\S+:|Enter passphrase for (?:key )?\S+:|Enter PEM pass phrase:)$",
    )
    .expect("static pattern")
});

/// [`looks_like_password_prompt`] for a segment read off the ALTERNATE
/// screen, where a prompt only counts when it is the whole segment and
/// one of [`MULTIPLEXED_SHAPES`] (issue #232). The loose rules still run
/// after the shape, so their exclusions hold here too: `New Password:`
/// fits the shape and is still refused.
///
/// Leading whitespace fails the anchor, and that is deliberate: a
/// multiplexer's pane starts right after its divider, while an editor's
/// line numbers and indentation do not.
pub fn looks_like_multiplexed_password_prompt(segment: &str) -> bool {
    let trimmed = segment.trim_end();
    MULTIPLEXED_SHAPES.is_match(trimmed) && looks_like_password_prompt(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_prompts_match() {
        for line in [
            "[sudo] password for wilson: ",
            "[sudo] password for wilson:",
            "Password:",
            "Password: ",
            "wilson@10.0.0.5's password: ",
            "wilson@10.0.0.5's password:",
            "Enter passphrase for key '/home/wilson/.ssh/id_ed25519': ",
            "Enter password: ",
            "Password for 'https://wilson@github.com': ",
            "LDAP Password: ",
            "(wilson@host) Password: ",
            "Enter PEM pass phrase:",
            "root's password:",
        ] {
            assert!(looks_like_password_prompt(line), "should match: {line:?}");
        }
    }

    #[test]
    fn set_and_confirm_prompts_never_match() {
        // Every line here asks the user to CHOOSE a password. Offering
        // the stored one would help them overwrite it with itself.
        for line in [
            "New password: ",
            "Retype new password: ",
            "Re-enter new password: ",
            "Confirm password: ",
            "Confirm new password:",
            "Verify password:",
            "Current password: ",
            "Current Kerberos password:",
            "Old password:",
            "Enter new UNIX password: ",
            "Retype password:",
            "Password again:",
            "Repeat password:",
            "Enter passphrase (empty for no passphrase): ",
            "Enter same passphrase again: ",
        ] {
            assert!(!looks_like_password_prompt(line), "should not match: {line:?}");
        }
    }

    #[test]
    fn a_username_containing_an_excluded_word_still_matches() {
        // The reason NEGATIVE is matched per token: `new` inside
        // `newton` (or `oldrich`, or `verity`) is not the word.
        for line in [
            "[sudo] password for newton: ",
            "[sudo] password for oldrich: ",
            "[sudo] password for verity: ",
            "newton@host's password: ",
        ] {
            assert!(looks_like_password_prompt(line), "should match: {line:?}");
        }
    }

    #[test]
    fn ordinary_output_does_not_match() {
        for line in [
            "",
            "   ",
            "wilson@host:~$ ",
            "wilson@host:~$ sudo -k",
            // Mentions the word but is not asking for one.
            "Reading package lists...",
            "password",
            "The password was changed.",
            // Prompts for something else entirely.
            "Verification code: ",
            "Enter PIN: ",
            "Username: ",
            "Are you sure you want to continue connecting (yes/no)? ",
        ] {
            assert!(!looks_like_password_prompt(line), "should not match: {line:?}");
        }
    }

    #[test]
    fn a_keyword_beyond_the_tail_window_does_not_carry_a_far_away_colon() {
        // A wrapped paragraph that happens to mention a password and
        // end in a colon is not a prompt. The window is what keeps the
        // two from meeting.
        let long = format!("the password policy for this host is {}:", "x".repeat(300));
        assert!(!looks_like_password_prompt(&long));
        // Same sentence short enough to fit the window still matches:
        // it is indistinguishable from a prompt by text alone, and the
        // popup is a suggestion that never sends itself.
        assert!(looks_like_password_prompt("the password policy is:"));
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(looks_like_password_prompt("PASSWORD:"));
        assert!(looks_like_password_prompt("[SUDO] PASSWORD FOR WILSON:"));
        assert!(!looks_like_password_prompt("NEW PASSWORD:"));
    }

    #[test]
    fn multiplexed_prompts_match_by_shape() {
        for line in [
            "[sudo] password for wilson: ",
            "[sudo: authenticate] Password: ",
            "doas (wilson@host) password: ",
            "Password: ",
            "LDAP Password:",
            "Enter password: ",
            "wilson@10.0.0.5's password: ",
            "(wilson@host) Password: ",
            "Password for 'https://wilson@github.com': ",
            "Password for user postgres: ",
            "Enter passphrase for key '/home/wilson/.ssh/id_ed25519': ",
            "Enter passphrase for /home/wilson/.ssh/id_ed25519: ",
            "Enter PEM pass phrase:",
        ] {
            assert!(looks_like_multiplexed_password_prompt(line), "should match: {line:?}");
        }
    }

    #[test]
    fn multiplexed_segments_that_only_look_like_prompts_do_not_match() {
        for line in [
            // vim editing YAML, cursor after the key.
            "password:",
            "  password:",
            "    Password:",
            // vim with line numbers.
            "  3 Password:",
            // less / vim searching.
            "/Password:",
            // Prose that the loose rules accept.
            "the password policy is:",
            "Please enter your password:",
            // The shape fits, the exclusions still refuse.
            "New Password:",
            "Current Password:",
            "Retype new password:",
        ] {
            assert!(!looks_like_multiplexed_password_prompt(line), "should not match: {line:?}");
        }
    }
}
