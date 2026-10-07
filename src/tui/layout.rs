use super::model::{Ask, Chain, Frame, Job, Peer, Snapshot};
use crate::wire::{
    AskState, BlockReason, DeliveryCondition, DispatchState, IdleWhy, JobAction, JobRelation,
    Liveness, Progress,
};
use std::collections::{BTreeMap, HashMap, HashSet};
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
    /* an ask's arrow: Flow steps toward the recipient while it works, Back toward the sender
    once the ack is in */
    Flow,
    Back,
    /* what needs a person now: the wait colour as a solid block */
    Badge,
    /* a short label on the select background, in the colour of its state */
    PillWait,
    PillFail,
    PillRun,
    PillText,
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
    ellipsis(text, max)
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

/* the top line: the peers online in the view and what each is doing, in name order. When
they do not all fit, those waiting for a person or at work are placed first, a name that
does not fit is counted and the next one tried; names are never cut. `spin` is the
spinner's frame, `lit` whether the WAIT mark shows in this frame */
pub(crate) fn presence(snap: &Frame, cols: usize, spin: char, lit: bool) -> Vec<(String, Tone)> {
    if !snap.capabilities.roster {
        let hint = "peers: restart the hub on the current amesh to list them";
        return vec![(fit(hint, cols), Tone::Dim)];
    }
    let online = online_peers(snap);
    if online.is_empty() {
        return vec![(fit("no peers online", cols), Tone::Dim)];
    }
    let prefix = prefix(snap);
    let label = label(online.len());
    let need = |kept: &[&Peer]| line_width(&label, kept, online.len() - kept.len(), prefix);
    let mut kept = online.clone();
    if need(&kept) > cols {
        let mut ranked = online.clone();
        ranked.sort_by_key(|p| urgency(p));
        kept.clear();
        for peer in ranked {
            kept.push(peer);
            if need(&kept) > cols {
                kept.pop();
            }
        }
        if kept.is_empty() {
            return vec![(fit(&format!("{} online", online.len()), cols), Tone::Dim)];
        }
        kept.sort_by(|a, b| shown(a).cmp(shown(b)));
    }
    let mut out = vec![(label, Tone::Dim)];
    for (i, peer) in kept.iter().enumerate() {
        if i > 0 {
            out.push(("  ".into(), Tone::Plain));
        }
        out.push(mark(peer, spin, lit));
        out.extend(named(peer, prefix));
    }
    let hidden = online.len() - kept.len();
    if hidden > 0 {
        out.push((format!("  +{hidden}"), Tone::Dim));
    }
    out
}

/* the online peers on at most `rows` lines: the top line while they all fit on it; otherwise
a grid, each name padded to the widest and every further line indented under the first name,
so the names stand in columns. A grid taller than `rows` keeps those waiting or at work and
counts the rest in its last cell; a name wider than any column leaves the one top line */
pub(crate) fn presence_lines(
    snap: &Frame,
    cols: usize,
    rows: usize,
    spin: char,
    lit: bool,
) -> Vec<Vec<(String, Tone)>> {
    let line = presence(snap, cols, spin, lit);
    let prefix = prefix(snap);
    let online = online_peers(snap);
    let label = label(online.len());
    let single = line_width(&label, &online, 0, prefix);
    let cell = online
        .iter()
        .map(|p| 2 + width(brief(shown(p), prefix)) + width(&tail(p)))
        .chain([width(&format!("+{}", online.len()))])
        .max()
        .unwrap_or(0);
    let per = (cols.saturating_sub(width(&label)) + 2) / (cell + 2);
    if rows < 2 || !snap.capabilities.roster || online.is_empty() || single <= cols || per == 0 {
        return vec![line];
    }
    let mut kept = online.clone();
    if kept.len() > rows * per {
        kept.sort_by_key(|p| urgency(p));
        kept.truncate(rows * per - 1);
        kept.sort_by(|a, b| shown(a).cmp(shown(b)));
    }
    let hidden = online.len() - kept.len();
    let mut cells: Vec<Vec<(String, Tone)>> = kept
        .iter()
        .map(|peer| [vec![mark(peer, spin, lit)], named(peer, prefix)].concat())
        .collect();
    if hidden > 0 {
        cells.push(vec![(format!("+{hidden}"), Tone::Dim)]);
    }
    cells
        .chunks(per)
        .enumerate()
        .map(|(r, group)| {
            let lead = if r == 0 {
                label.clone()
            } else {
                " ".repeat(width(&label))
            };
            let mut out = vec![(lead, Tone::Dim)];
            for (i, pieces) in group.iter().enumerate() {
                if i > 0 {
                    let used: usize = group[i - 1].iter().map(|(t, _)| width(t)).sum();
                    out.push((" ".repeat(cell - used + 2), Tone::Plain));
                }
                out.extend(pieces.iter().cloned());
            }
            out
        })
        .collect()
}

/* the online peers, in name order */
fn online_peers(snap: &Snapshot) -> Vec<&Peer> {
    let mut online: Vec<&Peer> = snap
        .roster
        .iter()
        .filter(|p| p.status == "online")
        .collect();
    online.sort_by(|a, b| shown(a).cmp(shown(b)));
    online
}

fn label(online: usize) -> String {
    format!("{online} online · ")
}

/* the top line's width with `kept` shown after `label` and `hidden` counted */
fn line_width(label: &str, kept: &[&Peer], hidden: usize, prefix: &str) -> usize {
    width(label)
        + kept
            .iter()
            .map(|p| 2 + width(brief(shown(p), prefix)) + width(&tail(p)))
            .sum::<usize>()
        + 2 * kept.len().saturating_sub(1)
        + if hidden > 0 {
            2 + width(&format!("+{hidden}"))
        } else {
            0
        }
}

fn shown(peer: &Peer) -> &str {
    if peer.name.is_empty() {
        &peer.peer_id
    } else {
        &peer.name
    }
}

/* the prefix every peer in the view shares up to a '-': the {folder}- of {folder}-{backend}
names, which tells nothing apart inside a circle */
pub(crate) fn prefix(snap: &Snapshot) -> &str {
    let mut names: Vec<&str> = snap
        .roster
        .iter()
        .map(shown)
        .filter(|name| !name.is_empty() && *name != "anonymous")
        .collect();
    names.sort_unstable();
    names.dedup();
    let [first, .., last] = names.as_slice() else {
        return "";
    };
    let same = first
        .char_indices()
        .zip(last.chars())
        .take_while(|((_, a), b)| a == b)
        .last()
        .map_or(0, |((at, ch), _)| at + ch.len_utf8());
    let prefix = first[..same].rfind('-').map_or("", |cut| &first[..=cut]);
    match names.iter().all(|name| name.len() > prefix.len()) {
        true => prefix,
        false => "",
    }
}

/* a name without the view's prefix; a peer that left or another circle's peer from the same
folder loses it too */
pub(crate) fn brief<'a>(name: &'a str, prefix: &str) -> &'a str {
    match name.strip_prefix(prefix) {
        Some(rest) if !rest.is_empty() => rest,
        _ => name,
    }
}

/* a name on the top line. NoPush dims it. A stuck queue under Push or NoPush adds the count */
fn named(peer: &Peer, prefix: &str) -> Vec<(String, Tone)> {
    let tone = match peer.delivery.as_ref().map(|row| row.condition) {
        Some(DeliveryCondition::NoPush) => Tone::Dim,
        _ => Tone::Soft,
    };
    let mut out = vec![(format!(" {}", brief(shown(peer), prefix)), tone)];
    let mark = tail(peer);
    if !mark.is_empty() {
        out.push((mark, Tone::Warn));
    }
    out
}

fn tail(peer: &Peer) -> String {
    let Some(delivery) = peer.delivery.as_ref() else {
        return String::new();
    };
    let Some(stuck) = delivery.stuck.as_ref() else {
        return String::new();
    };
    match delivery.condition {
        DeliveryCondition::Push | DeliveryCondition::NoPush => format!(" ✉{}", stuck.count),
        _ => String::new(),
    }
}

pub(crate) fn delivery(snap: &Frame, peer: &Peer) -> Row {
    let Some(delivery) = peer.delivery.as_ref() else {
        return Vec::new();
    };
    let mut out: Row = Vec::new();
    match delivery.condition {
        DeliveryCondition::NoPush => out.push(("no push".into(), Tone::Dim)),
        DeliveryCondition::Push => {}
        DeliveryCondition::Offline | DeliveryCondition::Unknown => return Vec::new(),
    }
    if let Some(stuck) = delivery.stuck.as_ref() {
        if !out.is_empty() {
            out.push((" · ".into(), Tone::Dim));
        }
        let age = ago(snap.captured_at, Some(stuck.since));
        out.push((format!("{} queued for {age}", stuck.count), Tone::Warn));
    }
    out
}

/* what needs a person, from the hub's facts: a peer at a prompt, the recipient of an open
ask the hub calls idle, a stuck queue, and a failed job, one item for each: its head and,
when there is one, its reason. The peers are the circle's and the recipients of its open
asks, in another circle too */
pub(crate) fn attention(snap: &Snapshot) -> Vec<Row> {
    let now = snap.captured_at;
    let prefix = prefix(snap);
    let mut items: Vec<Row> = Vec::new();
    let recipients = snap
        .asks
        .iter()
        .filter(|ask| ask.open)
        .filter_map(|ask| snap.peer_by_id(&ask.to_peer_id));
    let mut seen = HashSet::new();
    let peers: Vec<&Peer> = snap
        .roster
        .iter()
        .chain(recipients)
        .filter(|peer| seen.insert(peer.peer_id.as_str()))
        .collect();
    for peer in &peers {
        if let Some(Liveness::Wait { since, reason }) = &peer.liveness {
            let mut item = vec![(
                format!(
                    "WAIT {} {}",
                    brief(shown(peer), prefix),
                    ago(now, Some(*since))
                ),
                Tone::Badge,
            )];
            item.extend(reason.as_ref().map(|r| (format!(" {r}"), Tone::Dim)));
            items.push(item);
        }
    }
    /* one item per recipient, from the ask that stalled first */
    let mut idle: Vec<(&Ask, u64, &IdleWhy)> = Vec::new();
    for ask in &snap.asks {
        let Some(AskState::Open {
            progress: Progress::Idle { since, why },
        }) = &ask.state
        else {
            continue;
        };
        match idle
            .iter_mut()
            .find(|(kept, ..)| kept.to_peer_id == ask.to_peer_id)
        {
            Some(kept) if *since < kept.1 => *kept = (ask, *since, why),
            Some(_) => {}
            None => idle.push((ask, *since, why)),
        }
    }
    for (ask, since, why) in idle {
        let name = brief(
            snap.peer_by_id(&ask.to_peer_id)
                .map_or(ask.to_peer.as_str(), shown),
            prefix,
        );
        let mut item = vec![(
            format!("IDLE {name} {}", ago(now, Some(since))),
            Tone::PillWait,
        )];
        let why = idle_why(why);
        if !why.is_empty() {
            item.push((format!(" {why}"), Tone::Dim));
        }
        items.push(item);
    }
    for peer in &peers {
        let Some(delivery) = &peer.delivery else {
            continue;
        };
        let no_push = delivery.condition == DeliveryCondition::NoPush;
        let Some(stuck) = delivery
            .stuck
            .as_ref()
            .filter(|_| no_push || delivery.condition == DeliveryCondition::Push)
        else {
            continue;
        };
        let mut item = vec![(
            format!("✉{} {}", stuck.count, brief(shown(peer), prefix)),
            Tone::PillWait,
        )];
        if no_push {
            item.push((" no push".into(), Tone::Dim));
        }
        items.push(item);
    }
    for job in snap.jobs.iter().filter(|job| job.state == "failed") {
        items.push(vec![(format!("× {}", job.title), Tone::PillFail)]);
    }
    items
}

/* waiting for a person first, then at work, then the rest */
fn urgency(peer: &Peer) -> u8 {
    match peer.liveness {
        Some(Liveness::Wait { .. }) => 0,
        Some(Liveness::Work { .. }) => 1,
        _ => 2,
    }
}

fn mark(peer: &Peer, spin: char, lit: bool) -> (String, Tone) {
    let (mark, tone) = match peer.liveness {
        Some(Liveness::Work { .. }) => (spin, Tone::Run),
        Some(Liveness::Wait { .. }) => (if lit { '!' } else { ' ' }, Tone::Wait),
        Some(Liveness::Idle { .. }) => ('○', Tone::Dim),
        _ => ('●', Tone::Soft),
    };
    (mark.to_string(), tone)
}

/* how many events the hub keeps for the view, for the rule under the header; nothing from a
hub that does not count them */
pub(crate) fn events_tag(snap: &Snapshot) -> Option<String> {
    snap.capabilities.event_count.then(|| {
        let n = snap.event_count;
        format!("{n} event{}", if n == 1 { "" } else { "s" })
    })
}

/* the mark between an ask's sender and recipient: an open ask's steps toward the recipient
while it works and rests while it does not; one the hub closed within the second before a
fresh snapshot steps back to the sender, or crosses out when it closed failed, and an older
one reads > */
pub(crate) fn arrow(snap: &Frame, ask: &Ask, working: bool) -> (String, Tone) {
    match ask.open {
        true => ("─▸─".into(), if working { Tone::Flow } else { Tone::Dim }),
        false if !(snap.fresh && ask.closed_just_now == Some(true)) => (">".into(), Tone::Plain),
        false if super::asks::failed(ask) => ("─×─".into(), Tone::Fail),
        false => ("─◂─".into(), Tone::Back),
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

/* local wall-clock time with seconds, for what comes several a minute */
pub(crate) fn stamp(t: Option<u64>) -> String {
    let Some(t) = t else {
        return "--:--:--".into();
    };
    let local = t as i64 + utc_offset(t);
    format!(
        "{:02}:{:02}:{:02}",
        local.rem_euclid(86_400) / 3600,
        local.rem_euclid(3600) / 60,
        local.rem_euclid(60)
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
    pub snap: Frame,
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
        } else if self.waiting(id) || self.idle(id) {
            Tone::Wait
        } else if self.state(id) == "failed" {
            Tone::Fail
        } else if self.frontier(id) {
            Tone::Plain
        } else {
            Tone::Dim
        }
    }

    /* a worker that ended its turn while the job's ask is still open */
    fn idle(&self, id: &str) -> bool {
        self.snap.job(id).and_then(idle_since).is_some()
    }

    /* where the eye should go: running or failed jobs, and queued ones a dependency blocks */
    fn frontier(&self, id: &str) -> bool {
        matches!(self.state(id), "running" | "failed")
            || self.snap.job(id).is_some_and(|job| {
                matches!(job.dispatch_state, Some(DispatchState::Blocked { .. }))
            })
    }

    /* an edge takes the theme's line when it leads to a frontier job */
    fn edge<'a>(&self, ids: impl IntoIterator<Item = &'a String>) -> Tone {
        if ids.into_iter().any(|id| self.frontier(id)) {
            Tone::Line
        } else {
            Tone::Dim
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

    /* a flow row marks a worker waiting for a permission */
    fn waiting(&self, id: &str) -> bool {
        self.snap.job(id).is_some_and(|job| {
            matches!(
                job.progress.as_ref().and_then(|p| p.state.as_ref()),
                Some(Progress::Wait { .. })
            )
        })
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
        let tone = match self.waiting(id) || self.idle(id) {
            true => Tone::Wait,
            false => tone,
        };
        g.put_node(r, c, &ch.to_string(), tone, self.node(id), true);
    }

    /* the tail in Dim, the worker's name at its front in Soft */
    fn put_tail(&self, g: &mut Grid, r: usize, c: usize, id: &str, tail: &str) -> usize {
        let end = g.put(r, c, tail, Tone::Dim);
        let worker = self
            .snap
            .job(id)
            .and_then(|job| job.assigned_peer.as_deref())
            .unwrap_or("");
        if !worker.is_empty() && tail.starts_with(worker) {
            g.put(r, c, worker, Tone::Soft);
        }
        end
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
            let frame = v.edge(group);
            g.put(r, 0, &format!("┌{}┐", "─".repeat(room - 2)), frame);
            if i > 0 {
                g.put(r, center, "┴", frame);
            }
            r += 1;
            g.put(r, 0, "│", frame);
            g.put(r, room - 1, "│", frame);
            g.put(r, 2, &title, Tone::Dim);
            r += 1;
            for pair in group.chunks(2) {
                g.put(r, 0, "│", frame);
                g.put(r, room - 1, "│", frame);
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
            g.put(r, 0, &format!("└{}┘", "─".repeat(room - 2)), frame);
            if let Some(next) = groups.get(i + 1) {
                g.put(r, center, "┬", v.edge(next));
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
                end = v.put_tail(&mut g, r, end + 1, id, &tail);
            }
            if wait {
                end = g.put(r, end, " WAIT ", Tone::Badge);
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
            g.put(r, center, "│", v.edge(next));
            r += 1;
            continue;
        }
        let fan_out = group.len() == 1;
        let k = if fan_out { next.len() } else { group.len() };
        let xs: Vec<usize> = (0..k)
            .map(|n| center + n * slot - slot * (k - 1) / 2)
            .collect();
        let (a, b) = (xs[0], xs[k - 1]);
        /* a fan-in leads every branch to the next job; a fan-out leads each branch to its own
        job, so a corner takes its job's tone and a run takes the line when a frontier job lies
        beyond it */
        let lead = |x: usize| {
            if !fan_out {
                return v.edge(next);
            }
            let past = |n: usize| {
                (xs[n].min(center)..=xs[n].max(center)).contains(&x) && v.frontier(&next[n])
            };
            match xs.iter().position(|&p| p == x) {
                Some(n) if x != center => v.edge([&next[n]]),
                _ if (0..k).any(past) => Tone::Line,
                _ => Tone::Dim,
            }
        };
        for x in a..=b {
            g.put(r, x, "─", lead(x));
        }
        for x in &xs[1..k - 1] {
            g.put(r, *x, if fan_out { "┬" } else { "┴" }, lead(*x));
        }
        g.put(r, a, if fan_out { "┌" } else { "└" }, lead(a));
        g.put(r, b, if fan_out { "┐" } else { "┘" }, lead(b));
        let join = if xs.contains(&center) {
            "┼"
        } else if fan_out {
            "┴"
        } else {
            "┬"
        };
        g.put(r, center, join, lead(center));
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
            v.edge([id]),
        );
        for rr in r0 + 1..r1 {
            g.put(rr, x, "│", v.edge([id]));
        }
        g.put(
            r1,
            c1 + 1,
            &format!("◀{}┘", "─".repeat(x - c1 - 2)),
            v.edge([id]),
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

/* one row per job in flow order, bars on a shared sent→now axis */
pub(crate) fn timeline(v: &View, cols: usize) -> Grid {
    let mut ids = v.chain.topo();
    ids.sort_by_key(|id| v.num[id]);
    let now = v.snap.captured_at;
    let sent_of = |id: &str| -> Option<u64> {
        let job = v.snap.job(id)?;
        if job.state == "queued" {
            return None;
        }
        v.snap.ask(job.ask_id.as_deref()).and_then(|a| a.opened_at)
    };
    /* the axis ends at the snapshot; a start after it is not elapsed time */
    let start_of = |id: &str| sent_of(id).filter(|&t| t <= now);
    let t0 = ids
        .iter()
        .filter_map(|id| start_of(id))
        .min()
        .unwrap_or(now);
    let digits = ids
        .iter()
        .map(|id| v.num[id].to_string().len())
        .max()
        .unwrap_or(1);
    let state_w = if cols >= 80 { 10 } else { 8 };
    let worker_w = if cols >= 100 {
        8
    } else if cols >= 80 {
        6
    } else {
        3
    };
    let num_w = digits + 1;
    let mut title_w = if cols >= 100 {
        18
    } else if cols >= 80 {
        12
    } else {
        6
    };
    let mut bar_w = 4;
    let fitted =
        |title_w: usize, bar_w: usize| num_w + title_w + 1 + worker_w + 1 + bar_w + 1 + state_w;
    while fitted(title_w, bar_w + 1) <= cols {
        bar_w += 1;
    }
    while fitted(title_w, bar_w) > cols && title_w > 3 {
        title_w -= 1;
    }
    let c_title = num_w;
    let c_worker = c_title + title_w + 1;
    let c_bar = c_worker + worker_w + 1;
    let c_state = c_bar + bar_w + 1;
    let mut g = Grid::default();
    place_axis(&mut g, 0, c_bar, bar_w, t0, now);
    for (i, id) in ids.iter().enumerate() {
        let r = i + 1;
        let Some(job) = v.snap.job(id) else {
            continue;
        };
        let (num_tone, title_tone) = if *id == v.sel {
            (Tone::Select, Tone::Select)
        } else {
            (Tone::Dim, Tone::Plain)
        };
        /* the row carries the job's node, so scrolling and hints find it without a glyph */
        g.put_node(
            r,
            0,
            &format!("{} ", v.num[id]),
            num_tone,
            v.node(id),
            false,
        );
        g.put_node(
            r,
            c_title,
            &fit(&v.title(id), title_w),
            title_tone,
            v.node(id),
            false,
        );
        g.put(
            r,
            c_worker,
            &fit(worker_name(&v.snap, job), worker_w),
            Tone::Soft,
        );
        let (word, word_tone) = state_word(job, now, sent_of(id));
        g.put(r, c_state, &fit(&word, state_w), word_tone);
        /* only a running job ends at the snapshot; a finished one needs its own stamp */
        let end = match job.state.as_str() {
            "running" => Some(now),
            _ => job.finished_at.map(|t| t.min(now)),
        };
        let (Some(sent), Some(ended)) = (start_of(id), end) else {
            continue;
        };
        if ended < sent {
            continue;
        }
        let a = map_t(sent, t0, now, bar_w).min(bar_w - 1);
        let b = map_t(ended, t0, now, bar_w).max(a + 1).min(bar_w);
        let idle = idle_since(job).map_or(b, |s| map_t(s, t0, now, bar_w).clamp(a, b - 1));
        for x in a..b {
            let (ch, tone) = match job.state.as_str() {
                "failed" if x + 1 == b => ("×", Tone::Fail),
                "failed" => ("━", Tone::Fail),
                _ if x >= idle => ("╌", Tone::Wait),
                "running" => ("━", Tone::Run),
                _ => ("━", Tone::Dim),
            };
            g.put(r, c_bar + x, ch, tone);
        }
    }
    g
}

fn idle_since(job: &Job) -> Option<u64> {
    match job.progress.as_ref().and_then(|p| p.state.as_ref()) {
        Some(Progress::Idle { since, .. }) if job.state == "running" => Some(*since),
        _ => None,
    }
}

fn state_word(job: &Job, now: u64, sent: Option<u64>) -> (String, Tone) {
    if let Some(since) = idle_since(job) {
        return (format!("IDLE {}", ago(now, Some(since))), Tone::PillWait);
    }
    if job.state == "queued" && matches!(job.dispatch_state, Some(DispatchState::Blocked { .. })) {
        return ("blocked".into(), Tone::PillText);
    }
    match job.state.as_str() {
        "done" => ("done".into(), Tone::Done),
        "running" => (format!("run {}", ago(now, sent)), Tone::Run),
        "failed" => ("failed".into(), Tone::PillFail),
        other => (other.into(), Tone::Dim),
    }
}

fn map_t(t: u64, t0: u64, t1: u64, bar_w: usize) -> usize {
    if bar_w == 0 || t1 <= t0 {
        return 0;
    }
    let dt = t.saturating_sub(t0).min(t1 - t0);
    ((dt as u128) * (bar_w as u128) / ((t1 - t0) as u128)) as usize
}

fn place_axis(g: &mut Grid, r: usize, c_bar: usize, bar_w: usize, t0: u64, t1: u64) {
    let start = clock(Some(t0));
    let mid = clock(Some(t0 + t1.saturating_sub(t0) / 2));
    let mut taken: Vec<(usize, usize)> = Vec::new();
    let mut place = |c: usize, text: &str| {
        let w = width(text);
        let end = c + w;
        if taken.iter().any(|&(a, b)| c < b && end > a) {
            return;
        }
        g.put(r, c, text, Tone::Dim);
        taken.push((c, end));
    };
    place(c_bar + bar_w.saturating_sub(width("now")), "now");
    place(c_bar, &start);
    if t1 > t0 {
        place(c_bar + (bar_w / 2).saturating_sub(width(&mid) / 2), &mid);
    }
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
            let tone = v.edge(next);
            for (n, label) in labels.iter().enumerate() {
                g.put(
                    n,
                    c + 2 + width(label),
                    &format!(" {}", "─".repeat(w - width(label) + 1)),
                    tone,
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
                    tone,
                );
            }
            g.put(0, x + 1, "── ", tone);
            c = x + 4;
        } else if next.len() > 1 {
            /* the trunk takes the line while a frontier job hangs below it, each branch its job's tone */
            let x = c + 2 + w + 1;
            g.put(0, x, "─┬", v.edge(next));
            g.put(0, x + 2, "─ ", v.edge([&next[0]]));
            for n in 1..next.len() {
                let trunk = if n + 1 < next.len() { "├" } else { "└" };
                g.put(n, x + 1, trunk, v.edge(&next[n..]));
                g.put(n, x + 2, "─ ", v.edge([&next[n]]));
            }
            c = x + 4;
        } else {
            let x = c + 2 + w + 1;
            g.put(0, x, "── ", v.edge(next));
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
pub(crate) fn ellipsis(text: &str, max: usize) -> String {
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

pub(crate) type Row = Vec<(String, Tone)>;

pub(crate) const KEY: usize = 8;

/* a card names at most three dependencies on one line, as in the design */
const LISTED: usize = 3;

pub(crate) fn key(k: &str, first: bool) -> (String, Tone) {
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
pub(crate) fn field(k: &str, lines: &[String], n: usize, tone: Tone, room: usize) -> Vec<Row> {
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

pub(crate) fn wrapped(
    side: &mut Vec<Row>,
    k: &str,
    text: &str,
    tone: Tone,
    max: usize,
    lines: usize,
) {
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

/* a card takes two columns from 96 columns on; how wide each is */
pub(crate) fn columns(cols: usize) -> (bool, usize) {
    let two = cols >= 96;
    let inner = cols.saturating_sub(4);
    (two, if two { inner / 2 - 1 } else { inner })
}

/* the design's card: fields on the left and, in a wide pane, the prompt and any command
on the right */
fn idle_why(why: &IdleWhy) -> &'static str {
    match why {
        IdleWhy::NotPickedUp => "not picked up",
        IdleWhy::AskStillOpen => "ask still open",
        IdleWhy::TurnEnded => "turn ended",
        IdleWhy::Unknown => "",
    }
}

/* why `name` holds an ask up, in the words the job card and the ask card share */
pub(super) fn stall_why(progress: &Progress, name: &str, now: u64) -> Option<(String, Tone)> {
    match progress {
        Progress::Gone => Some((format!("{name} left the hub"), Tone::Warn)),
        Progress::Offline => Some((format!("{name} is offline"), Tone::Warn)),
        Progress::Wait { reason, .. } => Some((
            reason
                .as_deref()
                .unwrap_or("waiting for permission")
                .to_string(),
            Tone::Wait,
        )),
        Progress::Idle { since, why } => {
            let mut text = match why {
                IdleWhy::TurnEnded => format!("{name} ended its turn"),
                IdleWhy::Unknown => format!("{name} idle"),
                _ => format!("{name}: {}", idle_why(why)),
            };
            if *since <= now {
                let age = ago(now, Some(*since));
                text.push_str(&match why {
                    IdleWhy::TurnEnded => format!(" {age} ago"),
                    _ => format!(" · {age}"),
                });
            }
            Some((text, Tone::Wait))
        }
        _ => None,
    }
}

fn gone_label<'a>(ask: Option<&'a Ask>, job: &'a Job) -> &'a str {
    ask.and_then(|ask| {
        [ask.to_peer_id.as_str(), ask.to_peer.as_str()]
            .into_iter()
            .find(|part| !part.is_empty())
    })
    .or(job.assigned_peer.as_deref())
    .filter(|part| !part.is_empty())
    .unwrap_or("-")
}

fn worker_name<'a>(snap: &'a Snapshot, job: &'a Job) -> &'a str {
    job.worker
        .as_deref()
        .map(|id| {
            snap.peer_by_id(id)
                .map(|peer| peer.name.as_str())
                .unwrap_or(id)
        })
        .unwrap_or_else(|| job.assigned_peer.as_deref().unwrap_or("-"))
}

fn now_word(snap: &Snapshot, job: &Job) -> Option<&'static str> {
    let id = job.worker.as_deref()?;
    match snap.peer_by_id(id)?.liveness.as_ref()? {
        Liveness::Work { .. } => Some(" · now WORK"),
        Liveness::Idle { .. } => Some(" · now IDLE"),
        Liveness::Wait { .. } => Some(" · now WAIT"),
        Liveness::Offline => Some(" · offline"),
        _ => None,
    }
}

pub(crate) fn job_commands(job: &Job) -> Vec<(&'static str, String)> {
    let mut commands = Vec::new();
    let id = &job.job_id;
    for action in job.actions.iter().flatten() {
        let peer = job.assigned_peer.as_deref().unwrap_or("PEER");
        let mut command = match action {
            JobAction::Retry {
                needs_assignee: false,
            } => {
                format!("amesh jobs update {} --state queued", quote(id))
            }
            JobAction::Retry {
                needs_assignee: true,
            }
            | JobAction::Send => format!(
                "amesh jobs update {} --state queued --assigned-peer {}",
                quote(id),
                quote(peer)
            ),
            JobAction::Nudge { to } => format!(
                "amesh peer notify {} {}",
                quote(to),
                quote(&format!("{}?", job.title))
            ),
            JobAction::Resend => format!(
                "amesh jobs update {} --state queued --assigned-peer PEER",
                quote(id)
            ),
            JobAction::Unknown => continue,
        };
        let key_name = match action {
            JobAction::Retry { .. } => "retry",
            JobAction::Send => "send",
            JobAction::Nudge { .. } => "nudge",
            JobAction::Resend => "resend",
            JobAction::Unknown => continue,
        };
        command.retain(|c| !c.is_control());
        commands.push((key_name, command));
    }
    commands
}

/* the key promises the worker, so its name leads whole; the route gives way, the sender
first, then the ask, then the time */
fn compact_route(name: &str, ask: &Ask, room: usize) -> String {
    let to = if ask.to_peer.is_empty() {
        &ask.to_peer_id
    } else {
        &ask.to_peer
    };
    let id = short(&ask.correlation_id);
    let tail = format!(" · {}", clock(ask.opened_at));
    let from = if ask.from_peer.is_empty() {
        String::new()
    } else {
        format!("{}▸", ask.from_peer)
    };
    [
        format!("{name} · ask {id} {from}{to}{tail}"),
        format!("{name} · ask {id} {to}{tail}"),
        format!("{name}{tail}"),
    ]
    .into_iter()
    .find(|line| width(line) <= room)
    .unwrap_or_else(|| fit(name, room))
}

/* a card's key row: y and Y copy its first two commands, then what else it offers, then how
to open or leave the full card; a compact card short of room keeps only the keys */
pub(super) fn key_row(actions: &[(&str, String)], more: &[&str], size: Fit, colw: usize) -> String {
    let line = |copy: &str, last: &str| {
        ["y", "Y"]
            .iter()
            .zip(actions)
            .map(|(k, (action, _))| format!("{k} {copy}{action}"))
            .chain(more.iter().map(|m| m.to_string()))
            .chain([last.to_string()])
            .collect::<Vec<_>>()
            .join(" · ")
    };
    match size {
        Fit::Whole => line("copy ", "esc back"),
        Fit::Rows(_) => {
            let keys = line("copy ", "⏎ full card");
            if width(&keys) <= colw {
                keys
            } else {
                fit(&line("", "⏎"), colw)
            }
        }
    }
}

pub(super) fn card_keys(text: &str) -> Row {
    text.split_inclusive(' ')
        .map(|part| {
            (
                part.to_string(),
                if matches!(part.trim(), "y" | "Y" | "a" | "⏎" | "esc") {
                    Tone::Near
                } else {
                    Tone::Dim
                },
            )
        })
        .collect()
}

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
    if cols < 2 {
        return Grid::default();
    }
    let snap = &v.snap;
    let job = snap.job(id).expect("selected job is in the snapshot");
    let ask = snap.ask(job.ask_id.as_deref());
    let now = snap.captured_at;
    let (two, colw) = columns(cols);
    let (mut left, mut right, mut commands): (Vec<Row>, Vec<Row>, Vec<Row>) =
        (Vec::new(), Vec::new(), Vec::new());
    let prompt = full_text.map_or(job.prompt.as_str(), |(p, _)| p);
    let result = full_text.map_or(job.result.as_deref(), |(_, r)| r);
    let worker = job.assigned_peer.as_deref().unwrap_or("-");
    let state = job.state.as_str();
    let progress = job.progress.as_ref();
    let unacked = state == "running" && ask.is_some_and(|a| a.open);
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
            _ if snap.missing.jobs.contains(d) => {
                vec![(format!("{} deleted", fit(d, 14)), Tone::Fail)]
            }
            _ => vec![(fit(d, 14), Tone::Dim)],
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
    let name = worker_name(snap, job);
    let badge = match (
        state,
        progress.and_then(|p| p.state.as_ref()),
        &job.dispatch_state,
    ) {
        ("failed", _, _) => Some((" FAILED ", Tone::PillFail)),
        ("running", Some(Progress::Wait { .. }), _) => Some((" WAIT ", Tone::Badge)),
        ("running", Some(Progress::Idle { .. }), _) => Some((" IDLE ", Tone::PillWait)),
        ("queued", _, Some(DispatchState::Blocked { .. })) => Some((" BLOCKED ", Tone::PillText)),
        _ => None,
    };
    let relation_text = job.ask_id.as_deref().and_then(|cid| match job.relation {
        Some(JobRelation::Missing { will_fail: true }) => Some(format!(
            "{cid} is missing: the hub fails this job at its next check"
        )),
        Some(JobRelation::Missing { will_fail: false }) => Some(format!(
            "{cid} is missing; automatic settlement is disabled"
        )),
        Some(JobRelation::CleanedUp) => Some(format!("{cid} was cleaned up")),
        Some(JobRelation::Closed) if state == "running" => {
            Some("automatic settlement is disabled".into())
        }
        _ => None,
    });
    let dispatch_text = if state == "queued" {
        match &job.dispatch_state {
            Some(DispatchState::Held) => Some("held: set its assignee again to send it".into()),
            Some(DispatchState::Unassigned) => {
                Some("not dispatched: name an assignee to send it".into())
            }
            Some(DispatchState::NoPeer { name }) => Some(format!("no peer named {name}: not sent")),
            Some(DispatchState::OtherCircle { .. }) => {
                Some(format!("{worker} is in another circle: not sent"))
            }
            _ => None,
        }
    } else {
        None
    };
    let why = match (
        state,
        progress.and_then(|p| p.state.as_ref()),
        &job.dispatch_state,
    ) {
        ("done", _, _) => result
            .filter(|r| !r.is_empty())
            .map(|r| (r.to_string(), Tone::Done)),
        ("failed", _, _) => Some((
            result
                .filter(|r| !r.is_empty())
                .unwrap_or("no reason given")
                .to_string(),
            Tone::Fail,
        )),
        ("running", Some(p @ Progress::Gone), _) => stall_why(p, gone_label(ask, job), now),
        ("running", Some(p @ (Progress::Wait { .. } | Progress::Idle { .. })), _) => {
            stall_why(p, name, now)
        }
        (
            "queued",
            _,
            Some(DispatchState::Blocked {
                dependency, reason, ..
            }),
        ) => {
            let label = match (v.num.get(dependency), snap.job(dependency)) {
                (Some(n), Some(dep)) => format!(
                    "{n} {}",
                    fit(
                        &dep.title,
                        share(1).min(colw.saturating_sub(KEY + width(&format!("{n} cancelled: "))))
                    )
                ),
                _ => dependency.clone(),
            };
            Some((
                match reason {
                    BlockReason::Failed => format!("{label} failed: retry it first"),
                    BlockReason::Cancelled => format!("{label} cancelled: retry it first"),
                    BlockReason::Deleted => format!("{label} deleted: update dependencies"),
                    BlockReason::Unknown => format!("{label}: blocked"),
                },
                Tone::Dim,
            ))
        }
        _ => relation_text
            .clone()
            .or_else(|| dispatch_text.clone())
            .map(|text| (text, Tone::Dim)),
    };
    let lead = why.as_ref().map(|(text, tone)| {
        (
            if state == "done" { "result" } else { "why" },
            text.as_str(),
            *tone,
        )
    });
    let mut waiting = vec![("unacked ".into(), Tone::Dim), (ago(now, sent), Tone::Wait)];
    if unacked && !v.chain.loose {
        let mut durations: Vec<_> = v
            .chain
            .topo()
            .iter()
            .filter_map(|id| {
                let j = snap.job(id)?;
                if j.state != "done" {
                    return None;
                }
                j.finished_at?
                    .checked_sub(snap.ask(j.ask_id.as_deref())?.opened_at?)
            })
            .collect();
        durations.sort_unstable();
        if durations.len() >= 2 {
            let hi = durations[durations.len() / 2];
            let lo = durations[(durations.len() - 1) / 2];
            waiting.push((
                format!(" · chain median {}", ago(lo + (hi - lo) / 2, Some(0))),
                Tone::Dim,
            ));
        }
    }
    /* Job.worker is a peer id. Null with no derived fields is the v1 assignee line */
    let worker_block = left.len();
    let bare = job.worker.is_none() && job.relation.is_none() && job.progress.is_none();
    if matches!(job.relation, Some(JobRelation::Missing { .. })) || bare {
        left.push(vec![key("worker", true), (worker.to_string(), Tone::Soft)]);
    } else if state == "queued" {
        left.push(vec![
            key("worker", true),
            (format!("{worker} (when ready)"), Tone::Dim),
        ]);
    } else if state == "running" {
        match progress.and_then(|p| p.state.as_ref()) {
            Some(Progress::Gone) if ask.is_some() => wrapped(
                &mut left,
                "worker",
                &format!("{} left the hub", gone_label(ask, job)),
                Tone::Warn,
                colw,
                usize::MAX,
            ),
            Some(Progress::Wait { since, reason }) => {
                left.push(vec![
                    key("worker", true),
                    (format!("{name} "), Tone::Plain),
                    (format!("WAIT! {}", ago(now, Some(*since))), Tone::Warn),
                ]);
                wrapped(
                    &mut left,
                    "",
                    reason.as_deref().unwrap_or("needs your permission"),
                    Tone::Warn,
                    colw,
                    2,
                );
            }
            Some(Progress::Idle { .. }) => left.push(vec![
                key("worker", true),
                (format!("{name} "), Tone::Plain),
                ("IDLE".into(), Tone::Wait),
            ]),
            Some(Progress::Work { since }) if progress.is_some_and(|p| p.busy) => left.push(vec![
                key("worker", true),
                (format!("{name} "), Tone::Plain),
                (format!("WORK · turn {}", ago(now, Some(*since))), Tone::Run),
            ]),
            Some(Progress::Work { .. }) => left.push(vec![
                key("worker", true),
                (format!("{name} · now WORK"), Tone::Soft),
            ]),
            Some(Progress::Offline) => left.push(vec![
                key("worker", true),
                (format!("{name} · offline"), Tone::Soft),
            ]),
            _ => {
                let line = now_word(snap, job)
                    .map(|word| format!("{name}{word}"))
                    .unwrap_or_else(|| name.to_string());
                left.push(vec![key("worker", true), (line, Tone::Soft)]);
            }
        }
    } else {
        let line = now_word(snap, job)
            .map(|word| format!("{name}{word}"))
            .unwrap_or_else(|| name.to_string());
        left.push(vec![key("worker", true), (line, Tone::Soft)]);
    }
    /* why the open ask may go unseen, on a row of its own so a halved card keeps it whole */
    if let (Some(id), true) = (
        job.worker.as_deref(),
        matches!(job.relation, Some(JobRelation::Open)),
    ) {
        if let Some(p) = snap.peer_by_id(id) {
            let words = delivery(snap, p);
            if !words.is_empty() && left.len() > worker_block {
                left.push([vec![key("", false)], words].concat());
            }
        }
    }
    if let Some(ask) = ask {
        let (word, tone) = super::asks::state(snap, ask);
        let (tail, tone) = match job.relation {
            Some(JobRelation::Settling) => (format!("{word}, settling"), Tone::Warn),
            Some(JobRelation::Closed) if state == "running" => (
                format!("{word}; automatic settlement is disabled"),
                Tone::Warn,
            ),
            _ if state == "queued" => ("previous attempt".into(), Tone::Dim),
            _ => (word, tone),
        };
        let to = if ask.to_peer.is_empty() {
            &ask.to_peer_id
        } else {
            &ask.to_peer
        };
        let mut route = vec![(format!("{} ", short(&ask.correlation_id)), Tone::Plain)];
        if !ask.from_peer.is_empty() {
            let working =
                progress.is_some_and(|p| p.busy && !matches!(p.state, Some(Progress::Offline)));
            let mark = arrow(snap, ask, working);
            /* the sender gives way first, so the recipient stays whole */
            let room = colw.saturating_sub(KEY + width(&route[0].0) + width(&mark.0) + width(to));
            if room > 0 {
                route.push((fit(&ask.from_peer, room), Tone::Plain));
            }
            route.push(mark);
        }
        route.push((to.clone(), Tone::Plain));
        let mut parts = vec![
            route,
            vec![(format!("· {}", clock(ask.opened_at)), Tone::Plain)],
        ];
        if !unacked {
            parts.push(vec![("· ".into(), Tone::Plain), (tail, tone)]);
        }
        packed(&mut left, "ask", parts, colw);
        if unacked {
            left.push([vec![key("", false)], waiting.clone()].concat());
        }
    } else if let Some(text) = &relation_text {
        wrapped(&mut left, "ask", text, Tone::Warn, colw, usize::MAX);
    }
    if state == "queued" {
        match job.dispatch_state.clone() {
            Some(DispatchState::Waiting { dependencies }) => {
                listed(
                    &mut left,
                    "waits",
                    items(&by_number(dependencies)),
                    colw,
                    "  ",
                );
            }
            Some(DispatchState::Blocked {
                dependencies,
                dependency,
                reason,
            }) => {
                listed(
                    &mut left,
                    "waits",
                    items(&by_number(dependencies)),
                    colw,
                    "  ",
                );
                /* the reason is the hub's; one this build does not know claims nothing */
                let label = match snap.job(&dependency) {
                    Some(dep) => format!(
                        "{} {}",
                        v.num.get(&dependency).copied().unwrap_or(0),
                        fit(&dep.title, share(1))
                    ),
                    None => fit(&dependency, 14),
                };
                let (text, tone) = match reason {
                    BlockReason::Deleted => (
                        format!(
                            "{} was deleted: this job cannot start",
                            fit(&dependency, 14)
                        ),
                        Tone::Fail,
                    ),
                    BlockReason::Failed => (format!("{label} failed: retry it first"), Tone::Fail),
                    BlockReason::Cancelled => {
                        (format!("{label} cancelled: retry it first"), Tone::Fail)
                    }
                    BlockReason::Unknown => (label, Tone::Dim),
                };
                wrapped(&mut left, "blocked", &text, tone, colw, usize::MAX);
            }
            Some(
                DispatchState::Held
                | DispatchState::Unassigned
                | DispatchState::NoPeer { .. }
                | DispatchState::OtherCircle { .. },
            ) => {
                if let Some(text) = &dispatch_text {
                    wrapped(&mut left, "waits", text, Tone::Dim, colw, usize::MAX);
                }
            }
            Some(DispatchState::Ready { .. }) => wrapped(
                &mut left,
                "waits",
                "nothing: will be sent next tick",
                Tone::Done,
                colw,
                usize::MAX,
            ),
            _ => {}
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
    let actions = job_commands(job);
    for (key_name, command) in &actions {
        wrapped(
            &mut commands,
            key_name,
            command,
            Tone::Dim,
            colw,
            usize::MAX,
        );
    }
    let room = colw.saturating_sub(KEY);
    let keys = key_row(&actions, &[], size, colw);
    if matches!(size, Fit::Rows(1 | 2)) {
        let mut grid = Grid::default();
        let mut col = 0;
        for (text, tone) in card_keys(&fit(&keys, cols)) {
            col = grid.put(0, col, &text, tone);
        }
        return grid;
    }
    let keys = wrap(&keys, colw);
    match size {
        Fit::Whole => {
            if let Some((k, text, tone)) = lead {
                left.splice(0..0, field(k, &wrap(text, room), usize::MAX, tone, room));
            }
            let side = if two { &mut right } else { &mut left };
            side.extend(field(
                "prompt",
                &wrap(prompt, room),
                usize::MAX,
                Tone::Plain,
                room,
            ));
            side.extend(commands);
            left.resize(left.len().max(right.len()), Vec::new());
        }
        Fit::Rows(h) => {
            /* the compact card keeps none of the full card's rows */
            left.clear();
            let why_lines = lead
                .map(|(_, text, _)| wrap(text, room))
                .unwrap_or_default();
            let prompt_lines = wrap(prompt, room);
            let fixed = 1 + usize::from(unacked);
            let minimum = why_lines.len().min(2) + fixed + usize::from(!two);
            let height = if h == 0 {
                minimum
            } else {
                h.saturating_sub(2 + keys.len())
            };
            let mut why_rows = why_lines.len().min(2).min(height);
            let worker_rows = usize::from(height > why_rows);
            let waiting_rows = usize::from(unacked && height > why_rows + worker_rows);
            let fixed = worker_rows + waiting_rows;
            why_rows += (why_lines.len() - why_rows).min(
                (height - why_rows - fixed)
                    .saturating_sub(usize::from(!two && !prompt_lines.is_empty())),
            );
            let prompt_rows = prompt_lines.len().min(if two {
                height
            } else {
                height - why_rows - fixed
            });
            if let Some((k, _, tone)) = lead {
                left.extend(field(k, &why_lines, why_rows, tone, room));
            }
            if worker_rows > 0 {
                let route = ask.map_or_else(|| fit(name, room), |a| compact_route(name, a, room));
                left.push(vec![key("worker", true), (route, Tone::Soft)]);
            }
            if waiting_rows > 0 {
                left.push([vec![key("", false)], waiting].concat());
            }
            let side = if two { &mut right } else { &mut left };
            side.extend(field(
                "prompt",
                &prompt_lines,
                prompt_rows,
                Tone::Plain,
                room,
            ));
            left.resize(height, Vec::new());
        }
    }
    left.extend(keys.into_iter().map(|line| card_keys(&line)));
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
    let badge_w = badge.map_or(0, |(text, _)| width(text));
    let room = cols.saturating_sub(4 + bare + badge_w);
    let keep_stage = width(&stage) + width(&job.title).min(16) <= room;
    let title = fit(
        &job.title,
        room.saturating_sub(if keep_stage { width(&stage) } else { 0 }),
    );
    let mut head = format!(" {} {title} · {state_text} ", v.num[id]);
    if keep_stage {
        head.push_str(&stage);
    }
    framed(glyph(state), &head, badge, &left, &right, cols)
}

/* a card's frame: the glyph, the head and a badge at its right end in the top border, the
left rows and, in a wide pane, the right rows beside them */
pub(crate) fn framed(
    mark: (char, Tone),
    head: &str,
    badge: Option<(&str, Tone)>,
    left: &[Row],
    right: &[Row],
    cols: usize,
) -> Grid {
    if cols < 2 {
        return Grid::default();
    }
    let (two, colw) = columns(cols);
    let badge_w = badge.map_or(0, |(text, _)| width(text));
    let mut g = Grid::default();
    g.put(0, 0, "┌", Tone::Line);
    g.put(0, 1, &mark.0.to_string(), mark.1);
    let at = g.put(
        0,
        2,
        &fit(head, cols.saturating_sub(4 + badge_w)),
        Tone::Title,
    );
    g.put(
        0,
        at,
        &format!("{}┐", "─".repeat(cols.saturating_sub(at + 1))),
        Tone::Line,
    );
    if let Some((text, tone)) = badge {
        g.put(0, cols.saturating_sub(1 + badge_w), text, tone);
    }
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
