//! Deterministic conversion of capture events into pipeline events (0.1 `normalize.py`).

use std::collections::HashSet;

use intent_types::{
    COMMAND_FAMILIES, ContextEvidence, EventEntities, EventSignals, Family, FileKind, NormalizedEvent, Payload,
    PipelineWarning, RawEvent, WarningLevel,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::util::{basename, collapse_whitespace, hex16};

pub const MAX_CONTEXT_TERMS: usize = 64;

const CODE_EXTENSIONS: &[&str] = &[
    // 0.1 set
    ".py", ".js", ".ts", ".go", ".java", ".cpp", ".c", ".tf", ".yaml", ".json", ".sh",
    // 0.2 additions (only affect the `file_kinds` aggregate)
    ".rs", ".tsx", ".jsx", ".yml", ".toml", ".rb", ".kt", ".swift", ".h", ".hpp", ".cs", ".php", ".sql",
];
const IMAGE_EXTENSIONS: &[&str] = &[".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".bmp"];
const TODO_MARKERS: &[&str] = &["TODO", "FIXME", "XXX"];
const CONTEXT_STOP_WORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "this", "that", "into", "true", "false", "http", "https", "www", "com", "org",
    "net", "text", "insert", "delete", "replace",
];

/// Supported workflow family for a shell command's first token.
pub fn derive_command_family(command: &str) -> Option<String> {
    let first = command.split_whitespace().next()?.to_lowercase();
    COMMAND_FAMILIES.iter().find(|f| **f == first).map(|f| (*f).to_owned())
}

/// Classify a path by extension (Python `PurePath.suffix` semantics).
pub fn derive_file_kind(file_path: &str) -> Option<FileKind> {
    if file_path.is_empty() {
        return None;
    }
    let name = basename(file_path).unwrap_or("");
    let extension = match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => name[i..].to_lowercase(),
        _ => String::new(),
    };
    Some(if CODE_EXTENSIONS.contains(&extension.as_str()) {
        FileKind::Code
    } else if extension == ".pdf" {
        FileKind::Pdf
    } else if IMAGE_EXTENSIONS.contains(&extension.as_str()) {
        FileKind::Image
    } else {
        FileKind::Other
    })
}

/// Authority of an HTTP(S) URL without retaining the rest of it.
pub fn extract_domain_from_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    (!authority.is_empty()).then(|| authority.to_owned())
}

/// Concise activity headline built from extracted metadata only.
pub fn generate_normalized_text(
    event_type: &str,
    source: &str,
    family: Family,
    entities: &EventEntities,
    signals: &EventSignals,
) -> String {
    match family {
        Family::Editor => {
            if let Some(name) = &entities.file_name {
                format!("Edited {name}")
            } else if signals.typed_chars != 0 {
                format!("Typed {} chars", signals.typed_chars)
            } else {
                "Edited file".into()
            }
        }
        Family::Browser => match &entities.domain {
            Some(domain) => format!("Viewed {domain}"),
            None => "Viewed browser page".into(),
        },
        Family::Command => {
            let mut text = match &entities.command_family {
                Some(f) => format!("Ran {f}"),
                None => "Ran command".into(),
            };
            if let Some(code) = entities.exit_code.filter(|c| *c != 0) {
                text.push_str(&format!(" (exit code {code})"));
            }
            text
        }
        Family::Focus => match &entities.title {
            Some(title) => format!("Focused on {title}"),
            None => "Focused on application".into(),
        },
        Family::Idle => "Idle period".into(),
        Family::FileChange => match &entities.file_path {
            Some(path) => format!("File modified: {path}"),
            None => "File modified".into(),
        },
        Family::Other => format!("Observed {source}/{event_type}"),
    }
}

/// Normalize one event. `ordinal` is the event's index in the original export.
pub fn normalize_event(raw: &RawEvent, ordinal: usize) -> NormalizedEvent {
    let source = raw.source.trim().to_lowercase();
    let category = raw.kind.trim().to_lowercase();
    let family = Family::from_source(&source, &category);
    let evidence = extract_evidence(&raw.payload);
    let entities = extract_entities(&raw.payload, &evidence);
    let signals = extract_signals(&raw.payload, &category);
    let text = generate_normalized_text(&category, &source, family, &entities, &signals);
    NormalizedEvent {
        id: raw.id.clone(),
        ts: raw.ts,
        ordinal,
        source,
        family,
        category,
        text,
        entities,
        signals,
        evidence,
        raw: raw_view(raw),
    }
}

/// The 0.1 `raw` shape (`{id, ts, source, type, payload, schema_version}`), which the
/// semantic packer and resume builder read through `raw.payload`.
fn raw_view(raw: &RawEvent) -> Payload {
    let mut map = Map::new();
    map.insert("id".into(), Value::String(raw.id.clone()));
    map.insert("ts".into(), Value::from(raw.ts));
    map.insert("source".into(), Value::String(raw.source.clone()));
    map.insert("type".into(), Value::String(raw.kind.clone()));
    map.insert("payload".into(), Value::Object(raw.payload.clone()));
    map.insert("schema_version".into(), Value::from(raw.schema_version));
    map
}

/// Sort by `(ts, original index)`, drop duplicate ids (first wins), normalize.
pub fn normalize_events(raw_events: &[RawEvent]) -> (Vec<NormalizedEvent>, Vec<PipelineWarning>) {
    let mut order: Vec<usize> = (0..raw_events.len()).collect();
    order.sort_by_key(|&i| (raw_events[i].ts, i));
    let mut seen: HashSet<&str> = HashSet::with_capacity(raw_events.len());
    let mut events = Vec::with_capacity(raw_events.len());
    let mut warnings = Vec::new();
    for index in order {
        let raw = &raw_events[index];
        if raw.id.is_empty() {
            warnings.push(PipelineWarning {
                level: WarningLevel::Warning,
                message: "Normalization skipped an event without an id".into(),
                event_id: None,
            });
            continue;
        }
        if !seen.insert(raw.id.as_str()) {
            continue;
        }
        events.push(normalize_event(raw, index));
    }
    (events, warnings)
}

/// Deterministic 16-hex-char identifier of the normalized input.
///
/// Everything else on a [`NormalizedEvent`] is derived from these fields by this
/// engine version (which is itself part of the pipeline cache key), so the payload is
/// hashed exactly once, in a canonical compact form (sorted keys, no whitespace),
/// streamed into the hasher without building an intermediate document.
pub fn compute_source_hash(events: &[NormalizedEvent]) -> String {
    let mut hasher = Sha256::new();
    for event in events {
        hasher.update(b"[");
        write_canonical(&Value::String(event.id.clone()), &mut hasher);
        hasher.update(format!(",{},{},", event.ts, event.ordinal).as_bytes());
        write_canonical(&Value::String(event.source.clone()), &mut hasher);
        hasher.update(b",");
        write_canonical(&Value::String(event.category.clone()), &mut hasher);
        hasher.update(b",");
        match event.raw.get("payload") {
            Some(payload) => write_canonical(payload, &mut hasher),
            None => hasher.update(b"{}"),
        }
        hasher.update(b"]\n");
    }
    hex16(&hasher.finalize())
}

fn write_canonical(value: &Value, hasher: &mut Sha256) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            hasher.update(b"{");
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    hasher.update(b",");
                }
                hasher.update(serde_json::to_string(key).unwrap_or_default().as_bytes());
                hasher.update(b":");
                write_canonical(&map[key], hasher);
            }
            hasher.update(b"}");
        }
        Value::Array(items) => {
            hasher.update(b"[");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    hasher.update(b",");
                }
                write_canonical(item, hasher);
            }
            hasher.update(b"]");
        }
        scalar => hasher.update(serde_json::to_string(scalar).unwrap_or_default().as_bytes()),
    }
}

/// Local diagnostic rendering; never a provider prompt.
pub fn intelligence_text(event: &NormalizedEvent, ordinal: Option<usize>) -> String {
    let prefix = ordinal.map(|o| format!("{o}. ")).unwrap_or_default();
    let mut lines = vec![format!("{prefix}[{}/{}] {}", event.source, event.category, event.text)];
    lines.extend(event.evidence.iter().map(|e| format!("  {}: {}", e.field, e.value)));
    lines.join("\n")
}

fn string_field(payload: &Payload, key: &str) -> Option<String> {
    payload.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Integer (not bool, not float) as i64.
fn strict_int(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(n)) => n.as_i64(),
        _ => None,
    }
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn extract_entities(payload: &Payload, evidence: &[ContextEvidence]) -> EventEntities {
    let file_path = string_field(payload, "path");
    let command = string_field(payload, "cmd");
    let url = string_field(payload, "url");
    EventEntities {
        project_paths: project_paths(payload),
        file_name: file_path.as_deref().and_then(basename).map(str::to_owned),
        file_kind: file_path.as_deref().and_then(derive_file_kind),
        file_path,
        domain: url.as_deref().and_then(extract_domain_from_url),
        title: string_field(payload, "title"),
        command_family: command.as_deref().and_then(derive_command_family),
        command,
        cwd: string_field(payload, "cwd"),
        exit_code: strict_int(payload.get("exit_code")),
        context_terms: context_terms(evidence),
    }
}

fn extract_signals(payload: &Payload, event_type: &str) -> EventSignals {
    let typed_chars = match strict_int(payload.get("typed_chars")) {
        Some(n) if n >= 0 => n,
        _ => document_change_characters(payload),
    };
    EventSignals {
        typed_chars,
        save: event_type.contains("save"),
        todo_added: truthy(payload.get("todo_added")) || document_change_has_todo(payload),
    }
}

fn project_paths(payload: &Payload) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for key in ["cwd", "workspace", "folder"] {
        if let Some(value) = string_field(payload, key)
            && !paths.contains(&value)
        {
            paths.push(value);
        }
    }
    paths
}

fn changes(payload: &Payload) -> impl Iterator<Item = &Map<String, Value>> {
    payload.get("changes").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_object)
}

fn document_change_characters(payload: &Payload) -> i64 {
    let mut total = 0;
    for change in changes(payload) {
        match strict_int(change.get("text_length")) {
            Some(n) if n >= 0 => total += n,
            _ => {
                if !truthy(change.get("redacted"))
                    && let Some(text) = change.get("text").and_then(Value::as_str)
                {
                    total += text.chars().count() as i64;
                }
            }
        }
    }
    total
}

/// Inserted (non-redacted) change texts of an editor payload.
pub(crate) fn inserted_texts(payload: &Payload) -> impl Iterator<Item = &str> {
    changes(payload).filter(|c| !truthy(c.get("redacted"))).filter_map(|c| c.get("text").and_then(Value::as_str))
}

fn document_change_has_todo(payload: &Payload) -> bool {
    inserted_texts(payload).any(|text| {
        let upper = text.to_uppercase();
        TODO_MARKERS.iter().any(|m| upper.contains(m))
    })
}

/// Flatten every payload value into local evidence (`a.b`, `list[0]` field names).
///
/// Local, consent-approved data only: this must never cross a provider boundary.
pub fn extract_evidence(payload: &Payload) -> Vec<ContextEvidence> {
    let mut evidence = Vec::new();
    visit_object("", payload, &mut evidence);
    evidence
}

fn visit_object(field: &str, map: &Map<String, Value>, out: &mut Vec<ContextEvidence>) {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    for key in keys {
        let name = if field.is_empty() { key.clone() } else { format!("{field}.{key}") };
        visit(&name, &map[key], out);
    }
}

fn visit(field: &str, value: &Value, out: &mut Vec<ContextEvidence>) {
    let rendered = match value {
        Value::Object(map) => return visit_object(field, map, out),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                visit(&format!("{field}[{index}]"), item, out);
            }
            return;
        }
        Value::Null => return,
        Value::Bool(b) => (if *b { "true" } else { "false" }).to_owned(),
        Value::String(s) => collapse_whitespace(s),
        Value::Number(n) => n.to_string(),
    };
    if !rendered.is_empty() {
        out.push(ContextEvidence { field: field.to_owned(), value: rendered });
    }
}

/// Stable topic tokens (`[a-z][a-z0-9_-]{2,}` over lowercased values), stop words
/// removed, first 64 distinct.
pub fn context_terms(evidence: &[ContextEvidence]) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for item in evidence {
        for token in tokenize_terms(&item.value.to_lowercase()) {
            if terms.len() >= MAX_CONTEXT_TERMS {
                return terms;
            }
            if !CONTEXT_STOP_WORDS.contains(&token) && seen.insert(token.to_owned()) {
                terms.push(token.to_owned());
            }
        }
    }
    terms
}

/// Equivalent of `re.findall(r"[A-Za-z][A-Za-z0-9_-]{2,}", text)`.
pub(crate) fn tokenize_terms(text: &str) -> Vec<&str> {
    let is_body = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if !is_body(c) {
            continue;
        }
        let mut end = start + c.len_utf8();
        while let Some(&(i, next)) = chars.peek() {
            if !is_body(next) {
                break;
            }
            end = i + next.len_utf8();
            chars.next();
        }
        let run = &text[start..end];
        if let Some(letter) = run.find(|ch: char| ch.is_ascii_alphabetic()) {
            let token = &run[letter..];
            if token.len() >= 3 {
                out.push(token);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(id: &str, ts: i64, source: &str, kind: &str, payload: Value) -> RawEvent {
        RawEvent {
            id: id.into(),
            ts,
            source: source.into(),
            kind: kind.into(),
            payload: payload.as_object().cloned().unwrap_or_default(),
            schema_version: 1,
            ingested_at: None,
        }
    }

    #[test]
    fn command_family_uses_shared_table() {
        assert_eq!(derive_command_family("Git status").as_deref(), Some("git"));
        assert_eq!(derive_command_family("cargo test").as_deref(), Some("cargo"));
        assert_eq!(derive_command_family("ls -la"), None);
        assert_eq!(derive_command_family("   "), None);
    }

    #[test]
    fn file_kinds_follow_purepath_suffix() {
        assert_eq!(derive_file_kind("/a/b.PY"), Some(FileKind::Code));
        assert_eq!(derive_file_kind("/a/b.pdf"), Some(FileKind::Pdf));
        assert_eq!(derive_file_kind("x.svg"), Some(FileKind::Image));
        assert_eq!(derive_file_kind("/home/.bashrc"), Some(FileKind::Other));
        assert_eq!(derive_file_kind("README"), Some(FileKind::Other));
        assert_eq!(derive_file_kind(""), None);
    }

    #[test]
    fn domains() {
        assert_eq!(extract_domain_from_url("https://docs.rs/x?y").as_deref(), Some("docs.rs"));
        assert_eq!(extract_domain_from_url("http://a.com?q=1").as_deref(), Some("a.com"));
        assert_eq!(extract_domain_from_url("https://a.com#frag").as_deref(), Some("a.com"));
        assert_eq!(extract_domain_from_url("ftp://a.com"), None);
        assert_eq!(extract_domain_from_url("https://"), None);
    }

    #[test]
    fn evidence_is_sorted_flattened_and_whitespace_normalised() {
        let payload = json!({"z": "a\n  b", "a": {"y": [1, true, null, {"k": "v"}]}, "e": ""});
        let ev = extract_evidence(payload.as_object().unwrap());
        let pairs: Vec<(String, String)> = ev.into_iter().map(|e| (e.field, e.value)).collect();
        assert_eq!(
            pairs,
            vec![
                ("a.y[0]".into(), "1".into()),
                ("a.y[1]".into(), "true".into()),
                ("a.y[3].k".into(), "v".into()),
                ("z".into(), "a b".into()),
            ]
        );
    }

    #[test]
    fn tokenizer_matches_python_regex() {
        assert_eq!(tokenize_terms("12abc de fgh-ij_k x9y"), vec!["abc", "fgh-ij_k", "x9y"]);
        assert_eq!(tokenize_terms("ab é abc"), vec!["abc"]);
    }

    #[test]
    fn context_terms_drop_stop_words_and_cap() {
        let ev = vec![ContextEvidence { field: "t".into(), value: "The asyncio timeout and https asyncio".into() }];
        assert_eq!(context_terms(&ev), vec!["asyncio", "timeout"]);
        let many = (0..100).map(|i| format!("term{i}")).collect::<Vec<_>>().join(" ");
        let ev = vec![ContextEvidence { field: "t".into(), value: many }];
        assert_eq!(context_terms(&ev).len(), MAX_CONTEXT_TERMS);
    }

    #[test]
    fn normalizes_shell_and_editor() {
        let ev = normalize_event(
            &raw("1", 5, " Shell ", "Command", json!({"cmd": "pytest -q", "cwd": "/p", "exit_code": 1})),
            3,
        );
        assert_eq!(ev.family, Family::Command);
        assert_eq!(ev.category, "command");
        assert_eq!(ev.text, "Ran pytest (exit code 1)");
        assert_eq!(ev.entities.project_paths, vec!["/p"]);
        assert_eq!(ev.ordinal, 3);
        assert_eq!(ev.raw["payload"]["cmd"], "pytest -q");

        let ev = normalize_event(
            &raw(
                "2",
                6,
                "vscode",
                "document_save",
                json!({"path": "/p/a.py", "workspace": "/p", "changes": [{"text": "# todo x"}, {"text": "secret", "redacted": true}]}),
            ),
            0,
        );
        assert_eq!(ev.text, "Edited a.py");
        assert!(ev.signals.save);
        assert!(ev.signals.todo_added);
        assert_eq!(ev.signals.typed_chars, 8);
        assert_eq!(ev.entities.file_kind, Some(FileKind::Code));
    }

    #[test]
    fn exit_code_must_be_integer() {
        let ev = normalize_event(&raw("1", 5, "shell", "command", json!({"cmd": "git", "exit_code": true})), 0);
        assert_eq!(ev.entities.exit_code, None);
        let ev = normalize_event(&raw("1", 5, "shell", "command", json!({"cmd": "git", "exit_code": 1.5})), 0);
        assert_eq!(ev.entities.exit_code, None);
    }

    #[test]
    fn batch_sorts_dedupes_and_keeps_original_ordinal() {
        let events = vec![
            raw("b", 20, "shell", "command", json!({})),
            raw("a", 10, "shell", "command", json!({})),
            raw("a", 30, "shell", "command", json!({})),
            raw("", 1, "shell", "command", json!({})),
        ];
        let (out, warnings) = normalize_events(&events);
        assert_eq!(out.iter().map(|e| (e.id.as_str(), e.ordinal)).collect::<Vec<_>>(), vec![("a", 1), ("b", 0)]);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn source_hash_is_canonical_and_sensitive() {
        let a = raw("1", 1, "shell", "command", json!({"cmd": "ls", "cwd": "/"}));
        let mut b = a.clone();
        b.payload = json!({"cwd": "/", "cmd": "ls"}).as_object().unwrap().clone();
        let ha = compute_source_hash(&normalize_events(std::slice::from_ref(&a)).0);
        let hb = compute_source_hash(&normalize_events(&[b]).0);
        assert_eq!(ha, hb);
        assert_eq!(ha.len(), 16);
        let mut c = a;
        c.payload.insert("exit_code".into(), json!(1));
        assert_ne!(ha, compute_source_hash(&normalize_events(&[c]).0));
    }
}
