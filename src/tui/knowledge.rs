use serde::Deserialize;

use super::layout::{self, fit, width, Grid, Row, Tone, KEY};

/* the terminal's knowledge page as the hub sends it: the list row carries no body */
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct Item {
    pub id: String,
    pub circle: String,
    pub title: String,
    pub status: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct Page {
    pub captured_at: u64,
    pub total: u64,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct Detail {
    pub id: String,
    pub circle: String,
    pub title: String,
    pub body: String,
    pub applies_to: String,
    pub evidence: String,
    pub publisher_peer: String,
    pub publisher_session: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
    pub revoke_reason: Option<String>,
    pub revoked_by_peer: Option<String>,
    pub revoked_by_session: Option<String>,
    pub status: String,
}

fn tone(status: &str) -> Tone {
    match status {
        "active" => Tone::Soft,
        "expired" => Tone::Warn,
        _ => Tone::Fail,
    }
}

/* a stored timestamp in local time, so a card can be read without doing arithmetic */
fn stamp(ts: i64) -> String {
    let local = ts + layout::utc_offset(ts.max(0) as u64);
    let (days, secs) = (local.div_euclid(86_400), local.rem_euclid(86_400));
    let (y, m, d) = civil(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60
    )
}

/* Howard Hinnant's days-to-civil; no dependency, and correct for every stored row */
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/* a peer and its session; a stored empty session is missing, not a lone slash */
fn who(peer: &str, session: Option<&str>) -> String {
    format!(
        "{peer} / {}",
        session.filter(|s| !s.is_empty()).unwrap_or("-")
    )
}

/* one row: number, title, the state with its time, and the circle when the page spans them */
pub(crate) fn row(item: &Item, n: usize, now: u64, all: bool, cols: usize) -> Row {
    let when = match item.status.as_str() {
        "active" => format!(
            "left {}",
            layout::ago(item.expires_at.max(0) as u64, Some(now))
        ),
        "expired" => format!(
            "expired {}",
            layout::ago(now, Some(item.expires_at.max(0) as u64))
        ),
        _ => format!(
            "revoked {}",
            layout::ago(now, item.revoked_at.map(|at| at.max(0) as u64))
        ),
    };
    /* a page holds at most fifty rows, so two columns align every number */
    let mut row: Row = vec![(format!("{n:>2} "), Tone::Dim)];
    let status_w = width(&when);
    let spare = cols.saturating_sub(3 + status_w + 2);
    let circle = all.then(|| fit(&item.circle, 20.min(spare / 2)));
    if let Some(circle) = &circle {
        row.push((format!("{circle} "), Tone::Near));
    }
    let title_w = spare.saturating_sub(circle.as_ref().map_or(0, |c| width(c) + 1));
    row.push((
        match title_w {
            0 => String::new(),
            _ => fit(&item.title, title_w),
        },
        Tone::Plain,
    ));
    row.push(("  ".into(), Tone::Plain));
    row.push((when, tone(&item.status)));
    row
}

/* the selected row's whole record: provenance and the stored body on the left, applicability
and evidence on the right */
pub(crate) fn card(detail: &Detail, now: u64, cols: usize) -> Grid {
    let (_, colw) = layout::columns(cols);
    let state = match detail.status.as_str() {
        "active" => format!(
            "active · left {}",
            layout::ago(detail.expires_at.max(0) as u64, Some(now))
        ),
        "expired" => format!(
            "expired · {} ago",
            layout::ago(now, Some(detail.expires_at.max(0) as u64))
        ),
        _ => format!(
            "revoked · {} ago",
            layout::ago(now, detail.revoked_at.map(|at| at.max(0) as u64))
        ),
    };
    let mut fields = vec![
        ("status", state, tone(&detail.status)),
        ("id", detail.id.clone(), Tone::Soft),
        ("circle", detail.circle.clone(), Tone::Soft),
        ("created", stamp(detail.created_at), Tone::Dim),
        ("expires", stamp(detail.expires_at), Tone::Dim),
        (
            "author",
            who(&detail.publisher_peer, Some(&detail.publisher_session)),
            Tone::Soft,
        ),
    ];
    if let Some(at) = detail.revoked_at {
        let by = who(
            detail.revoked_by_peer.as_deref().unwrap_or("-"),
            detail.revoked_by_session.as_deref(),
        );
        fields.extend([
            ("revoked", stamp(at), Tone::Dim),
            ("by", by, Tone::Soft),
            (
                "reason",
                detail.revoke_reason.clone().unwrap_or_default(),
                Tone::Warn,
            ),
        ]);
    }
    let mut rows: Vec<Row> = Vec::new();
    for (k, text, tone) in &fields {
        layout::wrapped(&mut rows, k, text, *tone, colw, usize::MAX);
    }
    let room = colw.saturating_sub(KEY);
    let body: Vec<String> = detail
        .body
        .lines()
        .flat_map(|line| {
            /* the stored indent is the note's own shape: keep it, hang it under the wrap, and
            never let it take more than half the width from the text */
            let text = line.trim_start_matches([' ', '\t']);
            let lead = line[..line.len() - text.len()]
                .chars()
                .map(|c| if c == '\t' { 4 } else { 1 })
                .sum::<usize>()
                .min(room / 2);
            layout::wrap(text, room - lead)
                .into_iter()
                .map(move |part| format!("{}{part}", " ".repeat(lead)))
        })
        .collect();
    rows.extend(layout::field("body", &body, usize::MAX, Tone::Plain, room));
    let mut facts: Vec<Row> = Vec::new();
    for (k, text) in [
        ("applies", &detail.applies_to),
        ("checked", &detail.evidence),
    ] {
        layout::wrapped(&mut facts, k, text, Tone::Soft, colw, usize::MAX);
    }
    facts.push(vec![
        (" ".repeat(KEY), Tone::Dim),
        (
            "(publisher-reported; the hub does not verify it)".into(),
            Tone::Dim,
        ),
    ]);
    let badge = format!(" {} ", detail.status.to_uppercase());
    layout::framed(
        ('·', tone(&detail.status)),
        &detail.title,
        Some((&badge, tone(&detail.status))),
        &rows,
        &facts,
        cols,
    )
}

/* what the page has to say when the hub holds more than one screenful; "the rest" is the
tail of the active-first order and says nothing about age or group */
pub(crate) fn notice(loaded: usize, total: u64, cols: usize) -> Option<String> {
    if total as usize <= loaded {
        return None;
    }
    Some(match cols >= 96 {
        true => format!("{loaded} of {total} shown · active first · the rest not listed"),
        false => format!("{loaded} of {total} · active first · rest cut"),
    })
}
