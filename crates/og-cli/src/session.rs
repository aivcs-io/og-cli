//! Laptop session control — local, mesh-free, any machine.
//!
//! `login` / `exit` / `help` (with or without a leading `/`) are REPL verbs.
//! They must never become fleet objectives and must never require
//! `DATA_MESH_*`. Unknown English still forwards to og-leader.

/// A command that stays on this process. No A2A, no data-mesh, no confirmation
/// as a PR/repo mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionCommand {
    Exit,
    Help,
    Login,
    Logout,
    Whoami,
    Compact,
    Config,
}

impl SessionCommand {
    pub fn label(self) -> &'static str {
        match self {
            Self::Exit => "exit",
            Self::Help => "help",
            Self::Login => "login",
            Self::Logout => "logout",
            Self::Whoami => "whoami",
            Self::Compact => "compact",
            Self::Config => "config",
        }
    }
}

/// True when the whole line is a session verb (optional leading `/`, optional
/// trailing `.`/`!`/`?`, case-insensitive). Extra words mean it is not a
/// session verb (`login and review #1` still goes to intent).
pub fn parse_session_command(text: &str) -> Option<SessionCommand> {
    let mut s = text.trim();
    if s.is_empty() {
        return None;
    }
    if s == "?" || s.eq_ignore_ascii_case("/?") {
        return Some(SessionCommand::Help);
    }
    if let Some(rest) = s.strip_prefix('/') {
        s = rest.trim();
    }
    let s = s.trim_end_matches(|c: char| matches!(c, '.' | '!' | '?'));
    let key = s.to_ascii_lowercase();
    let key = key.trim();
    match key {
        "exit" | "quit" | "q" | "bye" => Some(SessionCommand::Exit),
        "help" | "h" | "?" => Some(SessionCommand::Help),
        "login" | "log in" | "signin" | "sign in" => Some(SessionCommand::Login),
        "logout" | "log out" | "signout" | "sign out" => Some(SessionCommand::Logout),
        "whoami" | "who am i" => Some(SessionCommand::Whoami),
        "compact" => Some(SessionCommand::Compact),
        "config" | "settings" => Some(SessionCommand::Config),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_optional_and_case_insensitive() {
        for text in ["exit", "EXIT", "/exit", "Exit!", "quit", "q", "bye"] {
            assert_eq!(
                parse_session_command(text),
                Some(SessionCommand::Exit),
                "{text}"
            );
        }
        for text in ["login", "/login", "Log in", "sign in"] {
            assert_eq!(
                parse_session_command(text),
                Some(SessionCommand::Login),
                "{text}"
            );
        }
        assert_eq!(parse_session_command("help"), Some(SessionCommand::Help));
        assert_eq!(parse_session_command("/whoami"), Some(SessionCommand::Whoami));
    }

    #[test]
    fn extra_words_are_not_session_verbs() {
        assert_eq!(parse_session_command("login and review #1"), None);
        assert_eq!(parse_session_command("exit the cluster"), None);
        assert_eq!(parse_session_command("make me a sandwich"), None);
    }
}
