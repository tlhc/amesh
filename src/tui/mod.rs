mod asks;
mod feed;
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
/* the most lines the online peers take above the view */
const PEER_ROWS: usize = 4;
/* snapshots the peer rows are kept after the roster stopped needing them */
const PEER_HOLD: u64 = 5;
/* rule characters that stay to the left of what the rule row says at its right end */
const RULE_MIN: usize = 4;

pub(crate) struct App {
    opts: Opts,
    snap: Option<model::Frame>,
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
    copied: Option<(Instant, String)>,
    /* the full-screen card's first shown line, and how many lines a page is */
    scroll: usize,
    page: usize,
    /* snapshots taken in, and the peer rows the view keeps with the snapshot that last
    needed them: the view moves down at once and back up only once the roster held still */
    snaps: u64,
    held: (usize, u64),
    /* the asks screen is up instead of the jobs; the ask selected there, and the order its
    numbers were read against */
    asks: bool,
    ask_sel: Option<String>,
    ask_order: Vec<String>,
    /* the events screen is up instead of the jobs or the asks; the events it holds, the seq
    of the one selected (None follows the newest), the last read's error, and the hub process
    the snapshots came from */
    events: bool,
    /* the jobs-screen body is the selected chain's timeline instead of the flow */
    timeline: bool,
    feed: Vec<feed::Held>,
    ev_sel: Option<u64>,
    ev_error: Option<String>,
    epoch: String,
}

/* a chain or the loose block, numbered 1..n; base counts the jobs in the blocks above */
struct Block {
    chain: Chain,
    order: Vec<String>,
    num: HashMap<String, usize>,
    base: usize,
}

impl Block {
    fn view<'a>(&'a self, snap: model::Frame, sel: &'a str) -> View<'a> {
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
    fn copy_command(&mut self, index: usize, tmux: bool, out: &mut impl std::io::Write) {
        if self.events || self.asks {
            return;
        }
        let Some(job) = self.snap.as_ref().and_then(|s| s.job(self.sel.as_deref()?)) else {
            return;
        };
        let Some((_, command)) = layout::job_commands(job).into_iter().nth(index) else {
            return;
        };
        let notice = match send_clipboard(&command, tmux, out) {
            Ok(()) => format!(
                "{}sent to clipboard (OSC 52): {command}",
                if tmux {
                    "tmux: set-clipboard on · "
                } else {
                    ""
                }
            ),
            Err(error) => format!("copy failed: {error}"),
        };
        self.copied = Some((Instant::now(), notice));
    }

    fn copy_notice(&self) -> Option<String> {
        self.copied
            .as_ref()
            .filter(|(at, _)| at.elapsed() < Duration::from_secs(2))
            .map(|(_, notice)| notice.clone())
    }

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
            copied: None,
            scroll: 0,
            page: 10,
            snaps: 0,
            held: (0, 0),
            asks: false,
            ask_sel: None,
            ask_order: Vec::new(),
            events: false,
            timeline: false,
            feed: Vec::new(),
            ev_sel: None,
            ev_error: None,
            epoch: String::new(),
        }
    }

    #[cfg(test)]
    pub fn apply(&mut self, fetched: std::result::Result<Snapshot, String>) {
        self.receive(fetched.map(|snap| {
            let mode = match snap.schema_version {
                v if v > crate::wire::SNAPSHOT_VERSION => model::HubMode::Unsupported(v),
                crate::wire::SNAPSHOT_VERSION => model::HubMode::V2,
                _ => model::HubMode::Neutral,
            };
            (mode, snap)
        }));
    }

    /* an error keeps the last frame */
    pub fn receive(&mut self, got: std::result::Result<(model::HubMode, Snapshot), String>) {
        match got {
            Ok((mode, view)) => {
                /* a newer hub: one notice, nothing from the last frame */
                if matches!(mode, model::HubMode::Unsupported(_)) {
                    self.snap = None;
                    self.error = Some("unsupported hub".into());
                    return;
                }
                let frame = model::Frame { view, fresh: false };
                self.snaps += 1;
                self.mark_restart(&frame);
                self.snap = Some(frame);
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

    /* another hub_epoch means the hub restarted and its ring started over: while any event is
    held, a divider goes after what is held, one for each restart */
    fn mark_restart(&mut self, snap: &model::Frame) {
        if snap.hub_epoch.is_empty() {
            return;
        }
        let before = std::mem::replace(&mut self.epoch, snap.hub_epoch.clone());
        if !before.is_empty()
            && before != snap.hub_epoch
            && self.feed.iter().any(|h| matches!(h, feed::Held::Event(_)))
        {
            self.feed.push(feed::Held::Restart(snap.captured_at));
        }
    }

    /* events read from the hub: those newer than any held since the last restart, or the whole
    ring from a hub that numbers none; the oldest go past feed::KEEP */
    pub fn take_events(&mut self, got: std::result::Result<Vec<model::Event>, String>) {
        let list = match got {
            Ok(list) => list,
            Err(error) => {
                self.ev_error = Some(error);
                return;
            }
        };
        self.ev_error = None;
        let list: Vec<model::Event> = list
            .into_iter()
            .filter(|e| e.kind != "chat_turn_delta")
            .collect();
        /* a hub that numbers no events sends its whole ring on every read, an empty one too */
        let whole = list.iter().any(|e| e.seq.is_none())
            || (list.is_empty() && feed::unnumbered(&self.feed));
        if !whole {
            let newest = feed::newest(&self.feed);
            self.feed.extend(
                list.into_iter()
                    .filter(|e| e.seq > newest)
                    .map(|e| feed::Held::Event(Box::new(e))),
            );
        } else {
            /* the ring read whole stands for what came after the last restart only */
            let kept = self
                .feed
                .iter()
                .rposition(|h| matches!(h, feed::Held::Restart(_)))
                .map_or(0, |at| at + 1);
            self.feed.truncate(kept);
            self.feed
                .extend(list.into_iter().map(|e| feed::Held::Event(Box::new(e))));
        }
        feed::trim(&mut self.feed, feed::KEEP);
        if self.ev_sel.is_some_and(|seq| !feed::holds(&self.feed, seq)) {
            self.ev_sel = None;
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
        if self.events {
            return self.event_key(code);
        }
        if self.asks {
            return self.ask_key(code);
        }
        let blocks = self.blocks();
        /* digits typed against numbers that have since changed would land on another job */
        if matches!(self.input.mode, Mode::Jump(_)) && self.numbers.renumbered != self.typed_at {
            self.input.mode = Mode::Normal;
            self.typed_at = self.numbers.renumbered;
            return Act::None;
        }
        self.typed_at = self.numbers.renumbered;
        let Some((b, p)) = self.settle(&blocks) else {
            let act = self.input.key(code, 0);
            match act {
                Act::Asks => self.flip(),
                Act::Events => self.open_events(),
                _ => {}
            }
            return act;
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
            Act::Asks => self.flip(),
            Act::Events => self.open_events(),
            Act::Timeline => self.timeline = !self.timeline,
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

    /* the asks screen and back; a full card closes, and the asks screen keeps the ask it had
    selected while that ask is still listed */
    fn flip(&mut self) {
        self.asks = !self.asks;
        self.full = false;
        self.scroll = 0;
        if let (true, Some(snap)) = (self.asks, &self.snap) {
            let order = asks::order(snap);
            if !order
                .iter()
                .any(|ask| Some(&ask.correlation_id) == self.ask_sel.as_ref())
            {
                self.ask_sel = order.first().map(|ask| ask.correlation_id.clone());
            }
        }
    }

    fn ask_key(&mut self, code: KeyCode) -> Act {
        let order: Vec<String> = self.snap.as_ref().map_or(Vec::new(), |snap| {
            asks::order(snap)
                .iter()
                .map(|ask| ask.correlation_id.clone())
                .collect()
        });
        /* digits typed against an order that has since changed would land on another ask */
        if matches!(self.input.mode, Mode::Jump(_)) && order != self.ask_order {
            self.input.mode = Mode::Normal;
            self.ask_order = order;
            return Act::None;
        }
        self.ask_order = order.clone();
        let act = self.input.key(code, order.len());
        /* the asks carry no letter hints */
        if matches!(self.input.mode, Mode::Hint) {
            self.input.mode = Mode::Normal;
        }
        let at = self
            .ask_sel
            .as_ref()
            .and_then(|id| order.iter().position(|x| x == id))
            .unwrap_or(0);
        let before = self.ask_sel.clone();
        match act {
            Act::Asks => self.flip(),
            Act::Events => self.open_events(),
            Act::Move(d) if self.full => {
                self.scroll = self.scroll.saturating_add_signed(d as isize);
            }
            Act::Page(d) if self.full => {
                self.scroll = self
                    .scroll
                    .saturating_add_signed(d as isize * self.page as isize);
            }
            Act::Edge(d) if self.full => self.scroll = if d < 0 { 0 } else { usize::MAX },
            Act::Card(open) => {
                self.full = open;
                self.scroll = 0;
            }
            _ if order.is_empty() => {}
            Act::Pick(num) => self.ask_sel = order.get(num - 1).cloned().or(before.clone()),
            Act::Move(d) => {
                let n = order.len() as i32;
                self.ask_sel = Some(order[(at as i32 + d).rem_euclid(n) as usize].clone());
            }
            Act::Find(d) => {
                let query = self.input.query.to_lowercase();
                let snap = self.snap.as_ref();
                let hit = |id: &String| {
                    snap.and_then(|s| s.asks.iter().find(|a| &a.correlation_id == id))
                        .is_some_and(|a| {
                            [&a.from_peer, &a.to_peer_id, &a.text]
                                .iter()
                                .any(|t| t.to_lowercase().contains(&query))
                        })
                };
                let n = order.len() as i32;
                let (start, step) = if d == 0 {
                    (at as i32, 1)
                } else {
                    (at as i32 + d, d.signum())
                };
                if let Some(id) = (0..n)
                    .map(|k| &order[(start + k * step).rem_euclid(n) as usize])
                    .find(|id| hit(id))
                {
                    self.ask_sel = Some(id.clone());
                }
            }
            _ => {}
        }
        if self.ask_sel != before {
            self.scroll = 0;
        }
        act
    }

    /* the events screen over the jobs or the asks, following the newest */
    fn open_events(&mut self) {
        self.events = true;
        self.ev_sel = None;
        self.full = false;
        self.scroll = 0;
    }

    /* the events screen's keys: j/k move toward newer and older, the newest row following the
    newest again; e goes to the jobs, a to the asks */
    fn event_key(&mut self, code: KeyCode) -> Act {
        let seqs: Vec<u64> = feed::items(&self.feed)
            .iter()
            .filter_map(|item| item.seq())
            .collect();
        let act = self.input.key(code, 0);
        /* the events carry no numbers, letters or search */
        self.input.mode = Mode::Normal;
        let at = self
            .ev_sel
            .and_then(|seq| seqs.iter().position(|s| *s == seq))
            .unwrap_or(seqs.len().saturating_sub(1));
        let before = self.ev_sel;
        match act {
            Act::Events => {
                self.events = false;
                self.asks = false;
                self.full = false;
                self.scroll = 0;
            }
            Act::Asks => {
                self.events = false;
                self.full = false;
                self.scroll = 0;
                if !self.asks {
                    self.flip();
                }
            }
            Act::Move(d) if self.full => {
                self.scroll = self.scroll.saturating_add_signed(d as isize);
            }
            Act::Page(d) if self.full => {
                self.scroll = self
                    .scroll
                    .saturating_add_signed(d as isize * self.page as isize);
            }
            Act::Edge(d) if self.full => self.scroll = if d < 0 { 0 } else { usize::MAX },
            Act::Card(open) => {
                self.full = open;
                self.scroll = 0;
            }
            _ if seqs.is_empty() => {}
            Act::Move(d) => {
                let next = (at as i32 + d).clamp(0, seqs.len() as i32 - 1) as usize;
                self.ev_sel = (next + 1 < seqs.len()).then(|| seqs[next]);
            }
            _ => {}
        }
        if self.ev_sel != before {
            self.scroll = 0;
        }
        act
    }

    /* the design's screen: the selected job's chain alone under its header, the flow, then
    the card; Tab moves to the next chain */
    pub fn screen(&mut self, cols: usize, rows: usize, tick: usize) -> Vec<Line<'static>> {
        let fresh = self.fresh();
        if let Some(frame) = self.snap.as_mut() {
            frame.fresh = fresh;
        }
        let mut g = Grid::default();
        let blocks = self.blocks();
        /* settling picks a job and starts its card at the top; the asks screen keeps its own
        place in the card it shows */
        let settled = if self.asks || self.events {
            None
        } else {
            self.settle(&blocks)
        };
        if cols < 30 || rows < 8 {
            g.put(0, 0, "pane too small", Tone::Warn);
            return self.lines(&g, cols, rows);
        }
        /* who is online, above whatever the view shows; the view starts below its last line
        and keeps at least three rows */
        let mut peer_rows = 1;
        if let Some(snap) = &self.snap {
            let moving = self.moving();
            let spin = SPINNER[if moving { tick % SPINNER.len() } else { 0 }];
            let lit = !(moving && tick / 4 % 2 == 1);
            let cap = PEER_ROWS.min(rows - 7);
            let lines = layout::presence_lines(snap, cols, cap, spin, lit);
            for (r, line) in lines.iter().enumerate() {
                let mut c = 0;
                for (text, tone) in line {
                    c = g.put(r, c, text, *tone);
                }
            }
            if lines.len() >= self.held.0 || self.snaps >= self.held.1 + PEER_HOLD {
                self.held = (lines.len(), self.snaps);
            }
            peer_rows = self.held.0.min(cap);
        }
        if self.events {
            return self.events_screen(g, peer_rows, cols, rows);
        }
        if self.asks {
            return self.asks_screen(g, peer_rows, cols, rows, tick);
        }
        let Some((b, _)) = settled else {
            let stale;
            let (text, hint) = match (&self.error, &self.snap) {
                (Some(error), Some(_)) => {
                    stale = self.stale(error);
                    (stale.as_str(), "")
                }
                (Some(error), None) => (error.as_str(), ""),
                (None, Some(_)) => (
                    "no jobs here yet",
                    "jobs appear once created: amesh jobs create TITLE --assigned-peer ID",
                ),
                (None, None) => ("waiting for the hub", ""),
            };
            let lines = layout::wrap(text, cols);
            for (r, line) in lines.iter().enumerate() {
                g.put(peer_rows + r, 0, line, Tone::Warn);
            }
            /* the open asks and what the hub keeps, the events the first to go */
            if let Some(snap) = &self.snap {
                let tags = [
                    asks::tag(snap).map(|t| (t, Tone::Near)),
                    layout::events_tag(snap).map(|t| (t, Tone::Dim)),
                ];
                let last = lines.len().saturating_sub(1);
                let taken = lines.last().map_or(0, |line| layout::width(line));
                self.message_end(
                    &mut g,
                    peer_rows + last,
                    taken,
                    cols,
                    tags.into_iter().flatten(),
                );
            }
            let below = peer_rows
                + lines.len()
                + usize::from(self.under_rule(&mut g, peer_rows + lines.len(), cols, None));
            let hints = layout::wrap(hint, cols);
            for (r, line) in hints.iter().enumerate() {
                g.put(below + r, 0, line, Tone::Dim);
            }
            if !hint.is_empty() && self.snap.as_ref().is_some_and(|snap| asks::held(snap)) {
                g.put(
                    below + hints.len(),
                    0,
                    &layout::fit("asks have a screen of their own: press a", cols),
                    Tone::Near,
                );
            }
            return self.lines(&g, cols, rows);
        };
        let snap = self.snap.clone().expect("blocks come from a snapshot");
        let sel = self.sel.clone().expect("settle picked a job");
        let cur = &blocks[b];
        /* one row per block above the rule, leaving the view at least three rows */
        let rows_free = rows.saturating_sub(peer_rows + 6).clamp(1, 4);
        let chains = strip(&blocks, b, &snap, cols, rows_free);
        for (r, row) in chains.iter().enumerate() {
            let mut c = 0;
            for (text, tone) in row {
                c = g.put(peer_rows + r, c, text, *tone);
            }
        }
        let rule = peer_rows + chains.len();
        g.put(rule, 0, &"─".repeat(cols), Tone::Line);
        let mut right: Vec<(String, Tone)> = Vec::new();
        /* the open asks and what the hub keeps, both as old as the snapshot while
        an error shows; the events are the first to go when the pane is narrow, the asks next */
        let tags: Vec<(String, Tone)> = [
            asks::tag(&snap).map(|t| (t, Tone::Near)),
            layout::events_tag(&snap).map(|t| (t, Tone::Dim)),
        ]
        .into_iter()
        .flatten()
        .filter(|_| self.error.is_none())
        .collect();
        let used = right
            .iter()
            .map(|(text, _)| layout::width(text))
            .sum::<usize>()
            + self.mark_room();
        for keep in (1..=tags.len()).rev() {
            let pieces: Vec<(String, Tone)> = tags[..keep]
                .iter()
                .enumerate()
                .map(|(i, (t, tone))| match i + 1 == keep && right.is_empty() {
                    true => (format!(" {t} "), *tone),
                    false => (format!(" {t} ·"), *tone),
                })
                .collect();
            let w: usize = pieces.iter().map(|(text, _)| layout::width(text)).sum();
            if used + w + 1 + RULE_MIN <= cols {
                right.splice(0..0, pieces);
                break;
            }
        }
        self.rule_end(&mut g, rule, cols, right);
        self.under_rule(&mut g, rule + 1, cols, self.error_lead());
        let top = rule + 2;
        let avail = rows - 1 - top;
        let view = cur.view(snap, &sel);
        let snap = &view.snap;
        let body = if self.full {
            Grid::default()
        } else if self.timeline {
            layout::timeline(&view, cols)
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
            true => card(Fit::Whole),
            false => card(Fit::Rows(avail - view_h)),
        };
        let node = cur.base + cur.num[&sel];
        let sel_row = body
            .rows
            .iter()
            .find(|(_, row)| row.values().any(|cell| cell.node == node))
            .map_or(0, |(r, _)| *r);
        let scroll = sel_row.saturating_sub(view_h / 2).min(body_h - view_h);
        g.blit(&body, scroll..scroll + view_h, top);
        self.place(&mut g, &card, top, view_h, avail, cols);
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
            Mode::Normal => job_keys(
                cols,
                self.timeline,
                snap.job(&sel)
                    .is_some_and(|job| !layout::job_commands(job).is_empty()),
                asks::held(snap),
            ),
        };
        let footer = if self.input.mode == Mode::Normal {
            self.copy_notice().unwrap_or(footer)
        } else {
            footer
        };
        g.put(rows - 1, 0, &layout::fit(&footer, cols), Tone::Dim);
        /* the spinner turns at 8 frames a second on a job its worker is busy with and holds
        its first frame when the pane is still */
        let ids: HashMap<usize, &String> = cur
            .order
            .iter()
            .map(|id| (cur.base + cur.num[id], id))
            .collect();
        /* a pane without focus keeps turning: in a split screen the monitor is the other pane */
        let moving = self.moving();
        let mut sent = Vec::new();
        for (r, row) in g.rows.iter_mut() {
            for (c, cell) in row.iter_mut() {
                if !cell.glyph || cell.node == 0 {
                    continue;
                }
                let Some(job) = ids.get(&cell.node).and_then(|id| snap.job(id)) else {
                    continue;
                };
                if job.state == "running"
                    && job.progress.as_ref().is_some_and(|p| p.busy)
                    && !matches!(job.relation, Some(crate::wire::JobRelation::Missing { .. }))
                {
                    cell.ch = SPINNER[if moving { tick % SPINNER.len() } else { 0 }];
                }
                if moving && job.state == "running" && just_sent(snap, job) {
                    sent.push((*r, *c));
                }
            }
        }
        walk(&mut g, tick, moving);
        /* a job the hub sent within the last second: a dot runs along the rail into it, then
        its glyph lights */
        for (r, c) in sent {
            let path = rail_into(&g, r, c);
            let (at, ch) = match path.get(tick / 2 % (path.len() + 1)) {
                Some(&at) => (at, '•'),
                None => ((r, c), '◉'),
            };
            if let Some(cell) = g.rows.get_mut(&at.0).and_then(|row| row.get_mut(&at.1)) {
                cell.ch = ch;
                cell.tone = Tone::Near;
            }
        }
        let jumping: Vec<usize> = self
            .input
            .candidates(cur.order.len())
            .iter()
            .map(|n| cur.base + n)
            .collect();
        if matches!(self.input.mode, Mode::Hint) {
            /* a job's letter takes its glyph; in a view without glyphs, the timeline, it takes
            the first cell that carries the job */
            let mut seen: Vec<(usize, (usize, usize))> = Vec::new();
            for (r, row) in g.rows.iter() {
                for (c, cell) in row.iter() {
                    if cell.glyph != self.timeline
                        && cell.node > 0
                        && !seen.iter().any(|(node, _)| *node == cell.node)
                    {
                        seen.push((cell.node, (*r, *c)));
                    }
                }
            }
            seen.truncate(HINT_KEYS.len());
            self.hints = seen.iter().map(|(node, _)| ids[node].clone()).collect();
            for (k, (_, (r, c))) in seen.iter().enumerate() {
                if let Some(cell) = g.rows.get_mut(r).and_then(|row| row.get_mut(c)) {
                    cell.ch = HINT_KEYS.as_bytes()[k] as char;
                    cell.tone = Tone::Warn;
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

    /* the card under the view: the full card scrolls under its header, its bottom edge saying
    where; a card cut short says how many lines it lost */
    fn place(
        &mut self,
        g: &mut Grid,
        card: &Grid,
        top: usize,
        view_h: usize,
        avail: usize,
        cols: usize,
    ) {
        let card_h = card.height().min(avail - view_h);
        if self.full && card.height() > avail {
            /* the full card scrolls under its header, and its bottom edge says where */
            let rows = avail.saturating_sub(2).max(1);
            let lines = card.height() - 2;
            self.page = rows;
            self.scroll = self.scroll.min(lines.saturating_sub(rows));
            g.blit(card, 0..1, top);
            g.blit(card, 1 + self.scroll..1 + self.scroll + rows, top + 1);
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
            g.blit(card, 0..card_h - 1, top + view_h);
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
            g.blit(card, 0..card_h, top + view_h);
        }
    }

    /* the asks no job points at under their header and the rule, the list, then the selected
    ask's card; the list and the card share the pane the way the flow and a job's card do */
    fn asks_screen(
        &mut self,
        mut g: Grid,
        peer_rows: usize,
        cols: usize,
        rows: usize,
        tick: usize,
    ) -> Vec<Line<'static>> {
        let empty = |snap: &Snapshot| asks::order(snap).is_empty();
        let say = match (&self.error, &self.snap) {
            (Some(error), None) => Some((error.clone(), "")),
            (None, None) => Some(("waiting for the hub".to_string(), "")),
            (_, Some(snap)) if !snap.capabilities.ask_list => Some((
                "this hub lists only the asks of jobs; restart it on the current amesh".to_string(),
                "a: back to the jobs",
            )),
            (Some(error), Some(snap)) if empty(snap) => Some((self.stale(error), "")),
            (None, Some(snap)) if empty(snap) => Some((
                "no asks here yet".to_string(),
                "asks appear when a peer calls amesh_ask; a: back to the jobs",
            )),
            _ => None,
        };
        if let Some((text, hint)) = say {
            let lines = layout::wrap(&text, cols);
            for (r, line) in lines.iter().enumerate() {
                g.put(peer_rows + r, 0, line, Tone::Warn);
            }
            if let Some(snap) = &self.snap {
                let last = lines.len().saturating_sub(1);
                let taken = lines.last().map_or(0, |line| layout::width(line));
                let events = layout::events_tag(snap).map(|t| (t, Tone::Dim));
                self.message_end(&mut g, peer_rows + last, taken, cols, events);
            }
            let below = peer_rows
                + lines.len()
                + usize::from(self.under_rule(&mut g, peer_rows + lines.len(), cols, None));
            for (r, line) in layout::wrap(hint, cols).iter().enumerate() {
                g.put(below + r, 0, line, Tone::Dim);
            }
            return self.lines(&g, cols, rows);
        }
        let snap = self.snap.clone().expect("checked above");
        let order = asks::order(&snap);
        let at = match self
            .ask_sel
            .as_deref()
            .and_then(|id| order.iter().position(|ask| ask.correlation_id == id))
        {
            Some(at) => at,
            None => {
                /* the selected ask is gone: a half-typed jump meant a number of the old order */
                if matches!(self.input.mode, Mode::Jump(_)) {
                    self.input.mode = Mode::Normal;
                }
                self.scroll = 0;
                self.ask_sel = Some(order[0].correlation_id.clone());
                0
            }
        };
        let ask = order[at];
        let head = asks::header(&order, self.opts.circle.is_none());
        g.put(peer_rows, 0, &layout::fit(&head, cols), Tone::Plain);
        g.put(peer_rows + 1, 0, &"─".repeat(cols), Tone::Line);
        let mut right = Vec::new();
        if let Some(events) = layout::events_tag(&snap).filter(|_| self.error.is_none()) {
            let piece = format!(" {events} ");
            if layout::width(&piece) + self.mark_room() + 1 + RULE_MIN <= cols {
                right.push((piece, Tone::Dim));
            }
        }
        self.rule_end(&mut g, peer_rows + 1, cols, right);
        self.under_rule(&mut g, peer_rows + 2, cols, self.error_lead());
        let top = peer_rows + 3;
        let avail = rows - 1 - top;
        let list = asks::rows(&snap, &order, cols);
        let detail = snap
            .ask_detail
            .as_ref()
            .filter(|d| d.correlation_id == ask.correlation_id)
            .map(|d| (d.text.as_str(), d.reply.as_deref()));
        let card = |fit| asks::card(&snap, ask, at + 1, cols, detail, fit);
        let list_h = if self.full { 0 } else { list.len() + 1 };
        let least = if self.full {
            0
        } else {
            card(Fit::Rows(0)).height()
        };
        let view_h = if list_h + least <= avail {
            list_h
        } else {
            list_h.min(avail.saturating_sub(least).max(avail / 2))
        };
        let card = match self.full {
            true => card(Fit::Whole),
            false => card(Fit::Rows(avail - view_h)),
        };
        /* the list scrolls to keep the selected ask in view; the blank row under it is the
        first to give way when the card needs the room */
        let shown = view_h.min(list.len());
        let first = at
            .saturating_sub(shown / 2)
            .min(list.len().saturating_sub(shown));
        for (i, row) in list.iter().skip(first).take(shown).enumerate() {
            let mut c = 0;
            for (text, tone) in row {
                c = g.put(top + i, c, text, *tone);
            }
        }
        self.place(&mut g, &card, top, view_h, avail, cols);
        let footer = match (&self.input.mode, self.full) {
            (Mode::Jump(buf), _) => {
                let c = self.input.candidates(order.len());
                format!(
                    "jump {buf}_ → {}  enter · esc",
                    c.iter().map(usize::to_string).collect::<Vec<_>>().join(" ")
                )
            }
            (Mode::Search(q), _) => format!("/{q}_"),
            (_, true) if cols >= 96 => "j/k ↑↓ scroll  space/b page  g/G top/end  esc back".into(),
            (_, true) => "j/k scroll  space/b page  esc back".into(),
            _ if cols >= 96 => {
                "j/k ↑↓ move  digits jump  / find  enter card  esc back  a jobs".into()
            }
            _ => "j/k move  1-9 jump  enter card  a jobs".into(),
        };
        g.put(rows - 1, 0, &layout::fit(&footer, cols), Tone::Dim);
        walk(&mut g, tick, self.moving());
        let mut lines = self.lines(&g, cols, rows);
        if !self.full && (first..first + shown).contains(&at) {
            if let Some(line) = lines.get_mut(top + at - first) {
                for span in &mut line.spans {
                    span.style = match self.opts.color {
                        true => span.style.bg(self.opts.theme.select_bg),
                        false => span.style.add_modifier(Modifier::REVERSED),
                    };
                }
            }
        }
        lines
    }

    /* the hub's events under their header and the rule, the newest at the bottom, then the
    selected one's card; the list and the card share the pane as on the asks screen */
    fn events_screen(
        &mut self,
        mut g: Grid,
        peer_rows: usize,
        cols: usize,
        rows: usize,
    ) -> Vec<Line<'static>> {
        let head = feed::header(&self.feed, self.opts.circle.is_none());
        g.put(peer_rows, 0, &layout::fit(&head, cols), Tone::Plain);
        g.put(peer_rows + 1, 0, &"─".repeat(cols), Tone::Line);
        let mut right = Vec::new();
        if let Some(tag) = self
            .snap
            .as_ref()
            .and_then(|snap| asks::tag(snap))
            .filter(|_| self.error.is_none())
        {
            let piece = format!(" {tag} ");
            if layout::width(&piece) + self.mark_room() + 1 + RULE_MIN <= cols {
                right.push((piece, Tone::Near));
            }
        }
        self.rule_end(&mut g, peer_rows + 1, cols, right);
        /* a hub that numbers no events cannot tell one from the next, so the selection stays
        on the newest; the screen says why */
        let problem = match (&self.error, &self.ev_error) {
            (Some(_), _) => self.error_lead(),
            (None, Some(error)) => Some((format!("! events: {error}"), Tone::Warn)),
            (None, None) if feed::unnumbered(&self.feed) => Some((
                "this hub numbers no events, so only the newest opens; restart it on the current amesh"
                    .to_string(),
                Tone::Dim,
            )),
            (None, None) => None,
        };
        self.under_rule(&mut g, peer_rows + 2, cols, problem);
        let footer = match self.full {
            true if cols >= 96 => "j/k ↑↓ scroll  space/b page  g/G top/end  esc back",
            true => "j/k scroll  space/b page  esc back",
            false if cols >= 96 => "j/k ↑↓ move  enter card  esc back  e jobs  a asks",
            false => "j/k move  enter card  e jobs  a asks",
        };
        g.put(rows - 1, 0, &layout::fit(footer, cols), Tone::Dim);
        let top = peer_rows + 3;
        let avail = rows - 1 - top;
        let items = feed::items(&self.feed);
        if items.is_empty() {
            g.put(top, 0, &layout::fit("no events yet", cols), Tone::Warn);
            let hint = "events appear as peers ask, ack, notify and broadcast; e: back to the jobs";
            for (r, line) in layout::wrap(hint, cols).iter().enumerate() {
                g.put(top + 1 + r, 0, line, Tone::Dim);
            }
            return self.lines(&g, cols, rows);
        }
        let at = self
            .ev_sel
            .and_then(|seq| items.iter().position(|item| item.seq() == Some(seq)))
            .unwrap_or(items.len() - 1);
        let prefix = self.snap.as_ref().map_or("", |snap| layout::prefix(snap));
        let list = feed::rows(&items, cols, prefix);
        let card = |fit| feed::card(&items[at], cols, fit, prefix);
        let list_h = if self.full { 0 } else { list.len() + 1 };
        let least = if self.full {
            0
        } else {
            card(Fit::Rows(0)).height()
        };
        let view_h = if list_h + least <= avail {
            list_h
        } else {
            list_h.min(avail.saturating_sub(least).max(avail / 2))
        };
        let card = match self.full {
            true => card(Fit::Whole),
            false => card(Fit::Rows(avail - view_h)),
        };
        let shown = view_h.min(list.len());
        let first = at
            .saturating_sub(shown / 2)
            .min(list.len().saturating_sub(shown));
        for (i, row) in list.iter().skip(first).take(shown).enumerate() {
            let mut c = 0;
            for (text, tone) in row {
                c = g.put(top + i, c, text, *tone);
            }
        }
        self.place(&mut g, &card, top, view_h, avail, cols);
        let mut lines = self.lines(&g, cols, rows);
        if !self.full && (first..first + shown).contains(&at) {
            if let Some(line) = lines.get_mut(top + at - first) {
                for span in &mut line.spans {
                    span.style = match self.opts.color {
                        true => span.style.bg(self.opts.theme.select_bg),
                        false => span.style.add_modifier(Modifier::REVERSED),
                    };
                }
            }
        }
        lines
    }

    /* the full texts the card wants: the selected job's, or on the asks screen the ask's; the
    events screen wants the events instead */
    fn wanted(&self) -> Want {
        let detail = match (self.events, self.asks) {
            (true, _) => None,
            (false, true) => self.ask_sel.as_ref().map(|cid| format!("ask={cid}")),
            (false, false) => self.sel.as_ref().map(|id| format!("detail={id}")),
        };
        Want {
            detail,
            events: self.events,
        }
    }

    fn fresh(&self) -> bool {
        self.last_ok
            .is_some_and(|t| t.elapsed() <= Duration::from_secs(2))
    }

    /* a stale snapshot holds all motion still */
    fn moving(&self) -> bool {
        self.opts.anim && self.fresh()
    }

    /* whole seconds rounded up, so a snapshot past the fresh two seconds reads at least 3s */
    fn age(&self) -> String {
        self.last_ok.map_or("--".to_string(), |t| {
            let elapsed = t.elapsed();
            format!(
                "{}s",
                elapsed.as_secs() + u64::from(elapsed.subsec_nanos() > 0)
            )
        })
    }

    /* the hub's error in a few words, leading the row under the rule; the rule's mark gives
    the snapshot's age */
    fn error_lead(&self) -> Option<(String, Tone)> {
        self.error
            .as_ref()
            .map(|error| (format!("! {error}"), Tone::Warn))
    }

    /* the hub did not answer: say so, and how old the snapshot on screen is */
    fn stale(&self, error: &str) -> String {
        if self.snap.is_none() {
            return format!("! {error}");
        }
        format!("! {error} · showing the snapshot from {} ago", self.age())
    }

    /* the rule's last word: ● while the snapshot is fresh, ○ and its age once it is older */
    fn freshness(&self) -> (String, Tone) {
        match self.fresh() {
            true => ("●".into(), Tone::Done),
            false => (format!("○ {}", self.age()), Tone::Warn),
        }
    }

    /* what the freshness takes of the rule, with the separator before it and the space after */
    fn mark_room(&self) -> usize {
        layout::width(&self.freshness().0) + 3
    }

    /* the rule's right end: the screen's tags, then the freshness while a snapshot shows, one
    rule character after */
    fn rule_end(&self, g: &mut Grid, row: usize, cols: usize, mut right: Vec<(String, Tone)>) {
        if self.snap.is_some() {
            let sep = if right.is_empty() { " " } else { "· " };
            right.push((sep.into(), Tone::Dim));
            right.push(self.freshness());
            right.push((" ".into(), Tone::Dim));
        }
        let used: usize = right.iter().map(|(text, _)| layout::width(text)).sum();
        let mut at = cols.saturating_sub(used + 1);
        for (text, tone) in right {
            at = g.put(row, at, &text, tone);
        }
    }

    /* the right end of a message's last line on a screen with nothing listed: the screen's
    tags while the hub answers, then the freshness; the tags give way first, the last one first */
    fn message_end(
        &self,
        g: &mut Grid,
        row: usize,
        taken: usize,
        cols: usize,
        tags: impl IntoIterator<Item = (String, Tone)>,
    ) {
        let tags: Vec<(String, Tone)> = tags
            .into_iter()
            .filter(|_| self.error.is_none())
            .chain([self.freshness()])
            .collect();
        let last = tags.len() - 1;
        for keep in (0..=last).rev() {
            let shown: Vec<&(String, Tone)> = tags[..keep].iter().chain(&tags[last..]).collect();
            let w =
                shown.iter().map(|(t, _)| layout::width(t)).sum::<usize>() + 3 * (shown.len() - 1);
            if taken + 2 + w <= cols {
                let mut c = cols - w;
                for (i, (t, tone)) in shown.into_iter().enumerate() {
                    if i > 0 {
                        c = g.put(row, c, " · ", Tone::Dim);
                    }
                    c = g.put(row, c, t, *tone);
                }
                break;
            }
        }
    }

    /* the row under the rule: what went wrong with the data, then what needs a person; a row
    longer than the pane is cut and ends in …. Says whether it drew anything */
    fn under_rule(
        &self,
        g: &mut Grid,
        row: usize,
        cols: usize,
        lead: Option<(String, Tone)>,
    ) -> bool {
        let items = self
            .snap
            .as_ref()
            .map(|snap| layout::attention(snap))
            .unwrap_or_default();
        /* every head before any reason; reasons join in order while they fit; heads that already
        overflow are cut */
        let push_heads = |pieces: &mut Vec<(String, Tone)>, reasons: usize| {
            for (i, item) in items.iter().enumerate() {
                let sep = match (i, pieces.is_empty()) {
                    (0, true) => "attention ",
                    (0, false) => " · ",
                    _ => " ",
                };
                pieces.push((sep.into(), Tone::Dim));
                let (head, tone) = &item[0];
                pieces.push((format!(" {head} "), *tone));
                if i < reasons {
                    pieces.extend(item[1..].iter().cloned());
                }
            }
        };
        let mut pieces: Vec<(String, Tone)> = lead.clone().into_iter().collect();
        push_heads(&mut pieces, 0);
        if row_width(&pieces) <= cols {
            let mut used = row_width(&pieces);
            let mut reasons = 0;
            for item in &items {
                let extra = row_width(&item[1..]);
                if used + extra <= cols {
                    used += extra;
                    reasons += 1;
                } else {
                    break;
                }
            }
            if reasons > 0 && items.iter().take(reasons).any(|item| item.len() > 1) {
                pieces = lead.clone().into_iter().collect();
                push_heads(&mut pieces, reasons);
            }
        }
        let fits = pieces
            .iter()
            .map(|(text, _)| layout::width(text))
            .sum::<usize>()
            <= cols;
        let mut c = 0;
        for (text, tone) in &pieces {
            if fits || c + layout::width(text) < cols {
                c = g.put(row, c, text, *tone);
            } else {
                g.put(row, c, &layout::ellipsis(text, cols - c), *tone);
                break;
            }
        }
        !pieces.is_empty()
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
                Tone::Badge => Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
                Tone::Title | Tone::PillWait | Tone::PillFail | Tone::PillRun | Tone::PillText => {
                    Style::default().add_modifier(Modifier::BOLD)
                }
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
            Tone::Near | Tone::Flow => fg(t.near),
            Tone::Back => fg(t.done),
            Tone::Select => fg(t.select).bg(t.select_bg),
            Tone::Badge => fg(t.wait).add_modifier(Modifier::REVERSED | Modifier::BOLD),
            Tone::PillWait => fg(t.wait).bg(t.select_bg),
            Tone::PillFail => fg(t.fail).bg(t.select_bg),
            Tone::PillRun => fg(t.run).bg(t.select_bg),
            Tone::PillText => fg(t.text).bg(t.select_bg),
        }
    }
}

fn draw(frame: &mut Frame, app: &mut App, tick: usize) {
    let area = frame.area();
    let lines = app.screen(area.width as usize, area.height as usize, tick);
    frame.render_widget(Paragraph::new(lines), area);
}

/* an ask's arrow steps every other frame, Flow toward the recipient and Back toward the
sender; a still pane, and the first frame, show the arrowhead in the middle */
fn walk(g: &mut Grid, tick: usize, moving: bool) {
    let step = if moving { tick / 2 + 1 } else { 1 };
    for row in g.rows.values_mut() {
        let marked: Vec<usize> = row
            .iter()
            .filter(|(_, cell)| matches!(cell.tone, Tone::Flow | Tone::Back))
            .map(|(c, _)| *c)
            .collect();
        for run in marked.chunk_by(|a, b| *b == a + 1) {
            for (i, c) in run.iter().enumerate() {
                let cell = row.get_mut(c).expect("collected from this row");
                let (at, head) = match cell.tone {
                    Tone::Back => (run.len() - 1 - step % run.len(), '◂'),
                    _ => (step % run.len(), '▸'),
                };
                cell.ch = if i == at { head } else { '─' };
            }
        }
    }
}

/* the rail cells that lead into the glyph at (r, c), in the order work travels them: the
spine above it in the vertical flow, the connector on its left in the horizontal one */
fn rail_into(g: &Grid, r: usize, c: usize) -> Vec<(usize, usize)> {
    let rail = |r: usize, c: usize, chars: &str| {
        g.rows
            .get(&r)
            .and_then(|row| row.get(&c))
            .is_some_and(|cell| cell.tone == Tone::Line && chars.contains(cell.ch))
    };
    if r > 0 && rail(r - 1, c, "│┼┬┴┌┐") {
        return vec![(r - 1, c)];
    }
    [3, 2]
        .into_iter()
        .filter(|d| c >= *d && rail(r, c - d, "─┬├└┤┘"))
        .map(|d| (r, c - d))
        .collect()
}

fn just_sent(snap: &model::Frame, job: &model::Job) -> bool {
    snap.fresh
        && snap
            .ask(job.ask_id.as_deref())
            .is_some_and(|ask| ask.opened_just_now == Some(true))
}

/* the jobs screen's keys by priority: the view toggle and the asks screen stay, the others drop
from the end until the row fits, so no key is cut in the middle */
fn job_keys(cols: usize, timeline: bool, copy: bool, asks: bool) -> String {
    let wide = cols >= 96;
    let keys = [
        (if wide { "j/k ↑↓ move" } else { "j/k move" }, true, false),
        (
            if wide {
                "h/l ←→ stage"
            } else {
                "h/l stage"
            },
            true,
            false,
        ),
        ("tab chain", true, false),
        (if timeline { "t flow" } else { "t timeline" }, true, true),
        ("enter card", true, false),
        ("y copy", copy, false),
        ("a asks", asks, true),
        ("/ find", true, false),
        (if wide { "f hints" } else { "f hint" }, true, false),
        (if wide { "digits jump" } else { "1-9 jump" }, true, false),
        ("esc back", true, false),
    ];
    let mut shown: Vec<(&str, bool)> = keys
        .iter()
        .filter(|(_, on, _)| *on)
        .map(|(key, _, pinned)| (*key, *pinned))
        .collect();
    let used = |shown: &[(&str, bool)]| {
        shown
            .iter()
            .map(|(key, _)| layout::width(key))
            .sum::<usize>()
            + 2 * shown.len().saturating_sub(1)
    };
    while used(&shown) > cols {
        let Some(last) = shown.iter().rposition(|(_, pinned)| !pinned) else {
            break;
        };
        shown.remove(last);
    }
    shown
        .iter()
        .map(|(key, _)| *key)
        .collect::<Vec<_>>()
        .join("  ")
}

fn send_clipboard(command: &str, tmux: bool, out: &mut impl std::io::Write) -> std::io::Result<()> {
    if command.chars().any(char::is_control) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "command contains control characters",
        ));
    }
    let osc = format!("\x1b]52;c;{}\x07", base64(command.as_bytes()));
    let sequence = if tmux {
        format!("{osc}\x1bPtmux;\x1b{osc}\x1b\\")
    } else {
        osc
    };
    out.write_all(sequence.as_bytes())?;
    out.flush()
}

fn base64(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for b in bytes.chunks(3) {
        let n = (u32::from(b[0]) << 16)
            | (u32::from(*b.get(1).unwrap_or(&0)) << 8)
            | u32::from(*b.get(2).unwrap_or(&0));
        for i in 0..4 {
            out.push(if i > b.len() {
                '='
            } else {
                DIGITS[((n >> (18 - i * 6)) & 63) as usize] as char
            });
        }
    }
    out
}

fn ascii(ch: char) -> char {
    match ch {
        '─' => '-',
        '━' => '=',
        '╌' => '~',
        '│' => '|',
        '┌' | '┐' | '└' | '┘' | '┬' | '┴' | '┼' | '├' | '┤' => '+',
        '◀' | '←' | '◂' => '<',
        '●' => '*',
        '◆' => '>',
        '⣿' => '#',
        '⏎' => '>',
        '↓' => 'v',
        '×' => 'x',
        '○' => 'o',
        '–' => '-',
        '·' => '|',
        '…' => '~',
        '↑' => '^',
        '→' | '▸' => '>',
        '•' => '.',
        '◉' => '@',
        '⠋' | '⠼' | '⠇' => '|',
        '⠙' | '⠴' | '⠏' => '/',
        '⠹' | '⠦' => '-',
        '⠸' | '⠧' => '\\',
        '✉' => '+',
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

/* what the fetch thread reads besides the snapshot: the full text the card wants, and the
events while their screen is up */
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Want {
    pub detail: Option<String>,
    pub events: bool,
}

/* what the fetch thread sends: a snapshot, or the events read after it */
enum Got {
    Snap(Box<std::result::Result<(model::HubMode, Snapshot), String>>),
    Events(std::result::Result<Vec<model::Event>, String>),
}

/* GET /events for the view's circle, after `since` */
fn read_events(
    circle: Option<&str>,
    since: Option<u64>,
) -> std::result::Result<Vec<model::Event>, String> {
    let mut query: Vec<String> = Vec::new();
    if let Some(c) = circle {
        query.push(format!("circle={c}"));
    }
    if let Some(s) = since {
        query.push(format!("since={s}"));
    }
    let path = match query.is_empty() {
        true => "/events".to_string(),
        false => format!("/events?{}", query.join("&")),
    };
    crate::cli::request("GET", &path, None)
        .map_err(|e| e.to_string())
        .and_then(|v| serde_json::from_value::<Vec<model::Event>>(v).map_err(|e| e.to_string()))
}

fn decoded(value: serde_json::Value) -> std::result::Result<(model::HubMode, Snapshot), String> {
    match crate::wire::decode_snapshot(value)? {
        crate::wire::Decoded::V2(snap) => Ok((model::HubMode::V2, snap)),
        crate::wire::Decoded::V1Neutral(snap) => Ok((model::HubMode::Neutral, snap)),
        crate::wire::Decoded::Unsupported(version) => {
            Ok((model::HubMode::Unsupported(version), Snapshot::default()))
        }
    }
}

/* a request a second on a background thread, so the screen never waits for the network;
a poke (r, or a new selection) asks sooner, at most five times a second. While the events
screen is up the events follow each snapshot, only those after the newest seq read */
fn fetch(
    circle: Option<String>,
    want: Arc<Mutex<Want>>,
    tx: mpsc::Sender<Got>,
    poke: mpsc::Receiver<()>,
) {
    std::thread::spawn(move || {
        let mut since: Option<u64> = None;
        loop {
            let started = Instant::now();
            let wanted = want.lock().map(|w| w.clone()).unwrap_or_default();
            let mut query: Vec<String> = Vec::new();
            if let Some(c) = &circle {
                query.push(format!("circle={c}"));
            }
            if let Some(d) = &wanted.detail {
                query.push(d.clone());
            }
            let path = if query.is_empty() {
                "/snapshot".to_string()
            } else {
                format!("/snapshot?{}", query.join("&"))
            };
            let got = crate::cli::request("GET", &path, None)
                .map_err(|e| e.to_string())
                .and_then(decoded);
            if tx.send(Got::Snap(Box::new(got))).is_err() {
                return;
            }
            if wanted.events {
                let events = read_events(circle.as_deref(), since);
                if let Ok(list) = &events {
                    since = list.iter().filter_map(|e| e.seq).max().or(since);
                }
                if tx.send(Got::Events(events)).is_err() {
                    return;
                }
            }
            let _ = poke.recv_timeout(Duration::from_secs(1));
            /* a held key pokes on every step; five fetches a second follow it well enough */
            std::thread::sleep(Duration::from_millis(200).saturating_sub(started.elapsed()));
            while poke.try_recv().is_ok() {}
        }
    });
}

pub(crate) fn run(args: &[String]) -> Result<()> {
    let opts = parse(args)?;
    let want = Arc::new(Mutex::new(Want::default()));
    let (tx, rx) = mpsc::channel();
    let (poke_tx, poke_rx) = mpsc::channel();
    fetch(opts.circle.clone(), want.clone(), tx, poke_rx);
    let stop = Arc::new(AtomicBool::new(false));
    stop_on_signals(&stop);
    exit_on_hangup();
    let mut app = App::new(opts);
    let mut terminal = ratatui::try_init()?;
    let mut tick = 0usize;
    let mut asked = Want::default();
    let result = (|| -> Result<()> {
        while !stop.load(Ordering::Relaxed) {
            while let Ok(got) = rx.try_recv() {
                match got {
                    Got::Snap(snap) => app.receive(*snap),
                    Got::Events(events) => app.take_events(events),
                }
            }
            /* a new selection fetches its full text now instead of on the next second */
            let wanted = app.wanted();
            if wanted != asked {
                asked = wanted;
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
                Act::Copy(index) => app.copy_command(
                    index,
                    std::env::var_os("TMUX").is_some(),
                    &mut std::io::stdout(),
                ),
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

/* one row per block, the selected one marked: a station per stage (its most urgent job),
done/total, then what needs a person. Blocks with a failure or a person to answer come
first; at most three rows and a count of the rest, and the selected block always shows */
fn strip(
    blocks: &[Block],
    b: usize,
    snap: &Snapshot,
    cols: usize,
    rows: usize,
) -> Vec<layout::Row> {
    let urgent = |i: usize| {
        blocks[i]
            .order
            .iter()
            .filter_map(|id| snap.job(id))
            .any(|job| station(job).0 < 2)
    };
    let key = |i: &usize| (!urgent(*i), *i);
    let mut order: Vec<usize> = (0..blocks.len()).collect();
    order.sort_by_key(key);
    let shown = match blocks.len() <= rows.min(3) {
        true => blocks.len(),
        false => rows.saturating_sub(1).clamp(1, 3),
    };
    let mut pick: Vec<usize> = order[..shown].to_vec();
    if !pick.contains(&b) {
        pick[shown - 1] = b;
        pick.sort_by_key(key);
    }
    let name_w = pick
        .iter()
        .map(|&i| layout::width(block_name(&blocks[i])))
        .max()
        .unwrap_or(0)
        .min(16);
    let rest = blocks.len() - pick.len();
    let more = (rest > 0).then(|| (format!("  +{rest} more"), Tone::Dim));
    let fold = more.is_some() && pick.len() >= rows;
    let more_w = more
        .as_ref()
        .map(|(text, _)| layout::width(text))
        .unwrap_or(0);
    let mut out: Vec<layout::Row> = pick
        .iter()
        .enumerate()
        .map(|(idx, &i)| {
            let row_cols = if fold && idx + 1 == pick.len() {
                cols.saturating_sub(more_w)
            } else {
                cols
            };
            strip_row(&blocks[i], i == b, snap, name_w, row_cols)
        })
        .collect();
    if let Some(more) = more {
        if out.len() < rows {
            out.push(vec![more]);
        } else if let Some(last) = out.last_mut() {
            last.push(more);
        }
    }
    out
}

fn block_name(block: &Block) -> &str {
    match block.chain.loose {
        true => "independent jobs",
        false => &block.chain.name,
    }
}

fn row_width(row: &[(String, Tone)]) -> usize {
    row.iter().map(|(text, _)| layout::width(text)).sum()
}

/* what a running job waits on a person for: a permission prompt or an idle holder */
fn asking(job: &model::Job) -> Option<&'static str> {
    match job.progress.as_ref().and_then(|p| p.state.as_ref()) {
        Some(crate::wire::Progress::Wait { .. }) => Some("WAIT"),
        Some(crate::wire::Progress::Idle { .. }) => Some("IDLE"),
        _ => None,
    }
}

/* a job's station: how soon a person should look (failed, then a person asked, then
running, queued and done), its glyph and its tone */
fn station(job: &model::Job) -> (u8, char, Tone) {
    let (ch, tone) = layout::glyph(&job.state);
    match job.state.as_str() {
        "failed" => (0, ch, tone),
        "running" if asking(job).is_some() => (1, ch, Tone::Wait),
        "running" => (2, ch, tone),
        "done" => (4, ch, tone),
        _ => (3, ch, tone),
    }
}

/* the pills give way first, then the stations from the right; done/total stays whole
and the name shrinks; what still does not fit is cut */
fn strip_row(block: &Block, sel: bool, snap: &Snapshot, name_w: usize, cols: usize) -> layout::Row {
    let jobs: Vec<&model::Job> = block.order.iter().filter_map(|id| snap.job(id)).collect();
    let stations: Vec<(char, Tone)> = block
        .chain
        .stages
        .iter()
        .filter_map(|stage| {
            stage
                .iter()
                .filter_map(|id| snap.job(id))
                .map(station)
                .min_by_key(|s| s.0)
        })
        .map(|(_, ch, tone)| (ch, tone))
        .collect();
    let title = |job: &model::Job| layout::fit(&job.title, 12);
    let mut pills: Vec<(String, Tone)> = jobs
        .iter()
        .filter(|job| job.state == "failed")
        .map(|job| (format!("× {}", title(job)), Tone::PillFail))
        .collect();
    for (word, tone) in [("WAIT", Tone::Badge), ("IDLE", Tone::PillWait)] {
        pills.extend(
            jobs.iter()
                .filter(|job| job.state == "running" && asking(job) == Some(word))
                .map(|job| (format!("{word} {}", title(job)), tone)),
        );
    }
    let running = jobs.iter().filter(|job| job.state == "running").count();
    if pills.is_empty() && running > 0 {
        pills.push((format!("{running} run"), Tone::PillRun));
    }
    let done = jobs.iter().filter(|job| job.state == "done").count();
    let count_done = done.to_string();
    let count_tot = format!("/{}", jobs.len());
    let count_w = 2 + layout::width(&count_done) + layout::width(&count_tot);
    let body_cols = cols.saturating_sub(count_w);
    let front = |name_max: usize, keep_stations: usize| {
        let name = layout::fit(block_name(block), name_max);
        let pad = " ".repeat(name_max.saturating_sub(layout::width(&name)) + 2);
        let mut row: layout::Row = vec![
            ((if sel { "▸ " } else { "  " }).into(), Tone::Near),
            (
                format!("{name}{pad}"),
                if sel { Tone::Title } else { Tone::Plain },
            ),
        ];
        for (i, (ch, tone)) in stations.iter().take(keep_stations).enumerate() {
            if i > 0 {
                row.push(("─".into(), Tone::Dim));
            }
            row.push((ch.to_string(), *tone));
        }
        if keep_stations < stations.len() {
            row.push(("…".into(), Tone::Dim));
        }
        row
    };
    let build = |name_max: usize, keep_stations: usize, keep_pills: usize| {
        let mut row = front(name_max, keep_stations);
        row.push(("  ".into(), Tone::Plain));
        row.push((count_done.clone(), Tone::Done));
        row.push((count_tot.clone(), Tone::Dim));
        for (text, tone) in pills.iter().take(keep_pills) {
            row.push((" ".into(), Tone::Plain));
            row.push((format!(" {text} "), *tone));
        }
        row
    };
    let fits = |row: &layout::Row| row_width(row) <= cols;
    if let Some(row) = (0..=pills.len())
        .rev()
        .map(|keep| build(name_w, stations.len(), keep))
        .find(fits)
    {
        return row;
    }
    if let Some(row) = (1..stations.len())
        .rev()
        .map(|keep| build(name_w, keep, 0))
        .find(fits)
    {
        return row;
    }
    let keep_st = 1.min(stations.len());
    if let Some(row) = (0..=name_w).rev().map(|n| build(n, keep_st, 0)).find(fits) {
        return row;
    }
    let mut out = Vec::new();
    let mut c = 0;
    for (text, tone) in front(1.min(name_w), keep_st) {
        let w = layout::width(&text);
        if c + w <= body_cols {
            c += w;
            out.push((text, tone));
        } else {
            if body_cols > c {
                out.push((layout::ellipsis(&text, body_cols - c), tone));
            }
            break;
        }
    }
    out.push(("  ".into(), Tone::Plain));
    out.push((count_done, Tone::Done));
    out.push((count_tot, Tone::Dim));
    out
}
