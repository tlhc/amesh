use super::asks;
use super::layout::{
    self, brief, clock, fit, key, short, stamp, width, wrap, Fit, Grid, Row, Tone, KEY,
};
use super::model::{Ask, Event, Snapshot};

/* events the screen holds at most, as many as the hub keeps */
pub(crate) const KEEP: usize = 500;

const ABOVE: &str = "the rows above came from the previous hub process";

/* what the events screen holds, oldest first: the hub's events, each with the hub run it was
read in, and where the hub restarted between them */
#[derive(Clone, Debug)]
pub(crate) enum Held {
    Event(u64, Box<Event>),
    Restart(u64),
}

/* a row of the list: an event with the other records of its broadcast or fan-out and its hub
run, or a restart */
pub(crate) enum Item<'a> {
    Event(u64, Vec<&'a Event>),
    Restart(u64),
}

impl Item<'_> {
    /* what the selection holds on: the row's hub run and its first record's seq, so a ring that
    numbers from below again after the clock stepped back keeps to its own rows, however the
    oldest rows and restarts go */
    pub fn key(&self) -> Option<(u64, u64)> {
        match self {
            Item::Event(ring, group) => Some((*ring, group[0].seq?)),
            Item::Restart(_) => None,
        }
    }
}

/* the newest seq held since the last restart: a ring that started over is all new */
pub(crate) fn newest(held: &[Held]) -> Option<u64> {
    held.iter()
        .rev()
        .map_while(|h| match h {
            Held::Event(_, e) => Some(e.seq),
            Held::Restart(_) => None,
        })
        .flatten()
        .max()
}

/* the events held since the last restart came from a hub that numbers none; an earlier ring
says nothing about this one */
pub(crate) fn unnumbered(held: &[Held]) -> bool {
    held.iter()
        .rev()
        .take_while(|h| matches!(h, Held::Event(..)))
        .any(|h| matches!(h, Held::Event(_, e) if e.seq.is_none()))
}

pub(crate) fn holds(held: &[Held], key: (u64, u64)) -> bool {
    items(held).iter().any(|item| item.key() == Some(key))
}

/* the oldest go once more than `keep` events are held, and a restart with nothing before it
goes with them */
pub(crate) fn trim(held: &mut Vec<Held>, keep: usize) {
    let mut over = held
        .iter()
        .filter(|h| matches!(h, Held::Event(..)))
        .count()
        .saturating_sub(keep);
    while !held.is_empty() && (over > 0 || matches!(held[0], Held::Restart(_))) {
        if matches!(held.remove(0), Held::Event(..)) {
            over -= 1;
        }
    }
}

/* the rows of the list: the records of one broadcast are one row, and so are the asks one
sender sent with one text in one second, the records of a fan-out */
pub(crate) fn items(held: &[Held]) -> Vec<Item<'_>> {
    let together = |a: &Event, b: &Event| {
        a.kind == b.kind
            && match a.kind.as_str() {
                "broadcast" => !a.id.is_empty() && a.id == b.id,
                "ask" => {
                    a.at.is_some() && (a.at, &a.from_peer, &a.text) == (b.at, &b.from_peer, &b.text)
                }
                _ => false,
            }
    };
    let mut out: Vec<Item> = Vec::new();
    for h in held {
        match h {
            Held::Restart(at) => out.push(Item::Restart(*at)),
            Held::Event(ring, e) => {
                let e: &Event = e;
                let joins =
                    matches!(out.last(), Some(Item::Event(_, group)) if together(group[0], e));
                if joins {
                    if let Some(Item::Event(_, group)) = out.last_mut() {
                        group.push(e);
                    }
                } else {
                    out.push(Item::Event(*ring, vec![e]));
                }
            }
        }
    }
    out
}

pub(crate) fn header(held: &[Held], all: bool) -> String {
    let events: Vec<&Event> = held
        .iter()
        .filter_map(|h| match h {
            Held::Event(_, e) => Some(&**e),
            Held::Restart(_) => None,
        })
        .collect();
    let scope = if all { " · all circles" } else { "" };
    let head = format!("events{scope} · {} kept", events.len());
    match events.iter().filter_map(|e| e.at).max() {
        Some(at) => format!("{head} · newest {}", stamp(Some(at))),
        None => head,
    }
}

/* an event's kind; the hub's own notices read hub */
fn kind(e: &Event) -> &str {
    match (e.kind.as_str(), e.from_peer.as_str()) {
        ("notify", "amesh") => "hub",
        (kind, _) => kind,
    }
}

/* an event's text and the glyph and tone of its kind. An ack is × when its ask failed, which
the hub says in `failed`; an older hub says it only by starting the text with "[failed]", the
form runtimes read because they render the text alone. A failed ack loses that marker and the
space after it; any other ack is ● */
fn body(e: &Event) -> (&str, (char, Tone)) {
    let text = if e.text.is_empty() {
        &e.message
    } else {
        &e.text
    };
    let marked = text.strip_prefix("[failed]");
    match kind(e) {
        "ack" if e.failed.unwrap_or(marked.is_some()) => (
            marked.map_or(text, |rest| rest.strip_prefix(' ').unwrap_or(rest)),
            ('×', Tone::Fail),
        ),
        "ack" => (text, ('●', Tone::Done)),
        "ask" => (text, ('•', Tone::Near)),
        "hub" => (text, ('•', Tone::Wait)),
        _ => (text, ('•', Tone::Dim)),
    }
}

/* a kind in the list's column, six wide and a space; an ack's glyph says how it ended */
fn label(kind: &str, (glyph, tone): (char, Tone)) -> Row {
    let word = match kind {
        "ack" => return vec![("ack ".into(), Tone::Dim), (format!("{glyph}  "), tone)],
        "broadcast" => "bcast",
        _ => kind,
    };
    vec![(format!("{:<6} ", fit(word, 6)), tone)]
}

fn route(group: &[&Event], prefix: &str) -> String {
    let e = group[0];
    let from = match kind(e) {
        "hub" => "",
        _ => brief(&e.from_peer, prefix),
    };
    match (e.kind.as_str(), group.len()) {
        ("chat", _) => format!("{} ({})", brief(&e.peer, prefix), e.role),
        (_, 1) => format!("{from}▸{}", brief(&e.to_peer, prefix)),
        (_, n) => format!("{from}▸{n} peers"),
    }
}

/* one row per item: the time, to the second in a wide pane, the kind, who to whom and the
text's first line, cut to fit; a restart is a rule across the pane */
pub(crate) fn rows(items: &[Item], cols: usize, prefix: &str) -> Vec<Row> {
    let wide = layout::columns(cols).0;
    items
        .iter()
        .map(|item| match item {
            Item::Restart(at) => {
                let line = match wide {
                    true => format!("─── hub restarted {} · {ABOVE} ", stamp(Some(*at))),
                    false => format!("─── hub restarted {} ", clock(Some(*at))),
                };
                let rule = "─".repeat(cols.saturating_sub(width(&line)));
                vec![(fit(&line, cols) + &rule, Tone::Line)]
            }
            Item::Event(_, group) => {
                let e = group[0];
                let (text, mark) = body(e);
                let mut row: Row = vec![(
                    format!("{} ", if wide { stamp(e.at) } else { clock(e.at) }),
                    Tone::Dim,
                )];
                row.extend(label(kind(e), mark));
                let room = cols.saturating_sub(row.iter().map(|(t, _)| width(t)).sum());
                let way = fit(&format!("{}  ", route(group, prefix)), room);
                let rest = room.saturating_sub(width(&way));
                let first = text.lines().next().unwrap_or("");
                row.push((way, Tone::Soft));
                if rest > 0 && !first.is_empty() {
                    row.push((fit(first, rest), Tone::Plain));
                }
                row
            }
        })
        .collect()
}

/* the ask a opens on the asks screen: of an ask's records, the one that screen lists first */
pub(crate) fn opens<'a>(item: &Item, snap: &'a Snapshot) -> Option<&'a Ask> {
    match item {
        Item::Event(_, group) if group[0].kind == "ask" => asks::order(snap)
            .into_iter()
            .find(|ask| group.iter().any(|e| e.correlation_id == ask.correlation_id)),
        _ => None,
    }
}

/* an ask's recipients in the order of the asks screen, each with its ask's glyph, state and id
while the snapshot holds the ask: a line each while every line fits, otherwise the name wrapped
on lines of its own and those under it. The states share a column, cut where the card leaves no
room for it and the id */
fn recipients(group: &[&Event], snap: Option<&Snapshot>, prefix: &str, room: usize) -> Vec<Row> {
    let order = snap.map(asks::order).unwrap_or_default();
    let mut group = group.to_vec();
    group.sort_by_key(|e| {
        order
            .iter()
            .position(|ask| ask.correlation_id == e.correlation_id)
            .unwrap_or(order.len())
    });
    let mut lines: Vec<(&str, Row)> = group
        .iter()
        .map(|e| {
            let status = snap.and_then(|snap| {
                let ask = snap.ask(Some(&e.correlation_id))?;
                let (glyph, tone) = asks::glyph(ask);
                let id = short(&ask.correlation_id).to_string();
                Some(vec![
                    (format!("{glyph} "), tone),
                    asks::state(snap, ask),
                    (id, Tone::Dim),
                ])
            });
            (brief(&e.to_peer, prefix), status.unwrap_or_default())
        })
        .collect();
    let name_w = lines.iter().map(|(name, _)| width(name)).max().unwrap_or(0);
    let column = |k: usize| {
        lines
            .iter()
            .filter_map(|(_, status)| status.get(k))
            .map(|(text, _)| width(text))
            .max()
            .unwrap_or(0)
    };
    let state_w = column(1).min(room.saturating_sub(2 + 2 + column(2)));
    /* the column pads in plain, so a badge or pill ends with its word */
    for (_, status) in &mut lines {
        if let Some((word, _)) = status.get_mut(1) {
            *word = fit(word, state_w);
            let pad = state_w.saturating_sub(width(word)) + 2;
            status.insert(2, (" ".repeat(pad), Tone::Plain));
        }
    }
    let inline = lines.iter().all(|(_, status)| {
        name_w + 1 + status.iter().map(|(t, _)| width(t)).sum::<usize>() <= room
    });
    lines
        .into_iter()
        .enumerate()
        .flat_map(|(i, (name, status))| {
            if inline {
                let pad = " ".repeat(name_w + 1 - width(name));
                return vec![[
                    vec![key("to", i == 0), (format!("{name}{pad}"), Tone::Soft)],
                    status,
                ]
                .concat()];
            }
            let mut rows: Vec<Row> = wrap(name, room)
                .into_iter()
                .enumerate()
                .map(|(j, part)| vec![key("to", i == 0 && j == 0), (part, Tone::Soft)])
                .collect();
            if !status.is_empty() {
                rows.push([vec![key("", false)], status].concat());
            }
            rows
        })
        .collect()
}

/* the selected row's card: who to whom, each recipient of an ask with how its ask stands, the
topic when it says more than the id, the whole text and the keys; a restart says what it means
for the rows above it. A card the pane gives no room for its frame is its keys alone */
pub(crate) fn card(item: &Item, cols: usize, size: Fit, snap: Option<&Snapshot>) -> Grid {
    let prefix = match (size, snap) {
        (Fit::Rows(_), Some(snap)) => layout::prefix(snap),
        _ => "",
    };
    let (two, colw) = layout::columns(cols);
    let room = colw.saturating_sub(KEY);
    let more: &[&str] = match snap.and_then(|snap| opens(item, snap)) {
        Some(_) => &["a open in asks"],
        None => &[],
    };
    let keys = layout::key_row(&[], more, size, colw);
    if let Fit::Rows(1 | 2) = size {
        let mut g = Grid::default();
        let mut col = 0;
        for (text, tone) in layout::card_keys(&fit(&keys, cols)) {
            col = g.put(0, col, &text, tone);
        }
        return g;
    }
    let keys: Vec<Row> = wrap(&keys, colw)
        .iter()
        .map(|line| layout::card_keys(line))
        .collect();
    let (mut left, mut right): (Vec<Row>, Vec<Row>) = (Vec::new(), Vec::new());
    let (mark, head) = match item {
        Item::Restart(at) => {
            let note = format!("{ABOVE}, which kept its events in memory only");
            layout::wrapped(&mut left, "note", &note, Tone::Dim, colw, usize::MAX);
            (
                ('─', Tone::Dim),
                format!(" hub restarted · {} ", stamp(Some(*at))),
            )
        }
        Item::Event(_, group) => {
            let e = group[0];
            let (text, mark) = body(e);
            let id = if e.correlation_id.is_empty() {
                e.id.as_str()
            } else {
                e.correlation_id.as_str()
            };
            let from = match e.kind.as_str() {
                "chat" => route(group, prefix),
                _ => brief(&e.from_peer, prefix).to_string(),
            };
            layout::wrapped(&mut left, "from", &from, Tone::Soft, colw, usize::MAX);
            match e.kind.as_str() {
                "ask" => left.extend(recipients(group, snap, prefix, room)),
                "chat" => {}
                _ => {
                    let to: Vec<&str> = group.iter().map(|r| brief(&r.to_peer, prefix)).collect();
                    layout::wrapped(
                        &mut left,
                        "to",
                        &to.join(", "),
                        Tone::Soft,
                        colw,
                        usize::MAX,
                    );
                }
            }
            if !e.topic.is_empty() && e.topic != id {
                layout::wrapped(&mut left, "topic", &e.topic, Tone::Dim, colw, usize::MAX);
            }
            let lines: Vec<String> = text.split('\n').flat_map(|part| wrap(part, room)).collect();
            let n = match size {
                Fit::Whole => lines.len(),
                Fit::Rows(h) if two => lines.len().min(h.saturating_sub(2)).max(1),
                Fit::Rows(h) => lines
                    .len()
                    .min(h.saturating_sub(2 + left.len() + keys.len()))
                    .max(1),
            };
            let side = if two { &mut right } else { &mut left };
            side.extend(layout::field("text", &lines, n, Tone::Plain, room));
            let tag = match group.len() {
                1 => id.to_string(),
                n => format!("{n} peers"),
            };
            let head = match tag.as_str() {
                "" => format!(" {} · {} ", kind(e), stamp(e.at)),
                _ => format!(" {} · {} · {tag} ", kind(e), stamp(e.at)),
            };
            (mark, head)
        }
    };
    /* the keys end the card: under the text in a full one, on its last row under the list */
    match size {
        Fit::Whole => left.resize(left.len().max(right.len()), Vec::new()),
        Fit::Rows(0) => {}
        Fit::Rows(h) => left.resize(h.saturating_sub(2 + keys.len()), Vec::new()),
    }
    left.extend(keys);
    layout::framed(mark, &head, None, &left, &right, cols)
}
