use super::layout::{
    self, ago, clock, field, fit, key, short, width, wrap, Fit, Grid, Row, Tone, KEY,
};
use super::model::{Ask, Frame, Snapshot};
use crate::wire::{AskAction, AskOutcome, AskState, Liveness, Progress};

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

pub(crate) fn failed(ask: &Ask) -> bool {
    matches!(
        ask.state,
        Some(AskState::Closed {
            failed_effective: true,
            ..
        })
    )
}

pub(crate) fn header(order: &[&Ask], all: bool) -> String {
    let open = order.iter().filter(|ask| ask.open).count();
    /* v1 carries no AskState, so a closed ask is not an answer and not a failure */
    if order.iter().all(|ask| ask.state.is_none()) {
        let scope = if all { " · all circles" } else { "" };
        return format!("asks{scope} · {open} open · {} closed", order.len() - open);
    }
    let failed = order.iter().filter(|ask| failed(ask)).count();
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
    match ask.state {
        Some(AskState::Open { .. }) => ('◆', Tone::Run),
        Some(AskState::Closed {
            failed_effective: true,
            ..
        }) => ('×', Tone::Fail),
        Some(AskState::Closed { .. }) => ('●', Tone::Done),
        _ if ask.open => ('◆', Tone::Run),
        _ => ('●', Tone::Plain),
    }
}

fn waiting(now: u64, opened: Option<u64>) -> (String, Tone) {
    (format!("waiting {}", ago(now, opened)), Tone::Run)
}

fn open_line(now: u64, opened: Option<u64>, progress: &Progress) -> (String, Tone) {
    match progress {
        Progress::Gone => ("left the hub".into(), Tone::Fail),
        Progress::Offline => ("offline".into(), Tone::Fail),
        Progress::Wait { since, .. } => (format!("WAIT! {}", ago(now, Some(*since))), Tone::Wait),
        Progress::Idle { since, .. } => (format!("IDLE! {}", ago(now, Some(*since))), Tone::Fail),
        Progress::Work { .. } | Progress::Pending { .. } | Progress::Unknown => {
            waiting(now, opened)
        }
    }
}

fn closed_line(outcome: &AskOutcome) -> (String, Tone) {
    match outcome {
        AskOutcome::AckedOk => ("acked ok".into(), Tone::Done),
        AskOutcome::AckedFailed => ("acked failed".into(), Tone::Fail),
        AskOutcome::HandOk => ("closed by hand".into(), Tone::Done),
        AskOutcome::HandFailed => ("closed by hand".into(), Tone::Fail),
        AskOutcome::Hub => ("closed by hub".into(), Tone::Fail),
        AskOutcome::ClosedOk => ("closed ok".into(), Tone::Done),
        AskOutcome::ClosedFailed => ("closed failed".into(), Tone::Fail),
        AskOutcome::Unknown => ("closed".into(), Tone::Plain),
    }
}

/* the job card's words for an ask */
pub(crate) fn state(snap: &Snapshot, ask: &Ask) -> (String, Tone) {
    let now = snap.captured_at;
    match ask.state.as_ref() {
        Some(AskState::Open { progress }) => open_line(now, ask.opened_at, progress),
        Some(AskState::Closed { outcome, .. }) => closed_line(outcome),
        _ if ask.open => waiting(now, ask.opened_at),
        _ => ("closed".into(), Tone::Plain),
    }
}

/* a roster id; anonymous is no sender. Display names are not identities */
fn own(snap: &Snapshot, id: &str) -> bool {
    !id.is_empty() && id != "anonymous" && snap.roster.iter().any(|peer| peer.peer_id == id)
}

fn identity<'a>(displayed: &'a str, resolved: Option<&'a str>) -> &'a str {
    resolved.filter(|id| !id.is_empty()).unwrap_or(displayed)
}

/* the prefix the view's own peers share up to a '-': the {folder}- of {folder}-{backend} ids,
which tells nothing apart inside a circle */
fn prefix(snap: &Snapshot, order: &[&Ask]) -> String {
    let mut names: Vec<&str> = order
        .iter()
        .flat_map(|ask| {
            [
                identity(ask.from_peer.as_str(), ask.from_peer_id.as_deref()),
                ask.to_peer_id.as_str(),
            ]
        })
        .filter(|id| own(snap, id))
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

fn ask_arrow(snap: &Frame, ask: &Ask) -> (String, Tone) {
    let working = matches!(
        ask.state,
        Some(AskState::Open {
            progress: Progress::Work { .. },
        })
    );
    if matches!(ask.state, Some(AskState::Open { .. })) || (ask.state.is_none() && ask.open) {
        return ("─▸─".into(), if working { Tone::Flow } else { Tone::Dim });
    }
    let just = snap.fresh && ask.closed_just_now == Some(true);
    if !just {
        return (">".into(), Tone::Plain);
    }
    if failed(ask) {
        ("─×─".into(), Tone::Fail)
    } else {
        ("─◂─".into(), Tone::Back)
    }
}

/* one row per ask: number, glyph, id, sender>recipient, state and, where it fits, the text or
the reply; the route gives way to the state */
pub(crate) fn rows(snap: &Frame, order: &[&Ask], cols: usize) -> Vec<Row> {
    let prefix = prefix(snap, order);
    let routes: Vec<(String, (String, Tone), String)> = order
        .iter()
        .map(|ask| {
            (
                shown(snap, &ask.from_peer, &prefix),
                ask_arrow(snap, ask),
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

fn liveness_word(kind: &Liveness) -> Row {
    let word = match kind {
        Liveness::Offline => return vec![("· offline".into(), Tone::Fail)],
        Liveness::Online | Liveness::Unknown => return Vec::new(),
        Liveness::Wait { .. } => "WAIT",
        Liveness::Work { .. } => "WORK",
        Liveness::Idle { .. } => "IDLE",
    };
    vec![(format!("· now {word}"), Tone::Soft)]
}

/* an unresolved party has left; a v1 snapshot claims nothing */
fn party_now(snap: &Snapshot, id: Option<&str>, neutral: bool) -> Row {
    match id.and_then(|id| snap.peer_by_id(id)) {
        Some(peer) => peer
            .liveness
            .as_ref()
            .map(liveness_word)
            .unwrap_or_default(),
        None if neutral => Vec::new(),
        None => vec![("· left the hub".into(), Tone::Fail)],
    }
}

/* the selected ask's card in the job card's frame: the reply first once there is one, who
asked whom and what each does now, the times, the question, and the command a stalled answer
calls for */
pub(crate) fn card(
    snap: &Frame,
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
        party_now(snap, ask.from_peer_id.as_deref(), ask.state.is_none()),
        room,
    ));
    let status = match ask.state.as_ref() {
        Some(AskState::Open {
            progress: Progress::Wait { since, .. },
        }) => vec![(format!("WAIT! {}", ago(now, Some(*since))), Tone::Wait)],
        Some(AskState::Open {
            progress: Progress::Idle { since, why },
        }) => vec![
            (format!("IDLE! {}", ago(now, Some(*since))), Tone::Fail),
            (format!(" · {}", layout::idle_why(why)), Tone::Dim),
        ],
        Some(AskState::Open {
            progress: Progress::Gone,
        }) => vec![("· left the hub".into(), Tone::Fail)],
        Some(AskState::Open {
            progress: Progress::Offline,
        }) => vec![("· offline".into(), Tone::Fail)],
        _ => party_now(snap, Some(&ask.to_peer_id), ask.state.is_none()),
    };
    left.extend(party("to", &ask.to_peer_id, status, room));
    if let Some(p) = snap.peer_by_id(&ask.to_peer_id).filter(|_| ask.open) {
        let words = layout::delivery(snap, p);
        if !words.is_empty() {
            left.push([vec![key("", false)], words].concat());
        }
    }
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
    for action in ask.actions.iter().flatten() {
        let command = match action {
            AskAction::CloseLeft => format!(
                "amesh peer ack {} --failed true --message {}",
                layout::quote(&ask.correlation_id),
                layout::quote("recipient left")
            ),
            AskAction::Nudge { to } => format!(
                "amesh peer notify {} {}",
                layout::quote(to),
                layout::quote(&format!(
                    "ask {} waits for your ack",
                    short(&ask.correlation_id)
                ))
            ),
            AskAction::Unknown => continue,
        };
        let key_name = match action {
            AskAction::CloseLeft => "close",
            _ => "nudge",
        };
        layout::wrapped(
            &mut commands,
            key_name,
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
