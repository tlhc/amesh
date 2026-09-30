use super::layout::{
    self, ago, clock, field, fit, key, short, width, wrap, Fit, Grid, Row, Tone, KEY,
};
use super::model::{Activity, Ask, Peer, Snapshot};

/* a preview narrower than this tells nothing, so the row leaves it out */
const PREVIEW_MIN: usize = 16;
/* reply lines the card shows under the list; the full card shows them all */
const REPLY_ROWS: usize = 4;

/* the asks no job points at, as the screen lists them: open first, the longest waiting on
top and those from before the hub kept the time last; then the closed, the latest first */
pub(crate) fn order(snap: &Snapshot) -> Vec<&Ask> {
    let mut asks = snap.asks_outside_jobs();
    let key = |ask: &Ask| match ask.open {
        true => (false, ask.opened_at.is_none(), ask.opened_at.unwrap_or(0)),
        false => (
            true,
            ask.closed_at.is_none(),
            u64::MAX - ask.closed_at.unwrap_or(0),
        ),
    };
    asks.sort_by(|a, b| {
        key(a)
            .cmp(&key(b))
            .then_with(|| a.correlation_id.cmp(&b.correlation_id))
    });
    asks
}

/* whether the view holds asks no job points at, which the job screen then points to */
pub(crate) fn held(snap: &Snapshot) -> bool {
    snap.capabilities.ask_list && !snap.asks_outside_jobs().is_empty()
}

/* how many of them wait for an answer, for the job screen; nothing while none does */
pub(crate) fn tag(snap: &Snapshot) -> Option<String> {
    let n = snap
        .asks_outside_jobs()
        .iter()
        .filter(|ask| ask.open)
        .count();
    (snap.capabilities.ask_list && n > 0)
        .then(|| format!("{n} ask{} open", if n == 1 { "" } else { "s" }))
}

fn failed(ask: &Ask) -> bool {
    ask.failed || ask.closed_by.as_deref() == Some("hub")
}

pub(crate) fn header(order: &[&Ask], all: bool) -> String {
    let open = order.iter().filter(|ask| ask.open).count();
    let failed = order.iter().filter(|ask| !ask.open && failed(ask)).count();
    let scope = if all { " · all circles" } else { "" };
    let mut head = format!(
        "asks{scope} · {open} open · {} answered",
        order.len() - open - failed
    );
    if failed > 0 {
        head.push_str(&format!(" · {failed} failed"));
    }
    head
}

fn glyph(ask: &Ask) -> (char, Tone) {
    match (ask.open, failed(ask)) {
        (true, _) => ('◆', Tone::Run),
        (false, true) => ('×', Tone::Fail),
        (false, false) => ('●', Tone::Done),
    }
}

/* the recipient by id: a name may have passed to another peer since */
fn recipient<'a>(snap: &'a Snapshot, ask: &Ask) -> Option<&'a Peer> {
    snap.peer_by_id(&ask.to_peer_id)
}

/* what the recipient of an open ask is doing, from hubs that report it */
fn doing<'a>(snap: &'a Snapshot, ask: &Ask) -> Option<&'a Activity> {
    recipient(snap, ask)
        .filter(|p| ask.open && p.status == "online" && snap.capabilities.peer_activity)
        .and_then(|p| p.activity.as_ref())
}

/* the job card's words for an ask; idle means the recipient's turn ended with the ask open */
pub(crate) fn state(snap: &Snapshot, ask: &Ask) -> (String, Tone) {
    let now = snap.captured_at;
    if ask.open {
        return match (recipient(snap, ask), doing(snap, ask)) {
            (None, _) => ("left the hub".into(), Tone::Fail),
            (Some(p), _) if p.status != "online" => ("offline".into(), Tone::Fail),
            (_, Some(a)) if a.state == "idle" => {
                (format!("IDLE! {}", ago(now, Some(a.since))), Tone::Fail)
            }
            (_, Some(a)) if a.state == "wait" => {
                (format!("WAIT! {}", ago(now, Some(a.since))), Tone::Wait)
            }
            _ => (format!("waiting {}", ago(now, ask.opened_at)), Tone::Run),
        };
    }
    let outcome = |ok: &str, bad: &str| match ask.failed {
        true => (bad.to_string(), Tone::Fail),
        false => (ok.to_string(), Tone::Done),
    };
    match ask.closed_by.as_deref() {
        Some("recipient") => outcome("acked ok", "acked failed"),
        Some("hand") => outcome("closed by hand", "closed by hand"),
        Some("hub") => ("closed by hub".into(), Tone::Fail),
        _ => outcome("closed ok", "closed failed"),
    }
}

/* a peer of the view's own circle, by id or name; anonymous stands for no sender */
fn own(snap: &Snapshot, name: &str) -> bool {
    name != "anonymous"
        && snap
            .roster
            .iter()
            .any(|peer| peer.peer_id == name || peer.name == name)
}

/* the prefix the view's own peers share up to a '-': the {folder}- of {folder}-{backend} ids,
which tells nothing apart inside a circle */
fn prefix(snap: &Snapshot, order: &[&Ask]) -> String {
    let mut names: Vec<&str> = order
        .iter()
        .flat_map(|ask| [ask.from_peer.as_str(), ask.to_peer_id.as_str()])
        .filter(|name| own(snap, name))
        .collect();
    names.sort_unstable();
    names.dedup();
    let [first, .., last] = names.as_slice() else {
        return String::new();
    };
    let same = first
        .char_indices()
        .zip(last.chars())
        .take_while(|((_, a), b)| a == b)
        .last()
        .map_or(0, |((at, ch), _)| at + ch.len_utf8());
    let prefix = first[..same].rfind('-').map_or("", |cut| &first[..=cut]);
    match names.iter().all(|name| name.len() > prefix.len()) {
        true => prefix.to_string(),
        false => String::new(),
    }
}

/* the view's own peers without the prefix; a name from another circle, a peer that left and
anonymous stay whole */
fn shown(snap: &Snapshot, name: &str, prefix: &str) -> String {
    match name.strip_prefix(prefix) {
        Some(rest) if !prefix.is_empty() && own(snap, name) => format!("…{rest}"),
        _ => name.to_string(),
    }
}

/* cut to `max` columns keeping the end, where the recipient that tells the rows apart is */
fn fit_end(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    let mut tail: Vec<char> = Vec::new();
    let mut used = 0;
    for ch in text.chars().rev() {
        let w = width(&ch.to_string());
        if used + w + 1 > max {
            break;
        }
        tail.push(ch);
        used += w;
    }
    tail.reverse();
    format!("…{}", tail.into_iter().collect::<String>())
}

fn pad(text: &str, to: usize) -> String {
    format!("{text}{}", " ".repeat(to.saturating_sub(width(text))))
}

/* a text on one line: its line breaks and runs of spaces as single spaces */
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/* one row per ask: number, glyph, id, sender>recipient, state and, where it fits, the text or
the reply; the route gives way to the state */
pub(crate) fn rows(snap: &Snapshot, order: &[&Ask], cols: usize) -> Vec<Row> {
    let prefix = prefix(snap, order);
    let routes: Vec<(String, (String, Tone), String)> = order
        .iter()
        .map(|ask| {
            let working = doing(snap, ask).is_some_and(|a| a.state == "work");
            (
                shown(snap, &ask.from_peer, &prefix),
                layout::arrow(snap, ask, working),
                shown(snap, &ask.to_peer_id, &prefix),
            )
        })
        .collect();
    let whole =
        |(from, (mark, _), to): &(String, (String, Tone), String)| format!("{from}{mark}{to}");
    let states: Vec<(String, Tone)> = order.iter().map(|ask| state(snap, ask)).collect();
    let digits = order.len().to_string().len();
    let sw = states
        .iter()
        .map(|(text, _)| width(text))
        .max()
        .unwrap_or(0);
    let rw = routes
        .iter()
        .map(|route| width(&whole(route)))
        .max()
        .unwrap_or(0)
        .min(cols.saturating_sub(digits + 8 + 2 + sw));
    order
        .iter()
        .zip(routes)
        .zip(states)
        .enumerate()
        .map(|(i, ((ask, route), (st, tone)))| {
            let (ch, gt) = glyph(ask);
            let mut row: Row = vec![
                (format!("{:>digits$} ", i + 1), Tone::Dim),
                (ch.to_string(), gt),
                (format!(" {} ", short(&ask.correlation_id)), Tone::Dim),
            ];
            /* a route cut to fit keeps its end and reads > */
            if width(&whole(&route)) <= rw {
                let (from, (mark, flow), to) = route;
                let rest = rw - width(&from) - width(&mark);
                row.extend([
                    (from, Tone::Plain),
                    (mark, flow),
                    (pad(&to, rest), Tone::Plain),
                ]);
            } else {
                let (from, _, to) = route;
                row.push((pad(&fit_end(&format!("{from}>{to}"), rw), rw), Tone::Plain));
            }
            row.extend([("  ".into(), Tone::Plain), (pad(&st, sw), tone)]);
            let used: usize = row.iter().map(|(text, _)| width(text)).sum();
            let rest = cols.saturating_sub(used + 2);
            if rest >= PREVIEW_MIN {
                let text = match ask.reply.as_deref().filter(|r| !ask.open && !r.is_empty()) {
                    Some(reply) => format!("→ {}", one_line(reply)),
                    None => one_line(&ask.text),
                };
                row.push((format!("  {}", fit(&text, rest)), Tone::Dim));
            }
            row
        })
        .collect()
}

/* a name and what its peer does now; the status takes a line of its own when both do not fit,
and a name wider than the card wraps, so no id is cut */
fn party(k: &str, name: &str, status: Row, room: usize) -> Vec<Row> {
    let mut lines: Vec<Row> = wrap(name, room)
        .into_iter()
        .enumerate()
        .map(|(i, part)| vec![key(k, i == 0), (part, Tone::Plain)])
        .collect();
    if status.is_empty() {
        return lines;
    }
    let w: usize = status.iter().map(|(t, _)| width(t)).sum();
    let last = lines.last_mut().expect("wrap gives a line");
    let used: usize = last[1..].iter().map(|(t, _)| width(t)).sum();
    if used + 1 + w <= room {
        last.push((" ".into(), Tone::Plain));
        last.extend(status);
    } else {
        lines.push([vec![key(k, false)], status].concat());
    }
    lines
}

fn now_doing(snap: &Snapshot, peer: Option<&Peer>) -> Row {
    match peer {
        None => vec![("· left the hub".into(), Tone::Fail)],
        Some(p) if p.status != "online" => vec![("· offline".into(), Tone::Fail)],
        Some(p) => p
            .activity
            .as_ref()
            .filter(|_| snap.capabilities.peer_activity)
            .map_or(Vec::new(), |a| {
                vec![(format!("· now {}", a.state.to_uppercase()), Tone::Soft)]
            }),
    }
}

/* the selected ask's card in the job card's frame: the reply first once there is one, who
asked whom and what each does now, the times, the question, and the command a stalled answer
calls for */
pub(crate) fn card(
    snap: &Snapshot,
    ask: &Ask,
    num: usize,
    cols: usize,
    full_text: Option<(&str, Option<&str>)>,
    size: Fit,
) -> Grid {
    let now = snap.captured_at;
    let (two, colw) = layout::columns(cols);
    let room = colw.saturating_sub(KEY);
    let text = full_text.map_or(ask.text.as_str(), |(t, _)| t);
    let reply = full_text.map_or(ask.reply.as_deref(), |(_, r)| r);
    let (mut left, mut right, mut commands): (Vec<Row>, Vec<Row>, Vec<Row>) =
        (Vec::new(), Vec::new(), Vec::new());
    left.extend(party(
        "from",
        &ask.from_peer,
        now_doing(snap, snap.peer(&ask.from_peer)),
        room,
    ));
    let to = recipient(snap, ask);
    let status = match doing(snap, ask) {
        Some(a) if a.state == "idle" => vec![
            (format!("IDLE! {}", ago(now, Some(a.since))), Tone::Fail),
            (" · turn ended".into(), Tone::Dim),
        ],
        Some(a) if a.state == "wait" => {
            vec![(format!("WAIT! {}", ago(now, Some(a.since))), Tone::Wait)]
        }
        _ => now_doing(snap, to),
    };
    left.extend(party("to", &ask.to_peer_id, status, room));
    let times = match (ask.open, ask.opened_at) {
        (true, None) => "sent before the hub kept the time".to_string(),
        (true, Some(t)) => format!("sent {} · waiting {}", clock(Some(t)), ago(now, Some(t))),
        (false, t) => format!(
            "sent {} · closed {} · took {}",
            clock(t),
            clock(ask.closed_at),
            ask.closed_at.map_or("--".into(), |end| ago(end, t))
        ),
    };
    layout::wrapped(&mut left, "times", &times, Tone::Dim, colw, usize::MAX);
    if ask.open && to.is_none() {
        let command = format!(
            "amesh peer ack {} --failed true --message {}",
            layout::quote(&ask.correlation_id),
            layout::quote("recipient left")
        );
        layout::wrapped(
            &mut commands,
            "close",
            &command,
            Tone::Dim,
            colw,
            usize::MAX,
        );
    } else if doing(snap, ask).is_some_and(|a| a.state == "idle") {
        let command = format!(
            "amesh peer notify {} {}",
            layout::quote(&ask.to_peer_id),
            layout::quote(&format!(
                "ask {} waits for your ack",
                short(&ask.correlation_id)
            ))
        );
        layout::wrapped(
            &mut commands,
            "nudge",
            &command,
            Tone::Dim,
            colw,
            usize::MAX,
        );
    }
    if !ask.open {
        let lines = wrap(reply.filter(|r| !r.is_empty()).unwrap_or("-"), room);
        let n = match size {
            Fit::Whole => lines.len(),
            Fit::Rows(_) => lines.len().min(REPLY_ROWS),
        };
        let tone = if failed(ask) { Tone::Fail } else { Tone::Done };
        left.splice(0..0, field("reply", &lines, n, tone, room));
    }
    let lines = wrap(text, room);
    let n = match size {
        Fit::Whole => lines.len(),
        Fit::Rows(h) if two => lines.len().min(h.saturating_sub(2 + commands.len())).max(1),
        Fit::Rows(h) => lines
            .len()
            .min(h.saturating_sub(2 + left.len() + commands.len()))
            .max(1),
    };
    let side = if two { &mut right } else { &mut left };
    side.extend(field("text", &lines, n, Tone::Plain, room));
    side.extend(commands);
    if let Fit::Rows(h) = size {
        while 2 + left.len().max(right.len()) < h {
            left.push(Vec::new());
        }
    }
    let (word, _) = state(snap, ask);
    let head_state = match (ask.open, ask.opened_at, ask.closed_at) {
        (true, None, _) => "open, age unknown".to_string(),
        (true, Some(t), _) => format!("waiting {}", ago(now, Some(t))),
        (false, Some(a), Some(b)) => format!("{word} · took {}", ago(b, Some(a))),
        (false, ..) => word,
    };
    let head = format!(" {num} ask {} · {head_state} ", short(&ask.correlation_id));
    layout::framed(glyph(ask), &head, &left, &right, cols)
}
