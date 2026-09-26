use super::model::{Chain, Job, Peer, Snapshot};
use std::collections::{BTreeMap, HashMap};
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Plain,
    Dim,
    Line,
    Done,
    Run,
    Fail,
    Queued,
    Select,
    Near,
    Title,
    Warn,
    /* the WAIT! mark, which blinks */
    Wait,
    /* what a worker is doing now */
    Soft,
}

/* a wide character fills its cell and leaves the next one as a spacer the renderer skips */
pub(crate) const SPACER: char = '\0';

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    pub ch: char,
    pub tone: Tone,
    pub node: usize,
    pub glyph: bool,
}

const BLANK: Cell = Cell {
    ch: ' ',
    tone: Tone::Plain,
    node: 0,
    glyph: false,
};

#[derive(Clone, Default)]
pub(crate) struct Grid {
    pub rows: BTreeMap<usize, BTreeMap<usize, Cell>>,
}

pub(crate) fn width(text: &str) -> usize {
    text.chars().map(|ch| ch.width().unwrap_or(0)).sum()
}

/* cut to at most `max` columns, ending in … when anything was dropped */
pub(crate) fn fit(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w + 1 > max {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

impl Grid {
    pub fn put(&mut self, r: usize, c: usize, text: &str, tone: Tone) -> usize {
        self.put_node(r, c, text, tone, 0, false)
    }

    pub fn put_node(
        &mut self,
        r: usize,
        c: usize,
        text: &str,
        tone: Tone,
        node: usize,
        glyph: bool,
    ) -> usize {
        let row = self.rows.entry(r).or_default();
        let mut col = c;
        for ch in text.chars() {
            let w = ch.width().unwrap_or(0);
            if w == 0 {
                continue;
            }
            row.insert(
                col,
                Cell {
                    ch,
                    tone,
                    node,
                    glyph,
                },
            );
            if w == 2 {
                row.insert(
                    col + 1,
                    Cell {
                        ch: SPACER,
                        tone,
                        node,
                        glyph,
                    },
                );
            }
            col += w;
        }
        col
    }

    pub fn width(&self) -> usize {
        self.rows
            .values()
            .filter_map(|row| row.keys().next_back())
            .map(|c| c + 1)
            .max()
            .unwrap_or(0)
    }

    pub fn height(&self) -> usize {
        self.rows.keys().next_back().map_or(0, |r| r + 1)
    }

    /* copies the rows `from` of another grid, starting at row `at` */
    pub fn blit(&mut self, other: &Grid, from: std::ops::Range<usize>, at: usize) {
        for (r, row) in other.rows.range(from.clone()) {
            for (c, cell) in row {
                self.rows
                    .entry(at + r - from.start)
                    .or_default()
                    .insert(*c, *cell);
            }
        }
    }

    pub fn line(&self, r: usize, width: usize) -> Vec<Cell> {
        let row = self.rows.get(&r);
        (0..width)
            .map(|c| row.and_then(|row| row.get(&c)).copied().unwrap_or(BLANK))
            .collect()
    }

    #[cfg(test)]
    pub fn text(&self) -> String {
        let w = self.width();
        (0..self.height())
            .map(|r| {
                self.line(r, w)
                    .iter()
                    .filter(|cell| cell.ch != SPACER)
                    .map(|cell| cell.ch)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub(crate) fn glyph(state: &str) -> (char, Tone) {
    match state {
        "done" => ('●', Tone::Done),
        "running" => ('◆', Tone::Run),
        "failed" => ('×', Tone::Fail),
        "cancelled" => ('–', Tone::Queued),
        _ => ('○', Tone::Queued),
    }
}

/* a running job whose worker needs a person: at a permission prompt (WAIT!, with what
it asks), or idle with the ask still open (IDLE!) */
pub(crate) enum Alarm {
    Wait(String),
    Idle,
}

pub(crate) fn alarm(snap: &Snapshot, job: &Job) -> Option<Alarm> {
    if !snap.capabilities.peer_activity || job.state != "running" {
        return None;
    }
    let activity = snap.worker(job)?.activity.as_ref()?;
    let open = snap.ask(job.ask_id.as_deref()).is_some_and(|ask| ask.open);
    match activity.state.as_str() {
        "wait" => Some(Alarm::Wait(
            activity
                .reason
                .clone()
                .unwrap_or_else(|| "needs your permission".into()),
        )),
        "idle" if open => Some(Alarm::Idle),
        _ => None,
    }
}

/* local wall-clock time; tests read UTC so their clocks do not depend on TZ */
pub(crate) fn clock(t: Option<u64>) -> String {
    let Some(t) = t else {
        return "--:--".into();
    };
    let local = t as i64 + utc_offset(t);
    format!(
        "{:02}:{:02}",
        local.rem_euclid(86_400) / 3600,
        local.rem_euclid(3600) / 60
    )
}

#[cfg(not(test))]
fn utc_offset(t: u64) -> i64 {
    let time = t as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        return 0;
    }
    tm.tm_gmtoff as i64
}

#[cfg(test)]
fn utc_offset(_: u64) -> i64 {
    0
}

/* the part of an ask id a person tells apart at a glance */
pub(crate) fn short(id: &str) -> &str {
    let id = id.strip_prefix("ask-").unwrap_or(id);
    id.char_indices().nth(4).map_or(id, |(at, _)| &id[..at])
}

pub(crate) fn ago(now: u64, then: Option<u64>) -> String {
    let Some(then) = then else {
        return "--".into();
    };
    let s = now.saturating_sub(then);
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        _ => format!("{}h", s / 3600),
    }
}

/* numbers restart in every block, so cells carry base + number: every job on the screen
has its own node for hints and highlights */
pub(crate) struct View<'a> {
    pub snap: &'a Snapshot,
    pub chain: &'a Chain,
    pub num: &'a HashMap<String, usize>,
    pub base: usize,
    pub sel: &'a str,
}

impl View<'_> {
    fn title(&self, id: &str) -> String {
        self.snap
            .job(id)
            .map_or(id.to_string(), |job| job.title.clone())
    }

    fn state(&self, id: &str) -> &str {
        self.snap.job(id).map_or("queued", |job| job.state.as_str())
    }

    fn label(&self, id: &str, max: usize) -> String {
        format!("{} {}", self.num[id], fit(&self.title(id), max))
    }

    /* number and name together in at most `total` columns */
    fn label_in(&self, id: &str, total: usize) -> String {
        let digits = self.num[id].to_string().len();
        self.label(id, total.saturating_sub(digits + 1).max(1))
    }

    fn node(&self, id: &str) -> usize {
        self.base + self.num[id]
    }

    fn tone(&self, id: &str) -> Tone {
        let needs = |a: &str, b: &str| {
            self.chain
                .deps
                .get(a)
                .is_some_and(|deps| deps.iter().any(|d| d == b))
        };
        if id == self.sel {
            Tone::Select
        } else if needs(id, self.sel) || needs(self.sel, id) {
            Tone::Near
        } else {
            Tone::Plain
        }
    }

    /* worker and how long this attempt has been going, or how long it took */
    fn meta(&self, id: &str) -> String {
        let Some(job) = self.snap.job(id) else {
            return String::new();
        };
        let ask = self.snap.ask(job.ask_id.as_deref());
        let worker = job.assigned_peer.clone().unwrap_or_default();
        let age = match (job.state.as_str(), ask) {
            ("running", Some(ask)) => ago(self.snap.captured_at, ask.opened_at),
            ("done" | "failed", Some(ask)) => match (ask.opened_at, job.finished_at) {
                (Some(a), Some(b)) => ago(b, Some(a)),
                _ => String::new(),
            },
            _ => String::new(),
        };
        format!("{worker} {age}").trim().to_string()
    }

    /* a flow row marks a worker waiting for a permission; an idle one shows in the card */
    fn waiting(&self, id: &str) -> bool {
        self.snap
            .job(id)
            .is_some_and(|job| matches!(alarm(self.snap, job), Some(Alarm::Wait(_))))
    }

    /* worker, age and any partial-dependency note after a one-job row */
    fn tail(&self, id: &str) -> String {
        [self.meta(id), self.needs_note(id).unwrap_or_default()]
            .join(" ")
            .trim()
            .to_string()
    }

    fn put_glyph(&self, g: &mut Grid, r: usize, c: usize, id: &str) {
        let (ch, tone) = glyph(self.state(id));
        g.put_node(r, c, &ch.to_string(), tone, self.node(id), true);
    }

    fn needs_note(&self, id: &str) -> Option<String> {
        let s = self.chain.stage[id];
        let prev = self.chain.stages.get(s.checked_sub(1)?)?;
        let deps = &self.chain.deps[id];
        let partial = prev.len() > 1
            && prev.iter().any(|p| !deps.contains(p))
            && deps.iter().any(|d| prev.contains(d));
        partial.then(|| {
            let mut nums: Vec<usize> = deps
                .iter()
                .filter(|d| prev.contains(d))
                .map(|d| self.num[d])
                .collect();
            nums.sort();
            format!(
                "(needs {})",
                nums.iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        })
    }
}

const FOLD: usize = 5;

/* a skip-level edge: a dependency more than one stage up */
fn skips(v: &View) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (id, deps) in &v.chain.deps {
        for d in deps {
            if v.chain.stage[d] + 1 < v.chain.stage[id] {
                out.push((d.clone(), id.clone()));
            }
        }
    }
    out.sort_by_key(|(d, id)| (v.num[d], v.num[id]));
    out
}

/* every width is taken from the pane: each skip edge reserves a two-column rail lane (or,
when lanes would leave under 30 columns, becomes an "also needs" line below), a stage of
several jobs shares one slot width, and a stage that cannot give each job 8 columns folds
into a two-column box. Independent jobs have no brackets to draw, so from two on they
always share the box */
pub(crate) fn vertical(v: &View, cols: usize) -> Grid {
    let mut groups: Vec<Vec<String>> = v.chain.stages.clone();
    for group in &mut groups {
        group.sort_by_key(|id| v.num[id]);
    }
    let skips = skips(v);
    let lanes = 2 + 2 * skips.len();
    let rails = !skips.is_empty() && cols >= 30 + lanes;
    let room = if rails { cols - lanes } else { cols };
    let boxed: Vec<bool> = groups
        .iter()
        .map(|g| g.len() > FOLD || g.len() * 8 > room || (v.chain.loose && g.len() > 1))
        .collect();
    let open: Vec<&Vec<String>> = groups
        .iter()
        .zip(&boxed)
        .filter(|(g, b)| !**b && g.len() > 1)
        .map(|(g, _)| g)
        .collect();
    let widest = open.iter().map(|g| g.len()).max().unwrap_or(1);
    let natural = open
        .iter()
        .flat_map(|g| g.iter())
        .map(|id| width(&v.label(id, 16)))
        .max()
        .unwrap_or(1);
    let mut slot = (natural + 2).min(room / widest);
    slot -= slot % 2;
    let labels: HashMap<&String, String> = open
        .iter()
        .flat_map(|g| g.iter())
        .map(|id| (id, v.label_in(id, slot - 1)))
        .collect();
    let left = 2.max((slot - 1).div_ceil(2));
    /* without a stage side by side the spine sits at column 20 of a 46-column pane, as in
    the design, and moves left only as far as the longest one-job row needs */
    let center = if widest > 1 {
        left + slot * (widest - 1) / 2
    } else {
        let longest = groups
            .iter()
            .zip(&boxed)
            .filter(|(g, b)| !**b && g.len() == 1)
            .map(|(g, _)| {
                let id = &g[0];
                let mark = if v.waiting(id) { 6 } else { 0 };
                2 + width(&v.label(id, usize::MAX)) + 1 + width(&v.tail(id)) + mark
            })
            .max()
            .unwrap_or(0);
        (room / 2 - 3).min(room.saturating_sub(longest + 1)).max(2)
    };
    let mut g = Grid::default();
    let mut r = 0;
    let mut pos: HashMap<String, (usize, usize)> = HashMap::new();
    for (i, group) in groups.iter().enumerate() {
        if boxed[i] {
            let count = |s: &str| group.iter().filter(|id| v.state(id) == s).count();
            let mut counts = format!(
                "{}● {}◆ {}× {}○",
                count("done"),
                count("running"),
                count("failed"),
                count("queued")
            );
            if count("cancelled") > 0 {
                counts.push_str(&format!(" {}–", count("cancelled")));
            }
            let jobs = format!("{} jobs · {counts}", group.len());
            let range = format!(
                "{}-{} · {jobs}",
                v.num[&group[0]],
                v.num[group.last().unwrap()]
            );
            /* a narrow pane keeps the counts: the range goes first, then the job count */
            let title = [&range, &jobs, &counts]
                .into_iter()
                .find(|title| width(title) <= room - 4)
                .map_or_else(|| fit(&counts, room - 4), |title| title.to_string());
            let half = (room - 2) / 2;
            g.put(r, 0, &format!("┌{}┐", "─".repeat(room - 2)), Tone::Line);
            if i > 0 {
                g.put(r, center, "┴", Tone::Line);
            }
            r += 1;
            g.put(r, 0, "│", Tone::Line);
            g.put(r, room - 1, "│", Tone::Line);
            g.put(r, 2, &title, Tone::Dim);
            r += 1;
            for pair in group.chunks(2) {
                g.put(r, 0, "│", Tone::Line);
                g.put(r, room - 1, "│", Tone::Line);
                for (k, id) in pair.iter().enumerate() {
                    let x = 2 + k * half;
                    v.put_glyph(&mut g, r, x, id);
                    g.put_node(
                        r,
                        x + 2,
                        &v.label_in(id, half - 3),
                        v.tone(id),
                        v.node(id),
                        false,
                    );
                    pos.insert(id.clone(), (r, x));
                }
                r += 1;
            }
            g.put(r, 0, &format!("└{}┘", "─".repeat(room - 2)), Tone::Line);
            if i + 1 < groups.len() {
                g.put(r, center, "┬", Tone::Line);
            }
            r += 1;
            continue;
        }
        if group.len() == 1 {
            let id = &group[0];
            v.put_glyph(&mut g, r, center, id);
            let wait = v.waiting(id);
            let avail = room.saturating_sub(center + 2 + if wait { 6 } else { 0 });
            let tail = v.tail(id);
            let min = width(&v.num[id].to_string()) + 6;
            let (label, tail) = if width(&tail) + 1 + min <= avail {
                (v.label_in(id, avail - width(&tail) - 1), tail)
            } else {
                (v.label_in(id, avail), String::new())
            };
            let mut end = g.put_node(r, center + 2, &label, v.tone(id), v.node(id), false);
            if !tail.is_empty() {
                end = g.put(r, end + 1, &tail, Tone::Dim);
            }
            if wait {
                end = g.put(r, end + 1, "WAIT!", Tone::Wait);
            }
            pos.insert(id.clone(), (r, end));
            r += 1;
        } else {
            let k = group.len();
            for (n, id) in group.iter().enumerate() {
                let x = center + n * slot - slot * (k - 1) / 2;
                v.put_glyph(&mut g, r, x, id);
                let label = &labels[id];
                g.put_node(
                    r + 1,
                    x.saturating_sub((width(label) - 1) / 2),
                    label,
                    v.tone(id),
                    v.node(id),
                    false,
                );
                pos.insert(id.clone(), (r, x));
            }
            r += 2;
        }
        let Some(next) = groups.get(i + 1) else {
            continue;
        };
        if boxed[i + 1] {
            continue;
        }
        if group.len() == 1 && next.len() == 1 {
            g.put(r, center, "│", Tone::Line);
            r += 1;
            continue;
        }
        let fan_out = group.len() == 1;
        let k = if fan_out { next.len() } else { group.len() };
        let xs: Vec<usize> = (0..k)
            .map(|n| center + n * slot - slot * (k - 1) / 2)
            .collect();
        let (a, b) = (xs[0], xs[k - 1]);
        g.put(r, a, &"─".repeat(b - a + 1), Tone::Line);
        for x in &xs[1..k - 1] {
            g.put(r, *x, if fan_out { "┬" } else { "┴" }, Tone::Line);
        }
        g.put(r, a, if fan_out { "┌" } else { "└" }, Tone::Line);
        g.put(r, b, if fan_out { "┐" } else { "┘" }, Tone::Line);
        let join = if xs.contains(&center) {
            "┼"
        } else if fan_out {
            "┴"
        } else {
            "┬"
        };
        g.put(r, center, join, Tone::Line);
        r += 1;
    }
    if !rails {
        for (d, id) in &skips {
            g.put(
                r,
                0,
                &fit(
                    &format!("{} also needs {}", v.label_in(id, 20), v.label_in(d, 20)),
                    cols,
                ),
                Tone::Dim,
            );
            r += 1;
        }
        return g;
    }
    let rail = g.width() + 2;
    for (lane, (d, id)) in skips.iter().enumerate() {
        let x = rail + lane * 2;
        let ((r0, c0), (r1, c1)) = (pos[d], pos[id]);
        g.put(
            r0,
            c0 + 1,
            &format!("{}┐", "─".repeat(x - c0 - 1)),
            Tone::Line,
        );
        for rr in r0 + 1..r1 {
            g.put(rr, x, "│", Tone::Line);
        }
        g.put(
            r1,
            c1 + 1,
            &format!("◀{}┘", "─".repeat(x - c1 - 2)),
            Tone::Line,
        );
    }
    g
}

/* stages left to right with whole names; when they do not fit, labels shrink from 14 to 8
columns, and a chain that still does not fit gets None so the caller falls back to the
vertical flow */
pub(crate) fn horizontal(v: &View, cols: usize) -> Option<Grid> {
    std::iter::once(usize::MAX)
        .chain((8..=14).rev())
        .map(|max| horizontal_with(v, max))
        .find(|g| g.width() <= cols)
}

fn horizontal_with(v: &View, max: usize) -> Grid {
    let mut groups: Vec<Vec<String>> = v.chain.stages.clone();
    for group in &mut groups {
        group.sort_by_key(|id| v.num[id]);
    }
    let mut g = Grid::default();
    let mut c = 0;
    let mut spot: HashMap<String, (usize, usize)> = HashMap::new();
    for (i, group) in groups.iter().enumerate() {
        let labels: Vec<String> = group.iter().map(|id| v.label(id, max)).collect();
        let w = labels.iter().map(|l| width(l)).max().unwrap_or(0);
        for (n, id) in group.iter().enumerate() {
            v.put_glyph(&mut g, n, c, id);
            g.put_node(n, c + 2, &labels[n], v.tone(id), v.node(id), false);
            spot.insert(id.clone(), (n, c + 2));
        }
        let Some(next) = groups.get(i + 1) else {
            continue;
        };
        if group.len() > 1 {
            let x = c + 2 + w + 2;
            for (n, label) in labels.iter().enumerate() {
                g.put(
                    n,
                    c + 2 + width(label),
                    &format!(" {}", "─".repeat(w - width(label) + 1)),
                    Tone::Line,
                );
                g.put(
                    n,
                    x,
                    if n == 0 {
                        "┬"
                    } else if n + 1 < group.len() {
                        "┤"
                    } else {
                        "┘"
                    },
                    Tone::Line,
                );
            }
            g.put(0, x + 1, "── ", Tone::Line);
            c = x + 4;
        } else if next.len() > 1 {
            let x = c + 2 + w + 1;
            g.put(0, x, "─┬─ ", Tone::Line);
            for n in 1..next.len() {
                g.put(
                    n,
                    x + 1,
                    if n + 1 < next.len() {
                        "├─ "
                    } else {
                        "└─ "
                    },
                    Tone::Line,
                );
            }
            c = x + 4;
        } else {
            let x = c + 2 + w + 1;
            g.put(0, x, "── ", Tone::Line);
            c = x + 3;
        }
    }
    /* the note sits under its job, or below the flow when a stage fills that spot */
    for (d, id) in skips(v) {
        let (row, col) = spot[&id];
        let text = format!("↑ also needs {}", v.label(&d, max));
        let at = col.saturating_sub(2);
        let free = (at..at + width(&text)).all(|c| {
            g.rows
                .get(&(row + 1))
                .is_none_or(|cells| !cells.contains_key(&c))
        });
        let r = if free { row + 1 } else { g.height() };
        g.put(r, at, &text, Tone::Dim);
    }
    g
}

/* the pieces a line may break between: a run of narrow characters, or one wide character,
since CJK text breaks between any two characters; each says whether a space came first */
fn pieces(text: &str) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    let (mut space, mut word) = (false, String::new());
    for ch in text.chars() {
        let wide = ch.width() == Some(2);
        if (ch.is_whitespace() || wide) && !word.is_empty() {
            out.push((space, std::mem::take(&mut word)));
            space = false;
        }
        if ch.is_whitespace() {
            space = true;
        } else if wide {
            out.push((space, ch.to_string()));
            space = false;
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push((space, word));
    }
    out
}

/* a long text, the width it was wrapped at, and its lines */
type Wrapped = (String, usize, Vec<String>);

thread_local! {
    /* the last long texts wrapped: a card is drawn eight times a second, and wrapping a
    100 KB prompt each time costs a core. A text is matched whole, so a hit is always the
    same text */
    static WRAPPED: std::cell::RefCell<Vec<Wrapped>> = const { std::cell::RefCell::new(Vec::new()) };
}

/* long texts wrapped afresh and whole cards built, for tests that count them */
#[cfg(test)]
thread_local! {
    pub(crate) static LONG_WRAPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static WHOLE_CARDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(crate) fn wrap(text: &str, max: usize) -> Vec<String> {
    if text.len() < 2048 {
        return wrap_now(text, max);
    }
    WRAPPED.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((.., lines)) = cache.iter().find(|(t, m, _)| *m == max && t == text) {
            return lines.clone();
        }
        let lines = wrap_now(text, max);
        if cache.len() >= 4 {
            cache.remove(0);
        }
        cache.push((text.to_string(), max, lines.clone()));
        lines
    })
}

/* every character is kept when a line can hold the widest: a piece wider than the line is
split across lines, and a character wider than a whole line becomes … */
fn wrap_now(text: &str, max: usize) -> Vec<String> {
    #[cfg(test)]
    if text.len() >= 2048 {
        LONG_WRAPS.with(|n| n.set(n.get() + 1));
    }
    let max = max.max(1);
    let mut lines = vec![String::new()];
    let mut used = 0;
    for (space, piece) in pieces(text) {
        let gap = usize::from(space && used > 0);
        if used > 0 && used + gap + width(&piece) > max {
            lines.push(String::new());
            used = 0;
        } else if gap == 1 {
            lines.last_mut().expect("starts with one line").push(' ');
            used += 1;
        }
        for ch in piece.chars() {
            let (ch, w) = match ch.width().unwrap_or(0) {
                w if w > max => ('…', 1),
                w => (ch, w),
            };
            if used > 0 && used + w > max {
                lines.push(String::new());
                used = 0;
            }
            lines.last_mut().expect("starts with one line").push(ch);
            used += w;
        }
    }
    lines
}

/* cut to `max` columns ending in …, which always shows */
fn ellipsis(text: &str, max: usize) -> String {
    let mut out = text.to_string();
    while !out.is_empty() && width(&out) + 1 > max {
        out.pop();
    }
    out.push('…');
    out
}

/* single quotes unless the word is plainly safe, so a pasted command never runs part of a
title as shell */
pub(crate) fn quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@".contains(c))
    {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

type Row = Vec<(String, Tone)>;

const KEY: usize = 8;

/* a card names at most three dependencies on one line, as in the design */
const LISTED: usize = 3;

fn key(k: &str, first: bool) -> (String, Tone) {
    debug_assert!(width(k) < KEY, "{k} leaves no space before its value");
    (
        if first {
            format!("{k:<KEY$}")
        } else {
            " ".repeat(KEY)
        },
        Tone::Dim,
    )
}

/* the first n lines of a field; the last ends in … when any were left out */
fn field(k: &str, lines: &[String], n: usize, tone: Tone, room: usize) -> Vec<Row> {
    let mut out: Vec<Row> = lines
        .iter()
        .take(n)
        .enumerate()
        .map(|(i, line)| vec![key(k, i == 0), (line.clone(), tone)])
        .collect();
    if n < lines.len() {
        if let Some(last) = out.last_mut() {
            last[1].0 = ellipsis(&last[1].0, room);
        }
    }
    out
}

fn wrapped(side: &mut Vec<Row>, k: &str, text: &str, tone: Tone, max: usize, lines: usize) {
    let room = max.saturating_sub(KEY);
    side.extend(field(k, &wrap(text, room), lines, tone, room));
}

/* whole items per line, as many lines as they need */
fn packed(side: &mut Vec<Row>, k: &str, items: Vec<Row>, max: usize) {
    let room = max.saturating_sub(KEY);
    let (mut lines, mut used): (Vec<Row>, usize) = (vec![Vec::new()], 0);
    for item in items {
        let w: usize = item.iter().map(|(t, _)| width(t)).sum();
        if used > 0 && used + 1 + w > room {
            lines.push(Vec::new());
            used = 0;
        }
        let line = lines.last_mut().expect("starts with one line");
        if used > 0 {
            line.push((" ".into(), Tone::Plain));
            used += 1;
        }
        line.extend(item);
        used += w;
    }
    for (i, line) in lines.into_iter().enumerate() {
        side.push([vec![key(k, i == 0)], line].concat());
    }
}

/* one line of up to three items, as many as fit; the design joins what a job waits for
with spaces and what it blocks with dots */
fn listed(side: &mut Vec<Row>, k: &str, items: Vec<Row>, max: usize, sep: &str) {
    let room = max.saturating_sub(KEY);
    let (mut line, mut used) = (vec![key(k, true)], 0);
    for (n, item) in items.into_iter().take(LISTED).enumerate() {
        let w: usize = item.iter().map(|(t, _)| width(t)).sum();
        if n > 0 {
            if used + width(sep) + w > room {
                break;
            }
            line.push((sep.into(), Tone::Dim));
            used += width(sep);
        }
        line.extend(item);
        used += w;
    }
    side.push(line);
}

/* how tall a card is: all it has, or exactly the rows the pane gives it */
#[derive(Clone, Copy, Debug)]
pub(crate) enum Fit {
    Whole,
    Rows(usize),
}

/* the design's card: fields on the left and, in a wide pane, the prompt and any command
on the right */
pub(crate) fn card(
    v: &View,
    id: &str,
    cols: usize,
    full_text: Option<(&str, Option<&str>)>,
    size: Fit,
) -> Grid {
    #[cfg(test)]
    if matches!(size, Fit::Whole) {
        WHOLE_CARDS.with(|n| n.set(n.get() + 1));
    }
    let snap = v.snap;
    let job = snap.job(id).expect("selected job is in the snapshot");
    let ask = snap.ask(job.ask_id.as_deref());
    let now = snap.captured_at;
    let two = cols >= 96;
    let inner = cols.saturating_sub(4);
    let colw = if two { inner / 2 - 1 } else { inner };
    let (mut left, mut right, mut commands): (Vec<Row>, Vec<Row>, Vec<Row>) =
        (Vec::new(), Vec::new(), Vec::new());
    let prompt = full_text.map_or(job.prompt.as_str(), |(p, _)| p);
    let result = full_text.map_or(job.result.as_deref(), |(_, r)| r);
    let worker = job.assigned_peer.as_deref().unwrap_or("-");
    let state = job.state.as_str();
    let alarm = alarm(snap, job);
    let idle = matches!(alarm, Some(Alarm::Idle));
    let sent = ask.and_then(|a| a.opened_at).filter(|_| state != "queued");
    /* up to three items share a line, so each name gets its share of the card's width */
    let share = |count: usize| {
        (colw.saturating_sub(KEY) / count.clamp(1, LISTED))
            .saturating_sub(4)
            .max(10)
    };
    let item = |d: &String, name: usize| -> Row {
        match (v.num.get(d), snap.job(d)) {
            (Some(n), Some(dep)) => {
                let (ch, tone) = glyph(&dep.state);
                vec![
                    (format!("{n} {} ", fit(&dep.title, name)), Tone::Near),
                    (ch.to_string(), tone),
                ]
            }
            _ => vec![(format!("{} deleted", fit(d, 14)), Tone::Fail)],
        }
    };
    let items = |ids: &[String]| -> Vec<Row> {
        let name = share(ids.len());
        ids.iter().map(|d| item(d, name)).collect()
    };
    let by_number = |mut ids: Vec<String>| {
        ids.sort_by_key(|x| v.num.get(x).copied().unwrap_or(usize::MAX));
        ids
    };
    let lead: Option<(&str, &str, Tone)> = match state {
        "done" => Some(("result", result.unwrap_or("-"), Tone::Done)),
        "failed" => Some((
            "reason",
            result
                .filter(|r| !r.is_empty())
                .unwrap_or("no reason given"),
            Tone::Fail,
        )),
        _ => None,
    };
    /* the worker in the design's words: WORK in a turn on this job, IDLE! with the ask
    open, WAIT! at a permission prompt, else what it does now. The card holds still; the
    flow above it carries the spinner */
    let doing = |p: &Peer| -> String {
        if p.status != "online" {
            return format!("{} · offline", p.name);
        }
        match p
            .activity
            .as_ref()
            .filter(|_| snap.capabilities.peer_activity)
        {
            Some(a) => format!("{} · now {}", p.name, a.state.to_uppercase()),
            None => p.name.clone(),
        }
    };
    match (state, snap.worker(job)) {
        ("queued", _) => left.push(vec![
            key("worker", true),
            (format!("{worker} (when ready)"), Tone::Dim),
        ]),
        ("running", None) if ask.is_some() => wrapped(
            &mut left,
            "worker",
            &format!(
                "{} left the hub",
                ask.map_or(worker, |a| a.to_peer_id.as_str())
            ),
            Tone::Warn,
            colw,
            usize::MAX,
        ),
        ("running", Some(p)) => {
            let since = p
                .activity
                .as_ref()
                .map_or(String::new(), |a| ago(now, Some(a.since)));
            let name = (format!("{} ", p.name), Tone::Plain);
            match &alarm {
                Some(Alarm::Wait(reason)) => {
                    left.push(vec![
                        key("worker", true),
                        name,
                        (format!("WAIT! {since}"), Tone::Warn),
                    ]);
                    wrapped(&mut left, "", reason, Tone::Warn, colw, 2);
                }
                Some(Alarm::Idle) => left.push(vec![
                    key("worker", true),
                    name,
                    (format!("IDLE! {since}"), Tone::Fail),
                    (" · turn ended".into(), Tone::Dim),
                ]),
                None if snap.spinning(job) => left.push(vec![
                    key("worker", true),
                    name,
                    (format!("WORK · turn {since}"), Tone::Run),
                ]),
                None => left.push(vec![key("worker", true), (doing(p), Tone::Soft)]),
            }
        }
        (_, Some(p)) => left.push(vec![key("worker", true), (doing(p), Tone::Soft)]),
        (_, None) => left.push(vec![key("worker", true), (worker.to_string(), Tone::Soft)]),
    }
    if let Some(ask) = ask {
        /* the hub records how an ask closed; "acked" is the recipient's own answer, and an
        ask closed before the hub kept this says only whether it went well */
        let outcome = |ok: &'static str, failed: &'static str| {
            if ask.failed {
                (failed, Tone::Fail)
            } else {
                (ok, Tone::Done)
            }
        };
        let (word, tone) = match (state, ask.open, ask.closed_by.as_deref()) {
            ("queued", _, _) => ("previous attempt", Tone::Dim),
            (_, true, _) => ("unacked", if idle { Tone::Fail } else { Tone::Plain }),
            (_, false, Some("recipient")) => outcome("acked ok", "acked failed"),
            (_, false, Some("hand")) => outcome("closed by hand", "closed by hand"),
            (_, false, Some("hub")) => ("closed by hub", Tone::Fail),
            (_, false, _) => outcome("closed ok", "closed failed"),
        };
        /* a closed ask on a job still running settles on the hub's next pass */
        let (tail, tone) = if state == "running" && !ask.open {
            (format!("{word}, settling"), Tone::Warn)
        } else {
            (word.to_string(), tone)
        };
        let to = if ask.to_peer.is_empty() {
            &ask.to_peer_id
        } else {
            &ask.to_peer
        };
        let route = if ask.from_peer.is_empty() {
            to.clone()
        } else {
            format!("{}>{to}", ask.from_peer)
        };
        packed(
            &mut left,
            "ask",
            vec![
                vec![(
                    format!("{} {route}", short(&ask.correlation_id)),
                    Tone::Plain,
                )],
                vec![(format!("· {}", clock(ask.opened_at)), Tone::Plain)],
                vec![("· ".into(), Tone::Plain), (tail, tone)],
            ],
            colw,
        );
        if idle {
            left.push(vec![
                key("", false),
                ("⣿".repeat(12), Tone::Fail),
                (format!(" waiting {}", ago(now, ask.opened_at)), Tone::Fail),
            ]);
        }
    } else if let Some(cid) = job.ask_id.as_deref() {
        /* settle_job fails a running job whose ask is gone */
        let text = if state == "running" {
            format!("{cid} is missing: the hub fails this job at its next check")
        } else {
            format!("{cid} was cleaned up")
        };
        wrapped(&mut left, "ask", &text, Tone::Warn, colw, usize::MAX);
    }
    if state == "queued" {
        /* the hub's rule: every dependency done, and a failed, cancelled or deleted one blocks */
        let waits = by_number(
            job.depends_on
                .iter()
                .filter(|d| snap.job(d).is_none_or(|dep| dep.state != "done"))
                .cloned()
                .collect(),
        );
        let blocked = waits.iter().find(|d| {
            snap.job(d)
                .is_none_or(|dep| matches!(dep.state.as_str(), "failed" | "cancelled"))
        });
        if waits.is_empty() {
            let (note, tone) = match snap.peer(worker) {
                /* only an update that names the assignee turns dispatch on */
                _ if !job.dispatch && job.assigned_peer.is_some() => (
                    "held: set its assignee again to send it".to_string(),
                    Tone::Dim,
                ),
                _ if !job.dispatch => (
                    "not dispatched: name an assignee to send it".to_string(),
                    Tone::Dim,
                ),
                None => (format!("no peer named {worker}: not sent"), Tone::Dim),
                Some(p) if !job.circle.is_empty() && p.circle != job.circle => (
                    format!("{worker} is in another circle: not sent"),
                    Tone::Dim,
                ),
                Some(_) => ("nothing: will be sent next tick".to_string(), Tone::Done),
            };
            wrapped(&mut left, "waits", &note, tone, colw, usize::MAX);
        } else {
            listed(&mut left, "waits", items(&waits), colw, "  ");
        }
        if let Some(bad) = blocked {
            let text = match snap.job(bad) {
                Some(dep) => format!(
                    "{} {} {}: retry it first",
                    v.num.get(bad).copied().unwrap_or(0),
                    fit(&dep.title, share(1)),
                    dep.state
                ),
                None => format!("{} was deleted: this job cannot start", fit(bad, 14)),
            };
            wrapped(&mut left, "blocked", &text, Tone::Fail, colw, usize::MAX);
        }
    } else {
        let needs = by_number(v.chain.deps.get(id).cloned().unwrap_or_default());
        if !needs.is_empty() {
            listed(&mut left, "needs", items(&needs), colw, "  ");
        }
    }
    let blocks = by_number(v.chain.dependents(id));
    if !blocks.is_empty() {
        listed(&mut left, "blocks", items(&blocks), colw, " · ");
    }
    let end = job.finished_at;
    let times = if state == "queued" {
        "not sent yet".to_string()
    } else {
        let mut parts: Vec<String> = sent
            .map(|t| format!("sent {}", clock(Some(t))))
            .into_iter()
            .collect();
        match state {
            "running" => parts.extend(sent.map(|t| format!("running {}", ago(now, Some(t))))),
            "done" => {
                parts.extend(end.map(|t| format!("ended {}", clock(Some(t)))));
                if let (Some(a), Some(b)) = (sent, end) {
                    parts.push(format!("took {}", ago(b, Some(a))));
                }
            }
            other => parts.extend(end.map(|t| format!("{other} {}", clock(Some(t))))),
        }
        if parts.is_empty() {
            "--".to_string()
        } else {
            parts.join(" · ")
        }
    };
    wrapped(&mut left, "times", &times, Tone::Dim, colw, usize::MAX);
    match state {
        "failed" => {
            let mut command = format!("amesh jobs update {} --state queued", quote(id));
            if !job.dispatch {
                command.push_str(&format!(
                    " --assigned-peer {}",
                    quote(job.assigned_peer.as_deref().unwrap_or("PEER"))
                ));
            }
            wrapped(
                &mut commands,
                "retry",
                &command,
                Tone::Dim,
                colw,
                usize::MAX,
            );
        }
        "queued" if !job.dispatch => {
            let command = format!(
                "amesh jobs update {} --state queued --assigned-peer {}",
                quote(id),
                quote(job.assigned_peer.as_deref().unwrap_or("PEER"))
            );
            wrapped(&mut commands, "send", &command, Tone::Dim, colw, usize::MAX);
        }
        "running" if idle => {
            /* the ask's recipient by peer id: a name may have passed to another peer */
            let to = ask
                .map(|a| a.to_peer_id.as_str())
                .filter(|to| !to.is_empty())
                .unwrap_or(worker);
            let command = format!(
                "amesh peer notify {} {}",
                quote(to),
                quote(&format!("{}?", job.title))
            );
            wrapped(
                &mut commands,
                "nudge",
                &command,
                Tone::Dim,
                colw,
                usize::MAX,
            );
        }
        /* a worker that left the hub without answering will not answer; sending the job
        again, to a peer that is here, is the way on. An answer already given only waits
        for the hub to settle it */
        "running" if ask.is_some_and(|a| a.open) && snap.worker(job).is_none() => {
            let command = format!(
                "amesh jobs update {} --state queued --assigned-peer PEER",
                quote(id)
            );
            wrapped(
                &mut commands,
                "resend",
                &command,
                Tone::Dim,
                colw,
                usize::MAX,
            );
        }
        _ => {}
    }
    /* result or reason and the prompt take the rows the pane has left, result first, and
    the frame stretches to the rows it was given; a field cut short ends in … */
    let room = colw.saturating_sub(KEY);
    let lead_lines = lead.map_or(Vec::new(), |(_, text, _)| wrap(text, room));
    let prompt_lines = wrap(prompt, room);
    let (want_r, want_p) = (lead_lines.len(), prompt_lines.len());
    let least = |want: usize| want.min(1);
    let (n_r, n_p) = match size {
        Fit::Whole => (want_r, want_p),
        Fit::Rows(h) if two => {
            let inner = h.saturating_sub(2);
            (
                want_r
                    .min(inner.saturating_sub(left.len()))
                    .max(least(want_r)),
                want_p
                    .min(inner.saturating_sub(commands.len()))
                    .max(least(want_p)),
            )
        }
        Fit::Rows(h) => {
            let space = h.saturating_sub(2 + left.len() + commands.len());
            let r = want_r
                .min(space.saturating_sub(least(want_p)))
                .max(least(want_r));
            (r, want_p.min(space.saturating_sub(r)).max(least(want_p)))
        }
    };
    if let Some((k, _, tone)) = lead {
        left.splice(0..0, field(k, &lead_lines, n_r, tone, room));
    }
    let side = if two { &mut right } else { &mut left };
    side.extend(field("prompt", &prompt_lines, n_p, Tone::Plain, room));
    side.extend(commands);
    if let Fit::Rows(h) = size {
        while 2 + left.len().max(right.len()) < h {
            left.push(Vec::new());
        }
    }
    let state_text = match state {
        "done" => match (sent, end) {
            (Some(a), Some(b)) => format!("done · took {}", ago(b, Some(a))),
            _ => "done".into(),
        },
        "running" => sent.map_or("running".into(), |t| {
            format!("running {}", ago(now, Some(t)))
        }),
        "failed" | "cancelled" => end.map_or(state.to_string(), |t| {
            format!("{state} {} ago", ago(now, Some(t)))
        }),
        other => other.to_string(),
    };
    /* the name takes the width the header leaves; the stage gives way first */
    let stage = if v.chain.loose {
        String::new()
    } else {
        format!(
            "· stage {} of {} ",
            v.chain.stage[id] + 1,
            v.chain.stages.len()
        )
    };
    let bare = width(&format!(" {} · {state_text} ", v.num[id])) + 1;
    let room = cols.saturating_sub(4 + bare);
    let keep_stage = width(&stage) + width(&job.title).min(16) <= room;
    let title = fit(
        &job.title,
        room.saturating_sub(if keep_stage { width(&stage) } else { 0 }),
    );
    let mut head = format!(" {} {title} · {state_text} ", v.num[id]);
    if keep_stage {
        head.push_str(&stage);
    }
    let mut g = Grid::default();
    g.put(0, 0, "┌", Tone::Line);
    let (ch, tone) = glyph(state);
    g.put(0, 1, &ch.to_string(), tone);
    let at = g.put(0, 2, &fit(&head, cols.saturating_sub(4)), Tone::Title);
    g.put(
        0,
        at,
        &format!("{}┐", "─".repeat(cols.saturating_sub(at + 1))),
        Tone::Line,
    );
    let draw = |g: &mut Grid, r: usize, start: usize, row: Option<&Row>| {
        let mut col = start;
        for (text, tone) in row.into_iter().flatten() {
            let room = (start + colw).saturating_sub(col);
            if room == 0 {
                break;
            }
            col = g.put(r, col, &fit(text, room), *tone);
        }
    };
    let split = 2 + colw + 1;
    let mut r = 1;
    for i in 0..left.len().max(right.len()) {
        g.put(r, 0, "│", Tone::Line);
        g.put(r, cols - 1, "│", Tone::Line);
        draw(&mut g, r, 2, left.get(i));
        if two {
            g.put(r, split, "│", Tone::Line);
            draw(&mut g, r, split + 2, right.get(i));
        }
        r += 1;
    }
    g.put(r, 0, &format!("└{}┘", "─".repeat(cols - 2)), Tone::Line);
    g
}
