//! A fuzzy picker in the style of fzf: a query line and a filtered list. Used for threads,
//! and later for the command palette and the file picker.

pub struct Entry {
    pub key: String,
    pub label: String,
    pub hint: String,
}

pub struct Picker {
    pub title: &'static str,
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
    pub fn new(title: &'static str, entries: Vec<Entry>) -> Self {
        Picker { title, entries, query: String::new(), selected: 0 }
    }

    pub fn matches(&self) -> Vec<&Entry> {
        let mut scored: Vec<(i32, &Entry)> = self
            .entries
            .iter()
            .filter_map(|e| fuzzy(&self.query, &format!("{} {}", e.label, e.hint)).map(|s| (s, e)))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
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
