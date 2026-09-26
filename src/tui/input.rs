use ratatui::crossterm::event::KeyCode;

pub(crate) const HINT_KEYS: &str = "asdfghjklqwertyuiopzxcvbnm";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Normal,
    Jump(String),
    Hint,
    Search(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Act {
    None,
    Pick(usize),
    Hint(usize),
    Move(i32),
    /* a page, or to the first or last line, of the full-screen card */
    Page(i32),
    Edge(i32),
    Stage(i32),
    Chain(i32),
    Card(bool),
    Find(i32),
    Refresh,
    Quit,
}

pub(crate) struct Input {
    pub mode: Mode,
    pub query: String,
}

impl Input {
    pub fn new() -> Self {
        Self {
            mode: Mode::Normal,
            query: String::new(),
        }
    }

    pub fn candidates(&self, count: usize) -> Vec<usize> {
        match &self.mode {
            Mode::Jump(buf) => (1..=count)
                .filter(|n| n.to_string().starts_with(buf.as_str()))
                .collect(),
            _ => Vec::new(),
        }
    }

    /* digits jump as soon as only one number starts with them, so every jump in a chain of up
    to nine jobs is one key; an ambiguous prefix waits for another digit or Enter, never for a
    timer */
    fn digits(&mut self, buf: String, count: usize) -> Act {
        let hits: Vec<usize> = (1..=count)
            .filter(|n| n.to_string().starts_with(buf.as_str()))
            .collect();
        match hits.as_slice() {
            [] => Act::None,
            [only] if only.to_string() == buf => Act::Pick(*only),
            _ => {
                self.mode = Mode::Jump(buf);
                Act::None
            }
        }
    }

    pub fn key(&mut self, code: KeyCode, count: usize) -> Act {
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Hint => match code {
                KeyCode::Char(c) => HINT_KEYS.find(c).map_or(Act::None, Act::Hint),
                _ => Act::None,
            },
            Mode::Jump(mut buf) => match code {
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    buf.push(c);
                    self.digits(buf, count)
                }
                KeyCode::Enter => buf
                    .parse()
                    .ok()
                    .filter(|n| (1..=count).contains(n))
                    .map_or(Act::None, Act::Pick),
                KeyCode::Backspace => {
                    buf.pop();
                    if !buf.is_empty() {
                        self.mode = Mode::Jump(buf);
                    }
                    Act::None
                }
                KeyCode::Esc => Act::None,
                _ => {
                    self.mode = Mode::Jump(buf);
                    Act::None
                }
            },
            Mode::Search(mut query) => match code {
                KeyCode::Char(c) => {
                    query.push(c);
                    self.mode = Mode::Search(query);
                    Act::None
                }
                KeyCode::Backspace => {
                    query.pop();
                    self.mode = Mode::Search(query);
                    Act::None
                }
                KeyCode::Enter => {
                    self.query = query;
                    Act::Find(0)
                }
                _ => Act::None,
            },
            Mode::Normal => match code {
                KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                    self.digits(c.to_string(), count)
                }
                KeyCode::Char('j') | KeyCode::Down => Act::Move(1),
                KeyCode::Char('k') | KeyCode::Up => Act::Move(-1),
                KeyCode::Char('l') | KeyCode::Right => Act::Stage(1),
                KeyCode::Char('h') | KeyCode::Left => Act::Stage(-1),
                KeyCode::Tab => Act::Chain(1),
                KeyCode::BackTab => Act::Chain(-1),
                KeyCode::Enter => Act::Card(true),
                KeyCode::Esc => Act::Card(false),
                KeyCode::Char('f') => {
                    self.mode = Mode::Hint;
                    Act::None
                }
                KeyCode::Char('/') => {
                    self.mode = Mode::Search(String::new());
                    Act::None
                }
                KeyCode::Char(' ') | KeyCode::PageDown => Act::Page(1),
                KeyCode::Char('b') | KeyCode::PageUp => Act::Page(-1),
                KeyCode::Char('g') | KeyCode::Home => Act::Edge(-1),
                KeyCode::Char('G') | KeyCode::End => Act::Edge(1),
                KeyCode::Char('n') => Act::Find(1),
                KeyCode::Char('N') => Act::Find(-1),
                KeyCode::Char('r') => Act::Refresh,
                KeyCode::Char('q') => Act::Quit,
                _ => Act::None,
            },
        }
    }
}
