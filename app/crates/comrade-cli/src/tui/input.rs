/// Text input state with cursor and command history.
pub struct InputState {
    /// Current text buffer.
    text: Vec<char>,
    /// Cursor position (byte offset into `text`).
    cursor: usize,
    /// Command history (most recent last).
    history: Vec<String>,
    /// Current position in history (None = composing new text).
    history_idx: Option<usize>,
    /// Saved text when navigating history.
    saved_text: String,
}

impl InputState {
    pub fn new() -> Self {
        Self {
            text: Vec::new(),
            cursor: 0,
            history: Vec::new(),
            history_idx: None,
            saved_text: String::new(),
        }
    }

    pub fn text(&self) -> String {
        self.text.iter().collect()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn insert(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.text.remove(self.cursor);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.text.len() {
            self.cursor += 1;
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.text.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Take the current text, push it to history, and reset.
    pub fn submit(&mut self) -> String {
        let text = self.text();
        if !text.is_empty() {
            self.history.push(text.clone());
        }
        self.text.clear();
        self.cursor = 0;
        self.history_idx = None;
        self.saved_text.clear();
        text
    }

    pub fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_idx {
            None => {
                // Save current text, move to last history entry.
                self.saved_text = self.text();
                let idx = self.history.len() - 1;
                self.history_idx = Some(idx);
                self.set_text(&self.history[idx].clone());
            }
            Some(idx) if idx > 0 => {
                let new_idx = idx - 1;
                self.history_idx = Some(new_idx);
                self.set_text(&self.history[new_idx].clone());
            }
            _ => {}
        }
    }

    pub fn history_down(&mut self) {
        match self.history_idx {
            Some(idx) if idx + 1 < self.history.len() => {
                let new_idx = idx + 1;
                self.history_idx = Some(new_idx);
                self.set_text(&self.history[new_idx].clone());
            }
            Some(_) => {
                // Back to current composition.
                self.history_idx = None;
                let saved = self.saved_text.clone();
                self.set_text(&saved);
                self.saved_text.clear();
            }
            None => {}
        }
    }

    fn set_text(&mut self, s: &str) {
        self.text = s.chars().collect();
        self.cursor = self.text.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_submit() {
        let mut input = InputState::new();
        input.insert('h');
        input.insert('i');
        assert_eq!(input.text(), "hi");
        assert_eq!(input.cursor(), 2);

        let submitted = input.submit();
        assert_eq!(submitted, "hi");
        assert_eq!(input.text(), "");
    }

    #[test]
    fn backspace_and_delete() {
        let mut input = InputState::new();
        for ch in "hello".chars() {
            input.insert(ch);
        }
        input.backspace();
        assert_eq!(input.text(), "hell");

        input.move_left();
        input.delete();
        assert_eq!(input.text(), "hel");
    }

    #[test]
    fn cursor_movement() {
        let mut input = InputState::new();
        for ch in "abc".chars() {
            input.insert(ch);
        }
        input.home();
        assert_eq!(input.cursor(), 0);
        input.end();
        assert_eq!(input.cursor(), 3);

        input.move_left();
        assert_eq!(input.cursor(), 2);
        input.move_right();
        assert_eq!(input.cursor(), 3);
        // Don't go past end.
        input.move_right();
        assert_eq!(input.cursor(), 3);
    }

    #[test]
    fn history_navigation() {
        let mut input = InputState::new();
        input.insert('a');
        input.submit();
        input.insert('b');
        input.submit();

        // Navigate up through history.
        input.insert('c');
        input.history_up();
        assert_eq!(input.text(), "b");
        input.history_up();
        assert_eq!(input.text(), "a");
        // Already at top, stays.
        input.history_up();
        assert_eq!(input.text(), "a");

        // Navigate back down.
        input.history_down();
        assert_eq!(input.text(), "b");
        input.history_down();
        assert_eq!(input.text(), "c"); // restored saved text
    }

    #[test]
    fn empty_submit_no_history() {
        let mut input = InputState::new();
        let submitted = input.submit();
        assert_eq!(submitted, "");
        // Empty submit should not add to history.
        input.history_up();
        assert_eq!(input.text(), "");
    }

    #[test]
    fn clear_input() {
        let mut input = InputState::new();
        for ch in "hello".chars() {
            input.insert(ch);
        }
        input.clear();
        assert_eq!(input.text(), "");
        assert_eq!(input.cursor(), 0);
    }

    #[test]
    fn insert_in_middle() {
        let mut input = InputState::new();
        for ch in "hllo".chars() {
            input.insert(ch);
        }
        input.home();
        input.move_right();
        input.insert('e');
        assert_eq!(input.text(), "hello");
        assert_eq!(input.cursor(), 2);
    }
}
