use super::layout::{self, brief, clock, fit, stamp, width, wrap, Fit, Grid, Row, Tone, KEY};
use super::model::Event;

/* events the screen holds at most, as many as the hub keeps */
pub(crate) const KEEP: usize = 500;

/* what the events screen holds, oldest first: the hub's events and where the hub restarted
between them */
#[derive(Clone, Debug)]
pub(crate) enum Held {
    Event(Box<Event>),
    Restart(u64),
}

/* a row of the list: an event with the other records of its broadcast, or a restart */
pub(crate) enum Item<'a> {
    Event(Vec<&'a Event>),
    Restart(u64),
}

impl Item<'_> {
    /* the seq the selection holds on: its first record's */
    pub fn seq(&self) -> Option<u64> {
        match self {
            Item::Event(group) => group[0].seq,
            Item::Restart(_) => None,
        }
    }
}

/* the newest seq held since the last restart: a ring that started over is all new */
pub(crate) fn newest(held: &[Held]) -> Option<u64> {
    held.iter()
        .rev()
        .map_while(|h| match h {
            Held::Event(e) => Some(e.seq),
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
        .take_while(|h| matches!(h, Held::Event(_)))
        .any(|h| matches!(h, Held::Event(e) if e.seq.is_none()))
}

pub(crate) fn holds(held: &[Held], seq: u64) -> bool {
    held.iter()
        .any(|h| matches!(h, Held::Event(e) if e.seq == Some(seq)))
}

/* the oldest go once more than `keep` events are held, and a restart with nothing before it
goes with them */
pub(crate) fn trim(held: &mut Vec<Held>, keep: usize) {
    let mut over = held
        .iter()
        .filter(|h| matches!(h, Held::Event(_)))
        .count()
        .saturating_sub(keep);
    while !held.is_empty() && (over > 0 || matches!(held[0], Held::Restart(_))) {
        if matches!(held.remove(0), Held::Event(_)) {
            over -= 1;
        }
    }
}

/* the rows of the list: the records of one broadcast are one row */
pub(crate) fn items(held: &[Held]) -> Vec<Item<'_>> {
    let mut out: Vec<Item> = Vec::new();
    for h in held {
        match h {
            Held::Restart(at) => out.push(Item::Restart(*at)),
            Held::Event(e) => {
                let e: &Event = e;
                let joins = matches!(out.last(), Some(Item::Event(group))
                    if e.kind == "broadcast" && !e.id.is_empty()
                        && group[0].kind == "broadcast" && group[0].id == e.id);
                if joins {
                    if let Some(Item::Event(group)) = out.last_mut() {
                        group.push(e);
                    }
                } else {
                    out.push(Item::Event(vec![e]));
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
            Held::Event(e) => Some(&**e),
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
    match e.kind.as_str() {
        "ack" if e.failed.unwrap_or(marked.is_some()) => (
            marked.map_or(text, |rest| rest.strip_prefix(' ').unwrap_or(rest)),
            ('×', Tone::Fail),
        ),
        "ack" => (text, ('●', Tone::Done)),
        "ask" => (text, ('•', Tone::Near)),
        "notify" | "broadcast" => (text, ('•', Tone::Plain)),
        _ => (text, ('•', Tone::Dim)),
    }
}

/* a kind in the list's column, six wide; an ack's glyph says how it ended */
fn label(kind: &str, glyph: char) -> String {
    let short = match kind {
        "broadcast" => "bcast".into(),
        "ack" => format!("ack {glyph}"),
        _ => kind.into(),
    };
    format!("{:<6}", fit(&short, 6))
}

fn route(group: &[&Event], prefix: &str) -> String {
    let e = group[0];
    let from = brief(&e.from_peer, prefix);
    match (e.kind.as_str(), group.len()) {
        ("chat", _) => format!("{} ({})", brief(&e.peer, prefix), e.role),
        (_, 1) => format!("{from}▸{}", brief(&e.to_peer, prefix)),
        (_, n) => format!("{from}▸{n} peers"),
    }
}

/* one row per item: the time, to the second in a wide pane, the kind, who to whom and the
text's first line, cut to fit */
pub(crate) fn rows(items: &[Item], cols: usize, prefix: &str) -> Vec<Row> {
    let wide = layout::columns(cols).0;
    items
        .iter()
        .map(|item| match item {
            Item::Restart(at) => vec![(
                fit(&format!("─── hub restarted {} ───", stamp(Some(*at))), cols),
                Tone::Dim,
            )],
            Item::Event(group) => {
                let e = group[0];
                let (text, (glyph, tone)) = body(e);
                let time = format!("{} ", if wide { stamp(e.at) } else { clock(e.at) });
                let kind = format!("{} ", label(&e.kind, glyph));
                let room = cols.saturating_sub(width(&time) + width(&kind));
                let way = fit(&format!("{}  ", route(group, prefix)), room);
                let rest = room.saturating_sub(width(&way));
                let first = text.lines().next().unwrap_or("");
                let mut row: Row = vec![(time, Tone::Dim), (kind, tone), (way, Tone::Plain)];
                if rest > 0 && !first.is_empty() {
                    row.push((fit(first, rest), Tone::Plain));
                }
                row
            }
        })
        .collect()
}

/* the selected row's card: who to whom, the topic when it says more than the id, and the whole
text; a restart says what it means for the rows above it */
pub(crate) fn card(item: &Item, cols: usize, size: Fit, prefix: &str) -> Grid {
    let prefix = if matches!(size, Fit::Whole) {
        ""
    } else {
        prefix
    };
    let (two, colw) = layout::columns(cols);
    let room = colw.saturating_sub(KEY);
    let (mut left, mut right): (Vec<Row>, Vec<Row>) = (Vec::new(), Vec::new());
    let (mark, head) = match item {
        Item::Restart(at) => {
            let note = "the rows above came from the previous hub process, which kept its events in memory only";
            layout::wrapped(&mut left, "note", note, Tone::Dim, colw, usize::MAX);
            (
                ('─', Tone::Dim),
                format!(" hub restarted · {} ", stamp(Some(*at))),
            )
        }
        Item::Event(group) => {
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
            if e.kind != "chat" {
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
            if !e.topic.is_empty() && e.topic != id {
                layout::wrapped(&mut left, "topic", &e.topic, Tone::Dim, colw, usize::MAX);
            }
            let lines: Vec<String> = text.split('\n').flat_map(|part| wrap(part, room)).collect();
            let n = match size {
                Fit::Whole => lines.len(),
                Fit::Rows(h) if two => lines.len().min(h.saturating_sub(2)).max(1),
                Fit::Rows(h) => lines.len().min(h.saturating_sub(2 + left.len())).max(1),
            };
            let side = if two { &mut right } else { &mut left };
            side.extend(layout::field("text", &lines, n, Tone::Plain, room));
            let head = match id {
                "" => format!(" {} · {} ", e.kind, stamp(e.at)),
                _ => format!(" {} · {} · {id} ", e.kind, stamp(e.at)),
            };
            (mark, head)
        }
    };
    if let Fit::Rows(h) = size {
        while 2 + left.len().max(right.len()) < h {
            left.push(Vec::new());
        }
    }
    layout::framed(mark, &head, None, &left, &right, cols)
}
