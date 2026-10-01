use serde::Deserialize;
use std::collections::{BTreeSet, HashMap, HashSet};

/* one record of the hub's event ring as GET /events lists it; ask and chat carry `text`, the
others `message` */
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Event {
    #[serde(default)]
    pub seq: Option<u64>,
    #[serde(default)]
    pub at: Option<u64>,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub from_peer: String,
    #[serde(default)]
    pub to_peer: String,
    #[serde(default)]
    pub peer: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub correlation_id: String,
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub message: String,
}

#[cfg(test)]
pub(crate) use crate::wire::Activity;
pub(crate) use crate::wire::{
    AskView as Ask, JobView as Job, PeerView as Peer, SnapshotView as Snapshot,
};

/* the wire snapshot plus whether this draw is still within the fresh window */
#[derive(Clone, Debug, Default)]
pub(crate) struct Frame {
    pub view: Snapshot,
    pub fresh: bool,
}

impl std::ops::Deref for Frame {
    type Target = Snapshot;

    fn deref(&self) -> &Snapshot {
        &self.view
    }
}

impl std::ops::DerefMut for Frame {
    fn deref_mut(&mut self) -> &mut Snapshot {
        &mut self.view
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HubMode {
    Neutral,
    V2,
    Unsupported(u64),
}

impl crate::wire::SnapshotView {
    pub fn job(&self, id: &str) -> Option<&Job> {
        self.jobs.iter().find(|job| job.job_id == id)
    }

    pub fn ask(&self, id: Option<&str>) -> Option<&Ask> {
        id.and_then(|id| self.asks.iter().find(|ask| ask.correlation_id == id))
    }

    /* the asks no job in the view points at; hubs without ask_list send only the asks of jobs */
    pub fn asks_outside_jobs(&self) -> Vec<&Ask> {
        let taken: HashSet<&str> = self
            .jobs
            .iter()
            .filter_map(|job| job.ask_id.as_deref())
            .collect();
        self.asks
            .iter()
            .filter(|ask| !taken.contains(ask.correlation_id.as_str()))
            .collect()
    }

    /* an ask's recipient is a fixed peer_id; a name may have passed to another peer */
    pub fn peer_by_id(&self, id: &str) -> Option<&Peer> {
        self.peers.iter().find(|peer| peer.peer_id == id)
    }
}

/* one connected component of depends_on among the returned jobs; stage = longest path
from a root, so a job always sits below everything it waits for. Jobs without any
dependency either way are gathered in one loose block instead of a chain each */
#[derive(Clone, Debug)]
pub(crate) struct Chain {
    pub name: String,
    pub loose: bool,
    pub stage: HashMap<String, usize>,
    pub stages: Vec<Vec<String>>,
    pub deps: HashMap<String, Vec<String>>,
}

impl Chain {
    pub fn dependents(&self, id: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .deps
            .iter()
            .filter(|(_, deps)| deps.iter().any(|d| d == id))
            .map(|(job, _)| job.clone())
            .collect();
        out.sort();
        out
    }

    pub fn topo(&self) -> Vec<String> {
        self.stages.iter().flatten().cloned().collect()
    }
}

pub(crate) fn chains(snap: &Snapshot) -> Vec<Chain> {
    let ids: HashSet<&str> = snap.jobs.iter().map(|job| job.job_id.as_str()).collect();
    let mut parent: HashMap<&str, &str> = ids.iter().map(|id| (*id, *id)).collect();
    fn root<'a>(parent: &mut HashMap<&'a str, &'a str>, id: &'a str) -> &'a str {
        let up = parent[id];
        if up == id {
            return id;
        }
        let top = root(parent, up);
        parent.insert(id, top);
        top
    }
    for job in &snap.jobs {
        for dep in job.depends_on.iter().filter(|d| ids.contains(d.as_str())) {
            let (a, b) = (root(&mut parent, &job.job_id), root(&mut parent, dep));
            if a != b {
                parent.insert(a, b);
            }
        }
    }
    let mut components: HashMap<&str, Vec<&Job>> = HashMap::new();
    for job in &snap.jobs {
        components
            .entry(root(&mut parent, &job.job_id))
            .or_default()
            .push(job);
    }
    let (single, mut groups): (Vec<Vec<&Job>>, Vec<Vec<&Job>>) =
        components.into_values().partition(|jobs| jobs.len() == 1);
    if !single.is_empty() {
        groups.push(single.into_iter().flatten().collect());
    }
    let mut out: Vec<(u64, Chain)> = groups
        .into_iter()
        .map(|jobs| {
            let deps: HashMap<String, Vec<String>> = jobs
                .iter()
                .map(|job| {
                    let known = job
                        .depends_on
                        .iter()
                        .filter(|d| ids.contains(d.as_str()) && **d != job.job_id)
                        .cloned()
                        .collect();
                    (job.job_id.clone(), known)
                })
                .collect();
            let mut stage: HashMap<String, usize> = HashMap::new();
            fn depth(
                id: &str,
                deps: &HashMap<String, Vec<String>>,
                stage: &mut HashMap<String, usize>,
            ) -> usize {
                if let Some(s) = stage.get(id) {
                    return *s;
                }
                stage.insert(id.to_string(), 0);
                let s = deps[id]
                    .iter()
                    .map(|d| depth(d, deps, stage) + 1)
                    .max()
                    .unwrap_or(0);
                stage.insert(id.to_string(), s);
                s
            }
            for job in &jobs {
                depth(&job.job_id, &deps, &mut stage);
            }
            let created = |id: &str| {
                jobs.iter()
                    .find(|job| job.job_id == id)
                    .and_then(|job| job.created_at)
                    .unwrap_or(0)
            };
            /* a cycle, which the hub never creates but a snapshot can carry, leaves stages
            empty; closing them up keeps every stage drawable */
            let mut used: Vec<usize> = stage.values().copied().collect();
            used.sort_unstable();
            used.dedup();
            for s in stage.values_mut() {
                *s = used.binary_search(s).expect("taken from the same values");
            }
            let mut stages: Vec<Vec<String>> = vec![Vec::new(); used.len()];
            for (id, s) in &stage {
                stages[*s].push(id.clone());
            }
            for row in &mut stages {
                row.sort_by(|a, b| created(a).cmp(&created(b)).then(a.cmp(b)));
            }
            let sinks: BTreeSet<&String> = deps
                .keys()
                .filter(|id| !deps.values().any(|ds| ds.contains(id)))
                .collect();
            let title = |id: &String| {
                jobs.iter()
                    .find(|job| &job.job_id == id)
                    .map_or(id.clone(), |job| job.title.clone())
            };
            let loose = deps.values().all(Vec::is_empty);
            let name = if loose {
                "independent".into()
            } else {
                sinks.into_iter().map(title).collect::<Vec<_>>().join(" + ")
            };
            let first = jobs
                .iter()
                .filter_map(|job| job.created_at)
                .min()
                .unwrap_or(0);
            (
                first,
                Chain {
                    name,
                    loose,
                    stage,
                    stages,
                    deps,
                },
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.name.cmp(&b.1.name)));
    out.into_iter().map(|(_, chain)| chain).collect()
}

/* numbers stay put while a block only grows: a job that appears later takes the next
number. A block that lost a job or took one in from another block is numbered again in
topological order, so a block always reads 1..n without gaps or clashes */
#[derive(Default)]
pub(crate) struct Numbers {
    map: HashMap<String, usize>,
    /* counts renumberings, so a half-typed jump can tell its numbers went stale */
    pub renumbered: u64,
}

impl Numbers {
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /* a pane left open for days forgets the jobs the hub cleaned up */
    pub fn forget_gone(&mut self, snap: &Snapshot) {
        self.map.retain(|id, _| snap.job(id).is_some());
    }

    pub fn assign(&mut self, chain: &Chain) -> Vec<(usize, String)> {
        let topo = chain.topo();
        let mut known: Vec<usize> = topo
            .iter()
            .filter_map(|id| self.map.get(id))
            .copied()
            .collect();
        known.sort_unstable();
        let dense = known.iter().enumerate().all(|(i, n)| *n == i + 1);
        if !dense {
            self.renumbered += 1;
        }
        let mut next = if dense { known.len() + 1 } else { 1 };
        for id in &topo {
            if !dense || !self.map.contains_key(id) {
                self.map.insert(id.clone(), next);
                next += 1;
            }
        }
        let mut out: Vec<(usize, String)> =
            topo.into_iter().map(|id| (self.map[&id], id)).collect();
        out.sort();
        out
    }
}
