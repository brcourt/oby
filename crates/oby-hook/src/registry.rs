use crate::capturers::{BashCapturer, ReadCapturer};
use oby_core::Capturer;

#[allow(dead_code)]
pub fn builtin_capturers() -> Vec<Box<dyn Capturer>> {
    vec![
        Box::new(BashCapturer),
        Box::new(ReadCapturer),
        // Add new capturers here in future plans.
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_bash_and_read() {
        let caps = builtin_capturers();
        let names: Vec<&str> = caps.iter().map(|c| c.name()).collect();
        assert!(names.contains(&"bash"));
        assert!(names.contains(&"read"));
    }

    #[test]
    fn lookup_by_tool_name() {
        let caps = builtin_capturers();
        let bash = caps.iter().find(|c| c.tool_name() == "Bash");
        assert!(bash.is_some());
        assert_eq!(bash.unwrap().name(), "bash");
    }
}
