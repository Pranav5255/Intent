//! Daily digest from persisted intents (0.1 `digest.py`).
//!
//! 0.1 replaced the headline for `project:taskflow-app` with a hard-coded demo
//! string; 0.2 derives the headline from the data only.

use intent_types::{Digest, Intent};

use crate::util::unique;

pub fn build_digest(intents: &[Intent], date: &str) -> Digest {
    let roots: Vec<&Intent> = intents.iter().filter(|i| i.depth == 0).collect();
    if roots.is_empty() {
        return Digest {
            date: date.into(),
            headline: "No recorded work".into(),
            summary: "No intents were stored for this date.".into(),
            top_intent_ids: Vec::new(),
            intent_count: 0,
            total_duration_seconds: 0,
        };
    }
    let primary = primary_root(&roots);
    let mut ranked: Vec<(usize, &Intent)> = roots.iter().copied().enumerate().collect();
    ranked.sort_by(|a, b| b.1.stats.duration_seconds.cmp(&a.1.stats.duration_seconds).then(a.0.cmp(&b.0)));
    Digest {
        date: date.into(),
        headline: headline(primary, &roots),
        summary: summary(primary),
        top_intent_ids: ranked.into_iter().take(3).map(|(_, i)| i.id.clone()).collect(),
        intent_count: roots.len() as i64,
        total_duration_seconds: roots.iter().map(|r| r.stats.duration_seconds).sum(),
    }
}

/// Project-tagged roots first; most children, then longest.
fn primary_root<'a>(roots: &[&'a Intent]) -> &'a Intent {
    let tagged: Vec<&Intent> = roots.iter().copied().filter(|r| r.project_tag().is_some()).collect();
    let candidates = if tagged.is_empty() { roots.to_vec() } else { tagged };
    // `max_by_key` keeps the last maximum; 0.1 `max` kept the first.
    let mut best = candidates[0];
    for candidate in candidates.iter().skip(1) {
        if (candidate.children.len(), candidate.stats.duration_seconds)
            > (best.children.len(), best.stats.duration_seconds)
        {
            best = candidate;
        }
    }
    best
}

fn headline(primary: &Intent, roots: &[&Intent]) -> String {
    let mut headline = primary.label.clone();
    let project = primary.project_tag();
    if let Some(project) = project
        && !headline.to_lowercase().contains(&project.to_lowercase())
    {
        headline = format!("{headline} in {project}");
    }
    let others: Vec<&str> = unique(roots.iter().filter_map(|r| r.project_tag()).filter(|p| Some(*p) != project));
    match others.as_slice() {
        [] => {}
        [one] => headline.push_str(&format!(", plus {one}")),
        [a, b] => headline.push_str(&format!(", plus {a} and {b}")),
        [a, rest @ ..] => headline.push_str(&format!(", plus {a} and {} other projects", rest.len())),
    }
    headline
}

fn summary(root: &Intent) -> String {
    let mut parts: Vec<String> = Vec::new();
    let labels: Vec<&str> = root.children.iter().map(|c| c.label.as_str()).filter(|l| !l.is_empty()).collect();
    if !labels.is_empty() {
        parts.push(format!(
            "You worked on {}",
            labels.iter().take(2).copied().collect::<Vec<_>>().join(", ").to_lowercase()
        ));
    }
    if let Some(file) = insight_values(root, |i| &i.insights.editor, "file").first() {
        parts.push(format!("edited {file}"));
    }
    if let Some(domain) = insight_values(root, |i| &i.insights.browser, "domain").first() {
        parts.push(format!("researched {domain}"));
    }
    if let Some(family) = failed_families(root).first() {
        parts.push(format!("hit failing {family} commands"));
    }
    if parts.is_empty() {
        return root.summary.clone();
    }
    let mut sentence = parts.into_iter().take(3).collect::<Vec<_>>().join(", ").trim_end_matches(['.', ' ']).to_owned();
    sentence.push('.');
    let mut chars = sentence.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => sentence,
    }
}

type InsightList = Vec<serde_json::Map<String, serde_json::Value>>;

fn insight_values(root: &Intent, pick: impl Fn(&Intent) -> &InsightList, key: &str) -> Vec<String> {
    let nodes = std::iter::once(root).chain(root.children.iter());
    unique(
        nodes.flat_map(|n| pick(n).iter().filter_map(|i| i.get(key)?.as_str().map(str::to_owned)).collect::<Vec<_>>()),
    )
}

fn failed_families(root: &Intent) -> Vec<String> {
    let nodes = std::iter::once(root).chain(root.children.iter());
    unique(nodes.flat_map(|n| {
        n.insights
            .shell
            .iter()
            .filter(|i| i.get("count").and_then(|c| c.as_i64()).is_some_and(|c| c > 0))
            .filter_map(|i| i.get("command_family")?.as_str().map(str::to_owned))
            .collect::<Vec<_>>()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::intent;
    use serde_json::json;

    #[test]
    fn empty_day() {
        let d = build_digest(&[], "2026-01-01");
        assert_eq!(d.headline, "No recorded work");
        assert_eq!(d.intent_count, 0);
    }

    #[test]
    fn headline_is_data_driven() {
        let mut a = intent("a", "Work on taskflow-app", 0, 600);
        a.tags = vec!["project:taskflow-app".into()];
        let mut child = intent("a1", "Edit login.ts", 0, 300);
        child.insights.editor = vec![json!({"file": "login.ts"}).as_object().unwrap().clone()];
        child.insights.shell = vec![json!({"command_family": "npm", "count": 2}).as_object().unwrap().clone()];
        a.children = vec![child.clone(), intent("a2", "Run Npm", 300, 600)];
        let mut b = intent("b", "Run Go", 700, 2000);
        b.tags = vec!["project:beta".into()];
        let d = build_digest(&[a, b], "2026-01-01");
        assert_eq!(d.headline, "Work on taskflow-app, plus beta");
        assert_ne!(d.headline, "Building Login Feature");
        assert_eq!(d.summary, "You worked on edit login.ts, run npm, edited login.ts, hit failing npm commands.");
        assert_eq!(d.top_intent_ids, vec!["b", "a"]);
        assert_eq!(d.total_duration_seconds, 600 + 1300);
    }

    #[test]
    fn project_appended_when_label_lacks_it() {
        let mut a = intent("a", "Run Cargo", 0, 60);
        a.tags = vec!["project:ledger".into()];
        assert_eq!(build_digest(&[a], "d").headline, "Run Cargo in ledger");
    }
}
