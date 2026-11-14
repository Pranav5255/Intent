//! Deterministic, privacy-bounded packets (port of 1.x `semantic_pack.py`).

use intent_types::{Family, NormalizedEvent, Payload};
use serde_json::{Map, Value};

use crate::util::{squash_ws, truncate_chars};

pub const MAX_EVENTS_PER_PACKET: usize = 12;
pub const MAX_SNIPPET_CHARS_PER_EVENT: usize = 500;
pub const MAX_SNIPPET_CHARS_PER_PACKET: usize = 2000;

const MESSAGING_MARKERS: &[&str] = &["whatsapp", "telegram", "signal", "slack", "discord", "teams"];
const MEDIA_MARKERS: &[&str] = &["spotify", "vlc", "rhythmbox", "media player", "music.apple.com", "music.youtube.com"];
const SAFE_BROWSER_ACTIONS: &[&str] = &[
    "click",
    "link_activation",
    "form_submit",
    "toggle",
    "select_change",
    "scroll",
    "like",
    "reply",
    "repost",
    "share",
    "follow",
    "unfollow",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketRole {
    Candidate,
    Background,
}

impl PacketRole {
    pub fn as_str(self) -> &'static str {
        match self {
            PacketRole::Candidate => "candidate",
            PacketRole::Background => "background",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classification {
    Candidate,
    Background,
    /// Messaging apps: never sent to a provider unless full capture is consented.
    Excluded,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PackConfig {
    pub max_gap_secs: i64,
    pub content_consent: bool,
    /// Requires `content_consent` as well.
    pub full_capture: bool,
}

impl PackConfig {
    pub fn full_capture(&self) -> bool {
        self.content_consent && self.full_capture
    }
}

/// A limited event representation that may be described to a provider.
#[derive(Debug, Clone, PartialEq)]
pub struct PacketEvent {
    /// Index into the session slice.
    pub index: usize,
    pub event_id: String,
    pub ts: i64,
    pub family: Family,
    pub category: String,
    pub source: String,
    pub project_paths: Vec<String>,
    pub file_name: Option<String>,
    pub domain: Option<String>,
    pub command_family: Option<String>,
    pub action: Option<String>,
    pub deterministic_role: PacketRole,
    pub content_snippet: Option<String>,
    /// The complete Role A event, only under explicit full-capture consent.
    pub captured_event: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
    pub start_ts: i64,
    pub end_ts: i64,
    pub role: PacketRole,
    pub events: Vec<PacketEvent>,
}

/// `raw` may be the full Role A event (`{id, ts, source, type, payload}`) or just its payload.
pub fn payload_of(event: &NormalizedEvent) -> &Payload {
    match event.raw.get("payload") {
        Some(Value::Object(p)) => p,
        _ => &event.raw,
    }
}

pub fn build_packets(session: &[NormalizedEvent], config: &PackConfig) -> Vec<Packet> {
    let full_capture = config.full_capture();
    let gap = config.max_gap_secs.max(0);
    let mut order: Vec<usize> = (0..session.len()).collect();
    order.sort_by_key(|&i| (session[i].ts, session[i].ordinal));

    let mut packets = Vec::new();
    let mut candidates: Vec<usize> = Vec::new();
    for idx in order {
        let event = &session[idx];
        let class = match classify(event) {
            Classification::Excluded if !full_capture => continue,
            Classification::Excluded => Classification::Candidate,
            other => other,
        };
        if class == Classification::Background {
            if !candidates.is_empty() {
                packets.push(build_packet(session, &std::mem::take(&mut candidates), PacketRole::Candidate, config));
            }
            packets.push(build_packet(session, &[idx], PacketRole::Background, config));
            continue;
        }
        if let Some(&last) = candidates.last()
            && (event.ts - session[last].ts > gap || candidates.len() >= MAX_EVENTS_PER_PACKET)
        {
            packets.push(build_packet(session, &std::mem::take(&mut candidates), PacketRole::Candidate, config));
        }
        candidates.push(idx);
    }
    if !candidates.is_empty() {
        packets.push(build_packet(session, &candidates, PacketRole::Candidate, config));
    }
    packets
}

fn build_packet(session: &[NormalizedEvent], indices: &[usize], role: PacketRole, config: &PackConfig) -> Packet {
    let full_capture = config.full_capture();
    let budgets = fair_snippet_budgets(session, indices, role, config);
    let events = indices
        .iter()
        .zip(budgets)
        .map(|(&idx, budget)| {
            let event = &session[idx];
            let snippet = if !full_capture && role == PacketRole::Candidate && budget > 0 {
                content_snippet(event, budget, config.content_consent)
            } else {
                None
            };
            PacketEvent {
                index: idx,
                event_id: event.id.clone(),
                ts: event.ts,
                family: event.family,
                category: event.category.clone(),
                source: event.source.clone(),
                project_paths: event.entities.project_paths.clone(),
                file_name: event.entities.file_name.clone(),
                domain: event.entities.domain.clone(),
                command_family: event.entities.command_family.clone(),
                action: browser_action(event),
                deterministic_role: role,
                content_snippet: snippet,
                captured_event: full_capture.then(|| captured_event(event)),
            }
        })
        .collect();
    Packet {
        start_ts: session[indices[0]].ts,
        end_ts: session[*indices.last().unwrap_or(&indices[0])].ts,
        role,
        events,
    }
}

fn fair_snippet_budgets(
    session: &[NormalizedEvent],
    indices: &[usize],
    role: PacketRole,
    config: &PackConfig,
) -> Vec<usize> {
    if config.full_capture() || role != PacketRole::Candidate || !config.content_consent {
        return vec![0; indices.len()];
    }
    let eligible: Vec<usize> = indices
        .iter()
        .enumerate()
        .filter(|(_, idx)| content_snippet(&session[**idx], MAX_SNIPPET_CHARS_PER_EVENT, true).is_some())
        .map(|(pos, _)| pos)
        .collect();
    let mut budgets = vec![0; indices.len()];
    if eligible.is_empty() {
        return budgets;
    }
    let per_event = (MAX_SNIPPET_CHARS_PER_PACKET / eligible.len()).clamp(1, MAX_SNIPPET_CHARS_PER_EVENT);
    for pos in eligible {
        budgets[pos] = per_event;
    }
    budgets
}

fn captured_event(event: &NormalizedEvent) -> Map<String, Value> {
    if event.raw.contains_key("payload") {
        return event.raw.clone();
    }
    let mut m = Map::new();
    m.insert("id".into(), Value::String(event.id.clone()));
    m.insert("ts".into(), Value::from(event.ts));
    m.insert("source".into(), Value::String(event.source.clone()));
    m.insert("type".into(), Value::String(event.category.clone()));
    m.insert("payload".into(), Value::Object(event.raw.clone()));
    m
}

fn browser_action(event: &NormalizedEvent) -> Option<String> {
    if event.family != Family::Browser || event.category != "user_action" {
        return None;
    }
    let action = payload_of(event).get("action")?.as_str()?;
    SAFE_BROWSER_ACTIONS.contains(&action).then(|| action.to_owned())
}

pub fn classify(event: &NormalizedEvent) -> Classification {
    let payload = payload_of(event);
    let mut ids = vec![
        event.source.to_lowercase(),
        event.entities.domain.as_deref().unwrap_or_default().to_lowercase(),
        event.entities.title.as_deref().unwrap_or_default().to_lowercase(),
    ];
    for key in ["app", "title"] {
        if let Some(Value::String(v)) = payload.get(key) {
            ids.push(v.to_lowercase());
        }
    }
    let matches = |markers: &[&str]| ids.iter().any(|v| markers.iter().any(|m| v.contains(m)));
    if matches(MESSAGING_MARKERS) {
        Classification::Excluded
    } else if matches(MEDIA_MARKERS) {
        Classification::Background
    } else {
        Classification::Candidate
    }
}

fn content_snippet(event: &NormalizedEvent, remaining: usize, consent: bool) -> Option<String> {
    if !consent {
        return None;
    }
    let payload = payload_of(event);
    match (event.family, event.category.as_str()) {
        (Family::Editor, "document_change") => {
            let changes = payload.get("changes")?.as_array()?;
            let fragments: Vec<&str> = changes
                .iter()
                .filter_map(|c| c.as_object())
                .filter(|c| !c.get("redacted").and_then(Value::as_bool).unwrap_or(false))
                .filter_map(|c| c.get("text").and_then(Value::as_str))
                .filter(|t| *t != "[redacted]")
                .collect();
            bounded_snippet(&fragments.join("\n"), remaining)
        }
        (Family::Browser, "user_action") => {
            let flagged = |k: &str| payload.get(k).is_some_and(|v| v.as_bool().unwrap_or(!v.is_null()));
            if flagged("sensitive_page") || flagged("blocked") {
                return None;
            }
            let excerpt = payload.get("context")?.get("text_excerpt")?.as_str()?;
            bounded_snippet(excerpt, remaining)
        }
        _ => None,
    }
}

fn bounded_snippet(value: &str, remaining: usize) -> Option<String> {
    let normalized = squash_ws(value);
    if normalized.is_empty() || normalized == "[redacted]" {
        return None;
    }
    let limit = MAX_SNIPPET_CHARS_PER_EVENT.min(remaining);
    let out = truncate_chars(&normalized, limit);
    (!out.is_empty()).then(|| out.to_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use intent_types::{EventEntities, EventSignals};
    use serde_json::json;

    pub fn ev(id: &str, ts: i64, family: Family, category: &str) -> NormalizedEvent {
        NormalizedEvent {
            id: id.into(),
            ts,
            ordinal: 0,
            source: match family {
                Family::Editor => "vscode",
                Family::Browser => "firefox",
                Family::Command => "shell",
                Family::Focus => "linux",
                _ => "other",
            }
            .into(),
            family,
            category: category.into(),
            text: String::new(),
            entities: EventEntities::default(),
            signals: EventSignals::default(),
            evidence: vec![],
            raw: Map::new(),
        }
    }

    #[test]
    fn splits_on_gap_and_size_and_background() {
        let mut session: Vec<NormalizedEvent> =
            (0..14).map(|i| ev(&format!("e{i}"), i * 10, Family::Editor, "document_change")).collect();
        session.push(ev("gap", 14 * 10 + 400, Family::Editor, "document_change"));
        let mut media = ev("m", 1000, Family::Focus, "app_focus");
        media.raw = json!({"payload": {"app": "Spotify"}}).as_object().unwrap().clone();
        session.push(media);
        let mut chat = ev("c", 1010, Family::Focus, "app_focus");
        chat.entities.title = Some("Slack | general".into());
        session.push(chat);

        let packets = build_packets(&session, &PackConfig { max_gap_secs: 300, ..Default::default() });
        let sizes: Vec<usize> = packets.iter().map(|p| p.events.len()).collect();
        assert_eq!(sizes, vec![12, 2, 1, 1]);
        assert_eq!(packets[3].role, PacketRole::Background);
        assert!(packets.iter().all(|p| p.events.iter().all(|e| e.event_id != "c")), "messaging excluded");

        let full =
            build_packets(&session, &PackConfig { max_gap_secs: 300, content_consent: true, full_capture: true });
        assert!(full.iter().any(|p| p.events.iter().any(|e| e.event_id == "c")), "full capture includes messaging");
    }

    #[test]
    fn snippets_need_consent_and_share_budget() {
        let mut a = ev("a", 0, Family::Editor, "document_change");
        a.raw = json!({"payload": {"changes": [{"text": "x".repeat(900)}, {"text": "secret", "redacted": true}]}})
            .as_object()
            .unwrap()
            .clone();
        let mut b = ev("b", 5, Family::Browser, "user_action");
        b.raw = json!({"payload": {"action": "click", "context": {"text_excerpt": "Rust   async traits"}}})
            .as_object()
            .unwrap()
            .clone();
        let session = vec![a, b];
        let none = build_packets(&session, &PackConfig { max_gap_secs: 300, ..Default::default() });
        assert!(none[0].events.iter().all(|e| e.content_snippet.is_none()));
        assert_eq!(none[0].events[1].action.as_deref(), Some("click"));

        let consented =
            build_packets(&session, &PackConfig { max_gap_secs: 300, content_consent: true, full_capture: false });
        let s0 = consented[0].events[0].content_snippet.clone().unwrap();
        assert_eq!(s0.len(), 500);
        assert!(!s0.contains("secret"));
        assert_eq!(consented[0].events[1].content_snippet.as_deref(), Some("Rust async traits"));
    }
}
