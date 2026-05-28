/// Returns true if OBS_ACTIVE is set to a truthy value.
/// Plain `claude` (no wrapper) → OBS_ACTIVE unset → hook should no-op.
#[allow(dead_code)]
pub fn is_active() -> bool {
    matches!(std::env::var("OBS_ACTIVE").as_deref(), Ok("1") | Ok("true"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests mutate process env; in CI they run sequentially.
    // For v0.1, simple sequential ordering is fine.

    #[test]
    fn unset_is_inactive() {
        std::env::remove_var("OBS_ACTIVE");
        assert!(!is_active());
    }

    #[test]
    fn one_is_active() {
        std::env::set_var("OBS_ACTIVE", "1");
        assert!(is_active());
        std::env::remove_var("OBS_ACTIVE");
    }

    #[test]
    fn other_value_is_inactive() {
        std::env::set_var("OBS_ACTIVE", "no");
        assert!(!is_active());
        std::env::remove_var("OBS_ACTIVE");
    }
}
