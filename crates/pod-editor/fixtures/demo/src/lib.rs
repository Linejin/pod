//! Demo library used by the pod-editor fixture repository.

pub mod geometry;

/// Builds a greeting for `name`.
pub fn greet(name: &str) -> String {
    format!("안녕하세요, {name}!")
}

/// Counts words separated by whitespace.
pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub title: String,
    pub done: bool,
}

impl Task {
    pub fn new(title: impl Into<String>) -> Self {
        Task { title: title.into(), done: false }
    }

    pub fn finish(&mut self) {
        self.done = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greets() {
        assert_eq!(greet("Pod"), "안녕하세요, Pod!");
        assert_eq!(word_count("a b  c"), 3);
    }
}
