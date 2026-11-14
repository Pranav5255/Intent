//! Proposal validation and clustering (union-find with workspace guard).

use std::collections::{BTreeSet, HashMap, HashSet};

use intent_types::{Family, NormalizedEvent, SemanticEventProposal, SemanticRole};

use super::{PacketRole, RequestPacket, cluster_semantics};
use crate::traits::SemanticOutcome;

/// Exactly one proposal per requested id; links only to allowed ids.
/// Returns a message suitable for the harness repair turn.
pub fn batch_proposals_valid(
    requested: &[String],
    proposals: &[SemanticEventProposal],
    allowed_links: Option<&HashSet<String>>,
) -> Result<(), String> {
    let requested_set: HashSet<&str> = requested.iter().map(String::as_str).collect();
    if requested_set.len() != requested.len() {
        return Err("duplicate packet ids were requested".into());
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for p in proposals {
        if !requested_set.contains(p.event_id.as_str()) {
            return Err(format!("event_id {:?} is not one of the supplied packet ids", p.event_id));
        }
        if !seen.insert(p.event_id.as_str()) {
            return Err(format!("packet {} has more than one proposal", p.event_id));
        }
        if !(0.0..=1.0).contains(&p.confidence) || !p.confidence.is_finite() {
            return Err(format!("confidence for {} must be between 0 and 1", p.event_id));
        }
        for link in &p.linked_event_ids {
            let ok = match allowed_links {
                Some(allowed) => allowed.contains(link),
                None => requested_set.contains(link.as_str()),
            };
            if !ok {
                return Err(format!("{} links to unknown packet id {:?}", p.event_id, link));
            }
        }
    }
    let missing: Vec<&str> = requested.iter().map(String::as_str).filter(|id| !seen.contains(id)).collect();
    if !missing.is_empty() {
        return Err(format!("missing proposals for packet ids {}", missing.join(", ")));
    }
    Ok(())
}

fn role_rank(role: SemanticRole) -> u8 {
    match role {
        SemanticRole::Task => 3,
        SemanticRole::SupportingContext => 2,
        SemanticRole::Unrelated => 1,
        SemanticRole::Background => 0,
    }
}

/// Stable choice when an overlap packet receives two valid proposals.
pub fn proposal_is_preferred(candidate: &SemanticEventProposal, existing: &SemanticEventProposal) -> bool {
    let key = |p: &SemanticEventProposal| {
        let mut links = p.linked_event_ids.clone();
        links.sort();
        (p.confidence, role_rank(p.role), p.topic.clone(), links)
    };
    let (a, b) = (key(candidate), key(existing));
    a.partial_cmp(&b) == Some(std::cmp::Ordering::Greater)
}

#[derive(Debug, Clone, Copy)]
pub struct LinkPolicy {
    pub min_link_confidence: f64,
    pub borderline_floor: f64,
    pub embed_link_threshold: f32,
}

impl LinkPolicy {
    pub fn linkable(&self, p: &SemanticEventProposal) -> bool {
        p.confidence >= self.min_link_confidence && p.role.is_work()
    }

    /// Model links are honoured at full confidence; a borderline pair needs embedding support.
    pub fn link_ok(&self, a: &SemanticEventProposal, b: &SemanticEventProposal, similarity: Option<f32>) -> bool {
        if !a.role.is_work() || !b.role.is_work() {
            return false;
        }
        let low = a.confidence.min(b.confidence);
        if low >= self.min_link_confidence {
            return true;
        }
        low >= self.borderline_floor && similarity.is_some_and(|s| s >= self.embed_link_threshold)
    }
}

pub struct UnionFind {
    parent: HashMap<usize, usize>,
    workspaces: HashMap<usize, BTreeSet<String>>,
}

impl UnionFind {
    pub fn new(session: &[NormalizedEvent], members: &[usize]) -> Self {
        Self {
            parent: members.iter().map(|&i| (i, i)).collect(),
            workspaces: members
                .iter()
                .map(|&i| (i, session[i].entities.project_paths.iter().cloned().collect()))
                .collect(),
        }
    }

    pub fn find(&mut self, x: usize) -> usize {
        let mut root = x;
        while self.parent[&root] != root {
            root = self.parent[&root];
        }
        let mut cur = x;
        while self.parent[&cur] != root {
            let next = self.parent[&cur];
            self.parent.insert(cur, root);
            cur = next;
        }
        root
    }

    /// Union unless both sides carry project roots and none overlap.
    pub fn union_if_workspace_safe(&mut self, a: usize, b: usize) -> bool {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return true;
        }
        let wa = self.workspaces.get(&ra).cloned().unwrap_or_default();
        let wb = self.workspaces.remove(&rb).unwrap_or_default();
        if !wa.is_empty() && !wb.is_empty() && wa.is_disjoint(&wb) {
            self.workspaces.insert(rb, wb);
            return false;
        }
        self.parent.insert(rb, ra);
        self.workspaces.insert(ra, wa.union(&wb).cloned().collect());
        true
    }
}

fn families(packets: &[&RequestPacket]) -> BTreeSet<Family> {
    packets.iter().flat_map(|p| p.packet.events.iter().map(|e| e.family)).collect()
}

fn adjacent_within(prev: &RequestPacket, cur: &RequestPacket) -> bool {
    prev.role() == PacketRole::Candidate
        && cur.role() == PacketRole::Candidate
        && cur.packet.start_ts - prev.packet.end_ts <= 5 * 60
}

/// Keep a continuous browser-only phase together when the model calls it non-work.
pub fn continuous_nonwork_browsing(
    prev: &RequestPacket,
    prev_p: &SemanticEventProposal,
    cur: &RequestPacket,
    cur_p: &SemanticEventProposal,
) -> bool {
    if !adjacent_within(prev, cur) || prev_p.role.is_work() || cur_p.role.is_work() {
        return false;
    }
    let fam = families(&[prev, cur]);
    fam.contains(&Family::Browser) && fam.iter().all(|f| matches!(f, Family::Browser | Family::Focus))
}

/// Join packet-size boundaries within the same uninterrupted editor work.
pub fn continuous_editor_work(
    prev: &RequestPacket,
    prev_p: &SemanticEventProposal,
    cur: &RequestPacket,
    cur_p: &SemanticEventProposal,
    policy: &LinkPolicy,
) -> bool {
    if !adjacent_within(prev, cur) || !policy.linkable(prev_p) || !policy.linkable(cur_p) {
        return false;
    }
    let fam = families(&[prev, cur]);
    if !fam.contains(&Family::Editor) || !fam.iter().all(|f| matches!(f, Family::Editor | Family::Focus)) {
        return false;
    }
    let context = |p: &RequestPacket| -> HashSet<String> {
        p.packet.events.iter().flat_map(|e| e.project_paths.iter().cloned().chain(e.file_name.clone())).collect()
    };
    !context(prev).is_disjoint(&context(cur))
}

/// Clusters of session indices plus the selected proposal per event index.
pub type ValidatedClusters = (Vec<Vec<usize>>, HashMap<usize, SemanticEventProposal>);

/// Validate every batch and turn proposals into clusters of session indices.
/// Returns `None` when any batch is invalid (the engine then keeps deterministic clusters).
pub fn validated_clusters(
    session: &[NormalizedEvent],
    packets: &[RequestPacket],
    results: &[(Vec<usize>, Vec<SemanticEventProposal>)],
    full_capture: bool,
    policy: &LinkPolicy,
    similarity: &dyn Fn(usize, usize) -> Option<f32>,
) -> Option<ValidatedClusters> {
    let supplied: Vec<usize> = packets.iter().flat_map(|p| p.packet.events.iter().map(|e| e.index)).collect();
    if supplied.iter().collect::<HashSet<_>>().len() != supplied.len() || supplied.iter().any(|&i| i >= session.len()) {
        return None;
    }
    let index_of: HashMap<&str, usize> = packets.iter().enumerate().map(|(i, p)| (p.id.as_str(), i)).collect();
    let all_ids: HashSet<String> = packets.iter().map(|p| p.id.clone()).collect();
    let mut selected: HashMap<usize, SemanticEventProposal> = HashMap::new();
    for (batch, proposals) in results {
        if batch.iter().any(|&i| i >= packets.len()) {
            return None;
        }
        let ids: Vec<String> = batch.iter().map(|&i| packets[i].id.clone()).collect();
        batch_proposals_valid(&ids, proposals, full_capture.then_some(&all_ids)).ok()?;
        for p in proposals {
            let pi = index_of[p.event_id.as_str()];
            match selected.get(&pi) {
                Some(existing) if !proposal_is_preferred(p, existing) => {}
                _ => {
                    selected.insert(pi, p.clone());
                }
            }
        }
    }
    if selected.len() != packets.len() {
        return None;
    }

    let mut uf = UnionFind::new(session, &supplied);
    let mut per_event: HashMap<usize, SemanticEventProposal> = HashMap::new();
    for (pi, packet) in packets.iter().enumerate() {
        let proposal = &selected[&pi];
        let linked: Vec<String> = proposal
            .linked_event_ids
            .iter()
            .filter_map(|id| index_of.get(id.as_str()))
            .map(|&li| packets[li].packet.events[0].event_id.clone())
            .collect();
        for e in &packet.packet.events {
            let mut p = proposal.clone();
            p.event_id = e.event_id.clone();
            p.linked_event_ids = linked.clone();
            per_event.insert(e.index, p);
        }
        // A candidate packet is a local, chronologically coherent unit: keep it together
        // even when the model calls it background/unrelated (that only blocks linking).
        if packet.role() == PacketRole::Background {
            continue;
        }
        let first = packet.packet.events[0].index;
        for e in &packet.packet.events[1..] {
            uf.union_if_workspace_safe(first, e.index);
        }
    }

    for (pi, packet) in packets.iter().enumerate() {
        let proposal = &selected[&pi];
        if packet.role() == PacketRole::Background || !proposal.role.is_work() {
            continue;
        }
        for link in &proposal.linked_event_ids {
            let Some(&li) = index_of.get(link.as_str()) else { continue };
            if li == pi || packets[li].role() == PacketRole::Background {
                continue;
            }
            if policy.link_ok(proposal, &selected[&li], similarity(pi, li)) {
                uf.union_if_workspace_safe(packet.packet.events[0].index, packets[li].packet.events[0].index);
            }
        }
    }

    for pi in 1..packets.len() {
        let (prev, cur) = (&packets[pi - 1], &packets[pi]);
        let (pp, cp) = (&selected[&(pi - 1)], &selected[&pi]);
        if continuous_nonwork_browsing(prev, pp, cur, cp) || continuous_editor_work(prev, pp, cur, cp, policy) {
            uf.union_if_workspace_safe(prev.packet.events[0].index, cur.packet.events[0].index);
        }
    }

    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for &i in &supplied {
        let root = uf.find(i);
        groups.entry(root).or_default().push(i);
    }
    let mut clusters: Vec<Vec<usize>> = groups.into_values().collect();
    // Excluded (messaging) events never reach the provider but must stay visible.
    let supplied_set: HashSet<usize> = supplied.iter().copied().collect();
    clusters.extend((0..session.len()).filter(|i| !supplied_set.contains(i)).map(|i| vec![i]));
    Some((clusters, per_event))
}

/// Sort clusters (and members) by (ts, ordinal) and attach per-cluster semantics.
pub fn outcome(
    session: &[NormalizedEvent],
    mut clusters: Vec<Vec<usize>>,
    proposals: &HashMap<usize, SemanticEventProposal>,
) -> SemanticOutcome {
    let key = |i: usize| (session[i].ts, session[i].ordinal, i);
    for c in &mut clusters {
        c.sort_by_key(|&i| key(i));
    }
    clusters.retain(|c| !c.is_empty());
    clusters.sort_by_key(|c| key(c[0]));
    let semantics = clusters.iter().map(|c| cluster_semantics(c, proposals)).collect();
    SemanticOutcome { clusters, semantics }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop(id: &str, role: SemanticRole, confidence: f64, links: &[&str]) -> SemanticEventProposal {
        SemanticEventProposal {
            event_id: id.into(),
            role,
            confidence,
            topic: "t".into(),
            linked_event_ids: links.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn batch_validation_rules() {
        let ids = vec!["p0".to_string(), "p1".to_string()];
        let ok = vec![prop("p0", SemanticRole::Task, 0.9, &["p1"]), prop("p1", SemanticRole::Task, 0.9, &[])];
        assert!(batch_proposals_valid(&ids, &ok, None).is_ok());
        let missing = vec![prop("p0", SemanticRole::Task, 0.9, &[])];
        assert!(batch_proposals_valid(&ids, &missing, None).unwrap_err().contains("p1"));
        let unknown_link = vec![prop("p0", SemanticRole::Task, 0.9, &["p7"]), prop("p1", SemanticRole::Task, 0.9, &[])];
        assert!(batch_proposals_valid(&ids, &unknown_link, None).is_err());
        let dup = vec![prop("p0", SemanticRole::Task, 0.9, &[]), prop("p0", SemanticRole::Task, 0.9, &[])];
        assert!(batch_proposals_valid(&ids, &dup, None).is_err());
        let bad_conf = vec![prop("p0", SemanticRole::Task, 1.5, &[]), prop("p1", SemanticRole::Task, 0.9, &[])];
        assert!(batch_proposals_valid(&ids, &bad_conf, None).is_err());
        let allowed: HashSet<String> = ["p0", "p1", "p9"].iter().map(|s| s.to_string()).collect();
        let cross = vec![prop("p0", SemanticRole::Task, 0.9, &["p9"]), prop("p1", SemanticRole::Task, 0.9, &[])];
        assert!(batch_proposals_valid(&ids, &cross, Some(&allowed)).is_ok());
    }

    #[test]
    fn preference_orders_by_confidence_then_role() {
        let a = prop("p0", SemanticRole::Task, 0.8, &[]);
        let b = prop("p0", SemanticRole::Background, 0.8, &[]);
        let c = prop("p0", SemanticRole::Background, 0.9, &[]);
        assert!(proposal_is_preferred(&a, &b));
        assert!(proposal_is_preferred(&c, &a));
        assert!(!proposal_is_preferred(&a, &a));
    }

    #[test]
    fn link_policy_borderline_needs_similarity() {
        let policy = LinkPolicy { min_link_confidence: 0.7, borderline_floor: 0.55, embed_link_threshold: 0.5 };
        let strong = prop("a", SemanticRole::Task, 0.9, &[]);
        let border = prop("b", SemanticRole::SupportingContext, 0.6, &[]);
        let weak = prop("c", SemanticRole::Task, 0.4, &[]);
        let bg = prop("d", SemanticRole::Background, 0.95, &[]);
        assert!(policy.link_ok(&strong, &strong, None));
        assert!(!policy.link_ok(&strong, &border, None));
        assert!(!policy.link_ok(&strong, &border, Some(0.3)));
        assert!(policy.link_ok(&strong, &border, Some(0.6)));
        assert!(!policy.link_ok(&strong, &weak, Some(0.99)));
        assert!(!policy.link_ok(&strong, &bg, Some(0.99)));
    }

    #[test]
    fn union_find_workspace_guard() {
        use crate::semantic::pack::tests::ev;
        let mut a = ev("a", 0, Family::Editor, "document_change");
        a.entities.project_paths = vec!["/w/one".into()];
        let mut b = ev("b", 1, Family::Editor, "document_change");
        b.entities.project_paths = vec!["/w/two".into()];
        let c = ev("c", 2, Family::Browser, "navigation");
        let mut d = ev("d", 3, Family::Editor, "document_change");
        d.entities.project_paths = vec!["/w/one".into(), "/w/two".into()];
        let session = vec![a, b, c, d];
        let mut uf = UnionFind::new(&session, &[0, 1, 2, 3]);
        assert!(!uf.union_if_workspace_safe(0, 1), "disjoint workspaces never merge");
        assert!(uf.union_if_workspace_safe(0, 2), "no workspace merges freely");
        assert!(!uf.union_if_workspace_safe(2, 1), "merged set inherits /w/one");
        assert!(uf.union_if_workspace_safe(3, 1), "overlap allowed");
        assert!(uf.union_if_workspace_safe(0, 3));
        assert_eq!(uf.find(1), uf.find(0));
    }
}
