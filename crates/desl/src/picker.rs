//! A fuzzy picker in the style of fzf: a query line and a filtered list. Used for threads,
//! and later for the command palette and the file picker.

pub struct Entry {
    pub key: String,
    pub label: String,
    pub hint: String,
}

pub struct Picker {
    pub title: String,
    /// One muted line under the list: what choosing does.
    pub note: Option<String>,
    pub entries: Vec<Entry>,
    pub query: String,
    pub selected: usize,
}

/// fzf-style subsequence match. Returns a score (higher is better) or None.
pub fn fuzzy(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut qi = 0;
    let mut score = 0;
    let mut last_match: Option<usize> = None;
    for (ti, c) in t.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            score += 10;
            if let Some(l) = last_match
                && l + 1 == ti
            {
                score += 5; // consecutive
            }
            if ti == 0 || !t[ti - 1].is_alphanumeric() {
                score += 8; // word start
            }
            last_match = Some(ti);
            qi += 1;
        }
    }
    (qi == q.len()).then_some(score - t.len() as i32 / 4)
}

impl Picker {
    pub fn new(title: impl Into<String>, entries: Vec<Entry>) -> Self {
        Picker { title: title.into(), note: None, entries, query: String::new(), selected: 0 }
    }

    /// Starts on row `i` instead of the first: the current choice, or the likely one.
    pub fn starting_at(mut self, i: usize) -> Self {
        self.selected = i;
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Entries that match the query, best first. What is typed is matched against the label
    /// and the hint, but the label counts for much more: typing a command's name finds that
    /// command, not another one whose description happens to mention the word.
    pub fn matches(&self) -> Vec<&Entry> {
        let query = self.query.trim().trim_start_matches('/').to_lowercase();
        let mut scored: Vec<(i32, &Entry)> = self
            .entries
            .iter()
            .filter_map(|e| {
                let score = fuzzy(&query, &format!("{} {}", e.label, e.hint))?;
                let label = e.label.trim_start_matches('/').to_lowercase();
                let bonus = if query.is_empty() {
                    0
                } else if label.starts_with(&query) {
                    300
                } else if label.contains(&query) {
                    200
                } else if fuzzy(&query, &label).is_some() {
                    100
                } else {
                    0
                };
                Some((score + bonus, e))
            })
            .collect();
        scored.sort_by_key(|s| std::cmp::Reverse(s.0));
        scored.into_iter().map(|(_, e)| e).collect()
    }

    pub fn current(&self) -> Option<&Entry> {
        let m = self.matches();
        m.get(self.selected.min(m.len().saturating_sub(1))).copied()
    }

    pub fn type_char(&mut self, c: char) {
        self.query.push(c);
        self.selected = 0;
    }
    pub fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }
    pub fn down(&mut self) {
        let n = self.matches().len();
        if n > 0 {
            self.selected = (self.selected + 1).min(n - 1);
        }
    }
    pub fn up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::COMMANDS;

    fn palette() -> Picker {
        let entries = COMMANDS
            .iter()
            .map(|c| Entry { key: c.name.to_string(), label: format!("{} {}", c.name, c.args), hint: c.desc.to_string() })
            .collect();
        Picker::new("commands", entries)
    }

    fn first(query: &str) -> String {
        let mut p = palette();
        for c in query.chars() {
            p.type_char(c);
        }
        p.current().map(|e| e.key.clone()).unwrap_or_default()
    }

    #[test]
    fn typing_a_commands_name_finds_that_command() {
        for c in COMMANDS {
            let name = c.name.trim_start_matches('/');
            assert_eq!(first(name), c.name, "typing {name}");
            assert_eq!(first(c.name), c.name, "typing {}", c.name);
        }
    }

    #[test]
    fn a_few_letters_are_enough_and_descriptions_still_match() {
        assert_eq!(first("sw"), "/switch");
        assert_eq!(first("mod"), "/model");
        assert_eq!(first("fo"), "/fork");
        // No command is named "markdown"; the one that writes it is found by its description.
        assert_eq!(first("markdown"), "/export");
    }

    #[test]
    fn an_empty_query_keeps_the_given_order_and_the_starting_row() {
        let p = palette().starting_at(3);
        let keys: Vec<&str> = p.matches().iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, COMMANDS.iter().map(|c| c.name).collect::<Vec<_>>());
        assert_eq!(p.current().map(|e| e.key.as_str()), Some(COMMANDS[3].name));
    }
}
