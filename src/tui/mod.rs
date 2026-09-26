mod input;
mod layout;
mod model;
#[cfg(test)]
mod tests;
mod theme;

use input::{Act, Input, Mode, HINT_KEYS};
use layout::{Cell, Fit, Grid, Tone, View, SPACER};
use model::{Chain, Numbers, Snapshot};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use theme::Theme;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub(crate) struct Opts {
    pub circle: Option<String>,
    pub ascii: bool,
    pub color: bool,
    pub anim: bool,
    pub theme: Theme,
}

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub(crate) struct App {
    opts: Opts,
    snap: Option<Snapshot>,
    error: Option<String>,
    last_ok: Option<Instant>,
    numbers: Numbers,
    sel: Option<String>,
    input: Input,
    full: bool,
    /* the job under each hint letter on the last screen drawn */
    hints: Vec<String>,
    /* Numbers::renumbered when the last key was taken */
    typed_at: u64,
    /* the full-screen card's first shown line, and how many lines a page is */
    scroll: usize,
    page: usize,
}

/* a chain or the loose block, numbered 1..n; base counts the jobs in the blocks above */
struct Block {
    chain: Chain,
    order: Vec<String>,
    num: HashMap<String, usize>,
    base: usize,
}

impl Block {
    fn view<'a>(&'a self, snap: &'a Snapshot, sel: &'a str) -> View<'a> {
        View {
            snap,
            chain: &self.chain,
            num: &self.num,
            base: self.base,
            sel,
        }
    }
}

impl App {
    pub fn new(opts: Opts) -> Self {
        Self {
            opts,
            snap: None,
            error: None,
            last_ok: None,
            numbers: Numbers::default(),
            sel: None,
            input: Input::new(),
            full: false,
            hints: Vec::new(),
            typed_at: 0,
            scroll: 0,
            page: 10,
        }
    }

    pub fn apply(&mut self, fetched: std::result::Result<Snapshot, String>) {
        match fetched {
            Ok(snap) => {
                self.snap = Some(snap);
                self.error = None;
                self.last_ok = Some(Instant::now());
            }
            Err(error) if error.contains("returned error: 404") => {
                self.error =
                    Some("this hub has no /snapshot; restart it on the current amesh".into());
            }
            Err(error) if error.contains("returned error: 401") => {
                self.error = Some("the hub refused the token; check AMESH_TOKEN".into())
            }
            Err(error) if error.contains(crate::cli::DAEMON_UNREACHABLE) => {
                self.error = Some("hub unreachable".into())
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn blocks(&mut self) -> Vec<Block> {
        let Some(snap) = &self.snap else {
            return Vec::new();
        };
        self.numbers.forget_gone(snap);
        let mut base = 0;
        model::chains(snap)
            .into_iter()
            .map(|chain| {
                let pairs = self.numbers.assign(&chain);
                let order: Vec<String> = pairs.iter().map(|(_, id)| id.clone()).collect();
                let num = pairs.into_iter().map(|(n, id)| (id, n)).collect();
                let block = Block {
                    chain,
                    order,
                    num,
                    base,
                };
                base += block.order.len();
                block
            })
            .collect()
    }

    /* the selected job while it exists, else the first running job, else the first job;
    returns its block and its place in that block */
    fn settle(&mut self, blocks: &[Block]) -> Option<(usize, usize)> {
        let find = |id: &str| {
            blocks
                .iter()
                .enumerate()
                .find_map(|(b, block)| block.order.iter().position(|x| x == id).map(|p| (b, p)))
        };
        if let Some(at) = self.sel.as_deref().and_then(find) {
            return Some(at);
        }
        /* the selected job is gone: a half-typed jump meant a number in its block, and the
        card that takes its place starts at its top */
        if matches!(self.input.mode, Mode::Jump(_)) {
            self.input.mode = Mode::Normal;
        }
        self.scroll = 0;
        let snap = self.snap.as_ref()?;
        let all = || blocks.iter().flat_map(|block| &block.order);
        let pick = all()
            .find(|id| snap.job(id).is_some_and(|job| job.state == "running"))
            .or_else(|| all().next())?
            .clone();
        let at = find(&pick);
        self.sel = Some(pick);
        at
    }

    pub fn key(&mut self, code: KeyCode) -> Act {
        let blocks = self.blocks();
        /* digits typed against numbers that have since changed would land on another job */
        if matches!(self.input.mode, Mode::Jump(_)) && self.numbers.renumbered != self.typed_at {
            self.input.mode = Mode::Normal;
            self.typed_at = self.numbers.renumbered;
            return Act::None;
        }
        self.typed_at = self.numbers.renumbered;
        let Some((b, p)) = self.settle(&blocks) else {
            return self.input.key(code, 0);
        };
        let cur = &blocks[b];
        let act = self.input.key(code, cur.order.len());
        let all: Vec<&String> = blocks.iter().flat_map(|block| &block.order).collect();
        let (at, n) = ((cur.base + p) as i32, all.len() as i32);
        let sel = cur.order[p].clone();
        let before = self.sel.clone();
        match act {
            /* the full card has the focus: j/k scroll it, and h/l, digits and tab still
            pick another job */
            Act::Move(d) if self.full => {
                self.scroll = self.scroll.saturating_add_signed(d as isize);
            }
            Act::Page(d) if self.full => {
                self.scroll = self
                    .scroll
                    .saturating_add_signed(d as isize * self.page as isize);
            }
            Act::Edge(d) if self.full => self.scroll = if d < 0 { 0 } else { usize::MAX },
            Act::Pick(num) => self.sel = cur.order.get(num - 1).cloned().or(Some(sel)),
            Act::Hint(i) => self.sel = self.hints.get(i).cloned().or(Some(sel)),
            Act::Move(d) => {
                let len = cur.order.len() as i32;
                self.sel = Some(cur.order[(p as i32 + d).rem_euclid(len) as usize].clone());
            }
            Act::Stage(d) => {
                let mut row = cur.chain.stages[cur.chain.stage[&sel]].clone();
                row.sort_by_key(|id| cur.num[id]);
                let q = row.iter().position(|id| *id == sel).unwrap_or(0) as i32;
                self.sel = Some(row[(q + d).rem_euclid(row.len() as i32) as usize].clone());
            }
            Act::Chain(d) => {
                let next = (b as i32 + d).rem_euclid(blocks.len() as i32) as usize;
                self.sel = blocks[next].order.first().cloned();
            }
            Act::Card(open) => {
                self.full = open;
                self.scroll = 0;
            }
            Act::Find(d) => {
                let query = self.input.query.to_lowercase();
                let snap = self.snap.as_ref();
                let hit = |id: &&String| {
                    snap.and_then(|s| s.job(id))
                        .is_some_and(|job| job.title.to_lowercase().contains(&query))
                };
                let (start, step) = if d == 0 {
                    (at, 1)
                } else {
                    (at + d, d.signum())
                };
                if let Some(id) = (0..n)
                    .map(|k| all[(start + k * step).rem_euclid(n) as usize])
                    .find(hit)
                {
                    self.sel = Some(id.clone());
                }
            }
            _ => {}
        }
        /* another job's card starts at its top */
        if self.sel != before {
            self.scroll = 0;
        }
        act
    }

    /* the design's screen: the selected job's chain alone under its header, the flow, then
    the card; Tab moves to the next chain */
    pub fn screen(&mut self, cols: usize, rows: usize, tick: usize) -> Vec<Line<'static>> {
        let mut g = Grid::default();
        let blocks = self.blocks();
        let settled = self.settle(&blocks);
        if cols < 30 || rows < 8 {
            g.put(0, 0, "pane too small", Tone::Warn);
            return self.lines(&g, cols, rows);
        }
        let Some((b, _)) = settled else {
            let (text, hint) = match (&self.error, &self.snap) {
                (Some(error), _) => (error.as_str(), ""),
                (None, Some(_)) => (
                    "no jobs here yet",
                    "jobs appear once created: amesh jobs create TITLE --assigned-peer ID",
                ),
                (None, None) => ("waiting for the hub", ""),
            };
            let lines = layout::wrap(text, cols);
            for (r, line) in lines.iter().enumerate() {
                g.put(r, 0, line, Tone::Warn);
            }
            for (r, line) in layout::wrap(hint, cols).iter().enumerate() {
                g.put(lines.len() + r, 0, line, Tone::Dim);
            }
            return self.lines(&g, cols, rows);
        };
        let snap = self.snap.clone().expect("blocks come from a snapshot");
        let sel = self.sel.clone().expect("settle picked a job");
        let cur = &blocks[b];
        let mut count: HashMap<&str, usize> = HashMap::new();
        for id in &cur.order {
            *count
                .entry(snap.job(id).map_or("", |job| job.state.as_str()))
                .or_default() += 1;
        }
        let name = if cur.chain.loose {
            "independent jobs".to_string()
        } else {
            format!("chain {}", cur.chain.name)
        };
        let n = |state: &str| count.get(state).copied().unwrap_or(0);
        let head = header(
            &name,
            [n("done"), cur.order.len(), n("failed"), n("running")],
            cols,
        );
        debug_assert!(layout::width(&head) <= cols, "{head} is wider than {cols}");
        g.put(0, 0, &head, Tone::Plain);
        g.put(1, 0, &"─".repeat(cols), Tone::Line);
        /* which block of how many, and what runs in the others, so work out of sight is
        not taken for work that stopped */
        if blocks.len() > 1 {
            let elsewhere = blocks
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != b)
                .flat_map(|(_, block)| &block.order)
                .filter(|id| snap.job(id).is_some_and(|job| job.state == "running"))
                .count();
            let run = if elsewhere > 0 {
                format!(" +{elsewhere} run ·")
            } else {
                String::new()
            };
            let tag = format!(" {}/{} ", b + 1, blocks.len());
            let at = cols - layout::width(&run) - layout::width(&tag) - 1;
            g.put(1, at, &run, Tone::Run);
            g.put(1, at + layout::width(&run), &tag, Tone::Dim);
        }
        if let Some(error) = &self.error {
            let age = self
                .last_ok
                .map_or("--".to_string(), |t| format!("{}s", t.elapsed().as_secs()));
            g.put(
                2,
                0,
                &layout::fit(
                    &format!("! {error} · showing the snapshot from {age} ago"),
                    cols,
                ),
                Tone::Warn,
            );
        }
        let top = 3;
        let avail = rows - 1 - top;
        let view = cur.view(&snap, &sel);
        let body = if self.full {
            Grid::default()
        } else if cols >= 96 {
            layout::horizontal(&view, cols).unwrap_or_else(|| layout::vertical(&view, cols))
        } else {
            layout::vertical(&view, cols)
        };
        /* the full text: the full card scrolls to its end, and wrap keeps what it wrapped, so
        a long text is wrapped once, not on every frame */
        let detail = snap
            .detail
            .as_ref()
            .filter(|d| d.job_id == sel)
            .map(|d| (d.prompt.as_str(), d.result.as_deref()));
        let card = |fit| layout::card(&view, &sel, cols, detail, fit);
        /* the pane is filled: the flow takes its height, down to half the pane when the card
        needs more, and the card stretches over every row left, giving them to its result
        and prompt; what still does not fit is cut and counted */
        let body_h = if self.full { 0 } else { body.height() + 1 };
        let least = if self.full {
            0
        } else {
            card(Fit::Rows(0)).height()
        };
        let view_h = if body_h + least <= avail {
            body_h
        } else {
            body_h.min(avail.saturating_sub(least).max(avail / 2))
        };
        /* only the full card needs all of a long text laid out; the card under the flow
        lays out the rows it has */
        let card = match self.full {
            true => match card(Fit::Whole) {
                whole if whole.height() > avail => whole,
                _ => card(Fit::Rows(avail)),
            },
            false => card(Fit::Rows(avail - view_h)),
        };
        let card_h = card.height().min(avail - view_h);
        let node = cur.base + cur.num[&sel];
        let sel_row = body
            .rows
            .iter()
            .find(|(_, row)| row.values().any(|cell| cell.glyph && cell.node == node))
            .map_or(0, |(r, _)| *r);
        let scroll = sel_row.saturating_sub(view_h / 2).min(body_h - view_h);
        g.blit(&body, scroll..scroll + view_h, top);
        if self.full && card.height() > avail {
            /* the full card scrolls under its header, and its bottom edge says where */
            let rows = avail.saturating_sub(2).max(1);
            let lines = card.height() - 2;
            self.page = rows;
            self.scroll = self.scroll.min(lines.saturating_sub(rows));
            g.blit(&card, 0..1, top);
            g.blit(&card, 1 + self.scroll..1 + self.scroll + rows, top + 1);
            let at = format!(
                "└─ lines {}-{} of {lines} ",
                self.scroll + 1,
                (self.scroll + rows).min(lines)
            );
            let end = g.put(top + 1 + rows, 0, &layout::fit(&at, cols - 1), Tone::Line);
            g.put(
                top + 1 + rows,
                end,
                &format!("{}┘", "─".repeat(cols.saturating_sub(end + 1))),
                Tone::Line,
            );
        } else if card_h < card.height() && card_h > 0 {
            g.blit(&card, 0..card_h - 1, top + view_h);
            let more = format!("└─ {} more lines · enter ", card.height() - card_h + 1);
            let end = g.put(
                top + view_h + card_h - 1,
                0,
                &layout::fit(&more, cols - 1),
                Tone::Line,
            );
            g.put(
                top + view_h + card_h - 1,
                end,
                &format!("{}┘", "─".repeat(cols.saturating_sub(end + 1))),
                Tone::Line,
            );
        } else {
            g.blit(&card, 0..card_h, top + view_h);
        }
        let footer = match &self.input.mode {
            Mode::Jump(buf) => {
                let c = self.input.candidates(cur.order.len());
                format!(
                    "jump {buf}_ → {}  enter · esc",
                    c.iter().map(usize::to_string).collect::<Vec<_>>().join(" ")
                )
            }
            Mode::Hint => "hint: press the letter on a job · esc".into(),
            Mode::Search(q) => format!("/{q}_"),
            Mode::Normal if self.full && cols >= 96 => {
                "j/k ↑↓ scroll  space/b page  g/G top/end  h/l ←→ stage  esc back".into()
            }
            Mode::Normal if self.full => "j/k scroll  space/b page  h/l stage  esc back".into(),
            Mode::Normal if cols >= 96 => "j/k ↑↓ move  h/l ←→ stage  digits jump  f hints  / find  enter card  esc back  tab chain".into(),
            Mode::Normal => "j/k move  h/l stage  digits jump  f hint".into(),
        };
        g.put(rows - 1, 0, &layout::fit(&footer, cols), Tone::Dim);
        /* the spinner turns at 8 frames a second on a job its worker is busy with and holds
        its first frame when the pane is still; WAIT! blinks once a second */
        let ids: HashMap<usize, &String> = cur
            .order
            .iter()
            .map(|id| (cur.base + cur.num[id], id))
            .collect();
        /* a pane without focus keeps turning: in a split screen the monitor is the other pane */
        let moving = self.opts.anim;
        for row in g.rows.values_mut() {
            for cell in row.values_mut() {
                if cell.tone == Tone::Wait && moving && tick / 4 % 2 == 1 {
                    cell.ch = ' ';
                }
                if !cell.glyph || cell.node == 0 {
                    continue;
                }
                let Some(job) = ids.get(&cell.node).and_then(|id| snap.job(id)) else {
                    continue;
                };
                if job.state == "running" && snap.spinning(job) {
                    cell.ch = SPINNER[if moving { tick % SPINNER.len() } else { 0 }];
                }
            }
        }
        let jumping: Vec<usize> = self
            .input
            .candidates(cur.order.len())
            .iter()
            .map(|n| cur.base + n)
            .collect();
        if matches!(self.input.mode, Mode::Hint) {
            let mut seen: Vec<usize> = Vec::new();
            for row in g.rows.values() {
                for cell in row.values().filter(|cell| cell.glyph && cell.node > 0) {
                    if !seen.contains(&cell.node) {
                        seen.push(cell.node);
                    }
                }
            }
            seen.truncate(HINT_KEYS.len());
            self.hints = seen.iter().map(|node| ids[node].clone()).collect();
            for row in g.rows.values_mut() {
                for cell in row.values_mut().filter(|cell| cell.glyph) {
                    if let Some(k) = seen.iter().position(|node| *node == cell.node) {
                        cell.ch = HINT_KEYS.as_bytes()[k] as char;
                        cell.tone = Tone::Warn;
                    }
                }
            }
        }
        for row in g.rows.values_mut() {
            for cell in row
                .values_mut()
                .filter(|cell| !cell.glyph && jumping.contains(&cell.node))
            {
                cell.tone = Tone::Run;
            }
        }
        self.lines(&g, cols, rows)
    }

    fn lines(&self, g: &Grid, cols: usize, rows: usize) -> Vec<Line<'static>> {
        (0..rows.min(g.height()))
            .map(|r| self.row(&g.line(r, cols)))
            .collect()
    }

    fn row(&self, cells: &[Cell]) -> Line<'static> {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut run = String::new();
        let mut style = Style::default();
        for cell in cells.iter().filter(|cell| cell.ch != SPACER) {
            let s = self.style(cell.tone);
            if s != style && !run.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut run), style));
            }
            style = s;
            run.push(if self.opts.ascii {
                ascii(cell.ch)
            } else {
                cell.ch
            });
        }
        if !run.is_empty() {
            spans.push(Span::styled(run, style));
        }
        Line::from(spans)
    }

    fn style(&self, tone: Tone) -> Style {
        if !self.opts.color {
            return match tone {
                Tone::Select => Style::default().add_modifier(Modifier::REVERSED),
                Tone::Title => Style::default().add_modifier(Modifier::BOLD),
                _ => Style::default(),
            };
        }
        let t = &self.opts.theme;
        let fg = |c: Color| Style::default().fg(c);
        match tone {
            Tone::Plain => fg(t.text),
            Tone::Title => fg(t.title),
            Tone::Dim | Tone::Queued => fg(t.dim),
            Tone::Line => fg(t.line),
            Tone::Done => fg(t.done),
            Tone::Run => fg(t.run),
            Tone::Fail => fg(t.fail),
            Tone::Warn | Tone::Wait => fg(t.wait),
            Tone::Soft => fg(t.worker),
            Tone::Near => fg(t.near),
            Tone::Select => fg(t.select).bg(t.select_bg),
        }
    }
}

/* the counts always show: the name takes the width they leave, cut when it must and dropped
first; counts wider than the pane lose their words for the glyphs, not their numbers */
fn header(name: &str, [done, total, failed, running]: [usize; 4], cols: usize) -> String {
    let mut counts = format!(" · {done}/{total} done");
    let mut glyphs = format!("{done}/{total}●");
    for (k, label, glyph) in [(failed, "fail", '×'), (running, "run", '◆')] {
        if k > 0 {
            counts.push_str(&format!(" · {k} {label}"));
            glyphs.push_str(&format!(" {k}{glyph}"));
        }
    }
    let bare = counts.trim_start_matches(" · ");
    match cols.checked_sub(layout::width(&counts)) {
        Some(room) if layout::width(name) <= room => format!("{name}{counts}"),
        Some(room) if room >= 14 => format!("{}{counts}", layout::fit(name, room)),
        _ if layout::width(bare) <= cols => bare.to_string(),
        _ => layout::fit(&glyphs, cols),
    }
}

fn draw(frame: &mut Frame, app: &mut App, tick: usize) {
    let area = frame.area();
    let lines = app.screen(area.width as usize, area.height as usize, tick);
    frame.render_widget(Paragraph::new(lines), area);
}

fn ascii(ch: char) -> char {
    match ch {
        '─' => '-',
        '│' => '|',
        '┌' | '┐' | '└' | '┘' | '┬' | '┴' | '┼' | '├' | '┤' => '+',
        '◀' | '←' => '<',
        '●' => '*',
        '◆' => '>',
        '⣿' => '#',
        '↓' => 'v',
        '×' => 'x',
        '○' => 'o',
        '–' => '-',
        '·' => '|',
        '…' => '~',
        '↑' => '^',
        '→' => '>',
        '⠋' | '⠼' | '⠇' => '|',
        '⠙' | '⠴' | '⠏' => '/',
        '⠹' | '⠦' => '-',
        '⠸' | '⠧' => '\\',
        other => other,
    }
}

fn parse(args: &[String]) -> Result<Opts> {
    let mut opts = Opts {
        circle: None,
        ascii: false,
        color: std::env::var_os("NO_COLOR").is_none(),
        anim: true,
        theme: Theme::default(),
    };
    let mut all = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--circle" => opts.circle = Some(it.next().ok_or("--circle needs a name")?.clone()),
            "--all" => all = true,
            "--ascii" => opts.ascii = true,
            "--no-color" => opts.color = false,
            "--no-anim" => opts.anim = false,
            other => return Err(format!("unknown tui option {other}").into()),
        }
    }
    if opts.circle.is_none() && !all {
        opts.circle = Some(crate::cli::project_circle(&std::env::current_dir()?));
    }
    /* the theme lives in the amesh directory beside the hub's config.toml */
    let saved = crate::cli::state_file().with_file_name("tui-theme.json");
    if saved.exists() {
        opts.theme = Theme::load(&saved)?;
    }
    if !std::env::var("COLORTERM").is_ok_and(|v| v == "truecolor" || v == "24bit") {
        opts.theme = opts.theme.indexed();
    }
    Ok(opts)
}

/* a request a second on a background thread, so the screen never waits for the network;
a poke (r, or a new selection) asks sooner, at most five times a second */
fn fetch(
    circle: Option<String>,
    want: Arc<Mutex<Option<String>>>,
    tx: mpsc::Sender<std::result::Result<Snapshot, String>>,
    poke: mpsc::Receiver<()>,
) {
    std::thread::spawn(move || loop {
        let started = Instant::now();
        let mut query: Vec<String> = Vec::new();
        if let Some(c) = &circle {
            query.push(format!("circle={c}"));
        }
        if let Some(d) = want.lock().ok().and_then(|w| w.clone()) {
            query.push(format!("detail={d}"));
        }
        let path = if query.is_empty() {
            "/snapshot".to_string()
        } else {
            format!("/snapshot?{}", query.join("&"))
        };
        let got = crate::cli::request("GET", &path, None)
            .map_err(|e| e.to_string())
            .and_then(|v| serde_json::from_value::<Snapshot>(v).map_err(|e| e.to_string()));
        if tx.send(got).is_err() {
            return;
        }
        let _ = poke.recv_timeout(Duration::from_secs(1));
        /* a held key pokes on every step; five fetches a second follow it well enough */
        std::thread::sleep(Duration::from_millis(200).saturating_sub(started.elapsed()));
        while poke.try_recv().is_ok() {}
    });
}

pub(crate) fn run(args: &[String]) -> Result<()> {
    let opts = parse(args)?;
    let want = Arc::new(Mutex::new(None));
    let (tx, rx) = mpsc::channel();
    let (poke_tx, poke_rx) = mpsc::channel();
    fetch(opts.circle.clone(), want.clone(), tx, poke_rx);
    let stop = Arc::new(AtomicBool::new(false));
    stop_on_signals(&stop);
    exit_on_hangup();
    let mut app = App::new(opts);
    let mut terminal = ratatui::try_init()?;
    let mut tick = 0usize;
    let mut asked: Option<String> = None;
    let result = (|| -> Result<()> {
        while !stop.load(Ordering::Relaxed) {
            while let Ok(got) = rx.try_recv() {
                app.apply(got);
            }
            /* a new selection fetches its full text now instead of on the next second */
            if app.sel != asked {
                asked = app.sel.clone();
                if let Ok(mut w) = want.lock() {
                    *w = asked.clone();
                }
                let _ = poke_tx.send(());
            }
            terminal.draw(|frame| draw(frame, &mut app, tick))?;
            if !event::poll(Duration::from_millis(125))? {
                tick = tick.wrapping_add(1);
                continue;
            }
            let key = match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => key,
                _ => continue,
            };
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Ok(());
            }
            match app.key(key.code) {
                Act::Quit => return Ok(()),
                Act::Refresh => {
                    let _ = poke_tx.send(());
                }
                _ => {}
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}

/* crossterm's reader never leaves event::poll once the terminal hangs up: its read loop
stops only on WouldBlock, and a closed pane reads EOF forever. A closed pty shows POLLHUP
on stdin, a tty revoked with its session POLLNVAL. The restore is for form: its writes go
to a terminal that is gone */
fn exit_on_hangup() {
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_millis(250));
        let mut stdin = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut stdin, 1, 0) };
        if ready > 0 && stdin.revents & (libc::POLLHUP | libc::POLLNVAL | libc::POLLERR) != 0 {
            ratatui::restore();
            std::process::exit(0);
        }
    });
}

/* SIGTERM, SIGHUP and SIGINT end the loop the way q does, so a killed or closed pane
still gets its terminal back */
fn stop_on_signals(stop: &Arc<AtomicBool>) {
    use tokio::signal::unix::{signal, SignalKind};
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let _context = runtime.enter();
    for kind in [
        SignalKind::terminate(),
        SignalKind::hangup(),
        SignalKind::interrupt(),
    ] {
        let Ok(mut signals) = signal(kind) else {
            continue;
        };
        let stop = stop.clone();
        runtime.spawn(async move {
            signals.recv().await;
            stop.store(true, Ordering::Relaxed);
        });
    }
}
