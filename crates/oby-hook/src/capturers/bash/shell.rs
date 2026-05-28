//! Detect which shell CC's Bash tool will execute the command in.

#[allow(dead_code)]
pub enum Shell {
    Bash,
    Zsh,
    Other,
}

#[allow(dead_code)]
pub fn detect() -> Shell {
    if std::env::var("BASH_VERSION").is_ok() {
        return Shell::Bash;
    }
    if std::env::var("ZSH_VERSION").is_ok() {
        return Shell::Zsh;
    }
    match std::env::var("SHELL").ok().as_deref() {
        Some(s) if s.ends_with("/bash") => Shell::Bash,
        Some(s) if s.ends_with("/zsh") => Shell::Zsh,
        _ => Shell::Other,
    }
}

impl Shell {
    #[allow(dead_code)]
    pub fn supports_process_substitution(&self) -> bool {
        matches!(self, Shell::Bash | Shell::Zsh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_supports_process_sub() {
        assert!(Shell::Bash.supports_process_substitution());
    }

    #[test]
    fn zsh_supports_process_sub() {
        assert!(Shell::Zsh.supports_process_substitution());
    }

    #[test]
    fn other_does_not() {
        assert!(!Shell::Other.supports_process_substitution());
    }
}
