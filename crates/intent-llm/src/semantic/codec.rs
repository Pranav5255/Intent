//! Lossless compact encoding of complete Role A events (full-capture consent only).
//! Port of 1.x `_full_capture_prompt`: positional records, ts deltas, a type legend,
//! key aliases and a string dictionary that is only used where it saves bytes.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Map, Value, json};

use super::RequestPacket;

const KEY_ALIASES: &[(&str, &str)] = &[
    ("action", "a"),
    ("app", "b"),
    ("author", "c"),
    ("blocked", "d"),
    ("changes", "e"),
    ("character", "f"),
    ("cmd", "g"),
    ("context", "h"),
    ("cwd", "i"),
    ("end", "j"),
    ("exit_code", "k"),
    ("folder", "l"),
    ("href", "m"),
    ("input_type", "n"),
    ("kind", "o"),
    ("language", "p"),
    ("line", "q"),
    ("mime", "r"),
    ("path", "s"),
    ("range", "t"),
    ("redacted", "u"),
    ("removed_characters", "v"),
    ("save", "w"),
    ("schema_version", "x"),
    ("scroll", "y"),
    ("sensitive_page", "z"),
    ("start", "A"),
    ("tab_id", "B"),
    ("target", "C"),
    ("text", "D"),
    ("text_excerpt", "E"),
    ("text_length", "F"),
    ("title", "G"),
    ("url", "H"),
    ("window_id", "I"),
    ("workspace", "J"),
    ("checked", "K"),
    ("direction", "L"),
    ("position_bucket", "M"),
    ("role", "N"),
    ("size_bytes", "O"),
    ("sha256", "P"),
    ("excerpt", "Q"),
    ("ingested_at", "R"),
];

struct Parts {
    id: Value,
    source: String,
    kind: String,
    payload: Value,
    envelope: Map<String, Value>,
}

fn parts(event: &super::pack::PacketEvent) -> Parts {
    let captured = event.captured_event.clone().unwrap_or_default();
    let id = captured.get("id").cloned().unwrap_or_else(|| json!(event.event_id));
    let mut envelope: Map<String, Value> = captured
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "id" | "ts" | "source" | "type" | "payload"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let source = match captured.get("source") {
        Some(Value::String(s)) => s.clone(),
        other => {
            if let Some(v) = other {
                envelope.insert("source".into(), v.clone());
            }
            event.source.clone()
        }
    };
    let kind = match captured.get("type") {
        Some(Value::String(s)) => s.clone(),
        other => {
            if let Some(v) = other {
                envelope.insert("type".into(), v.clone());
            }
            event.category.clone()
        }
    };
    let payload = match captured.get("payload") {
        Some(p @ Value::Object(_)) => p.clone(),
        other => {
            if let Some(v) = other {
                envelope.insert("payload".into(), v.clone());
            }
            json!({})
        }
    };
    if let Some(ts) = captured.get("ts")
        && ts.as_i64() != Some(event.ts)
    {
        envelope.insert("ts".into(), ts.clone());
    }
    Parts { id, source, kind, payload, envelope: envelope.into_iter().collect() }
}

fn json_size(v: &Value) -> usize {
    serde_json::to_string(v).map(|s| s.len()).unwrap_or(0)
}

fn count_strings(v: &Value, counts: &mut HashMap<String, usize>, order: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            let c = counts.entry(s.clone()).or_insert_with(|| {
                order.push(s.clone());
                0
            });
            *c += 1;
        }
        Value::Array(a) => a.iter().for_each(|x| count_strings(x, counts, order)),
        Value::Object(m) => m.values().for_each(|x| count_strings(x, counts, order)),
        _ => {}
    }
}

fn count_keys(v: &Value, counts: &mut HashMap<String, usize>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                *counts.entry(k.clone()).or_default() += 1;
                count_keys(x, counts);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| count_keys(x, counts)),
        _ => {}
    }
}

fn encode(
    v: &Value,
    strings: &HashMap<String, usize>,
    aliases: &HashMap<String, String>,
    alias_values: &[String],
) -> Value {
    match v {
        Value::String(s) => match strings.get(s) {
            Some(i) => json!(format!("@{i}")),
            None if s.starts_with('@') => json!(format!("@{s}")),
            None => v.clone(),
        },
        Value::Array(a) => Value::Array(a.iter().map(|x| encode(x, strings, aliases, alias_values)).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| {
                    let key = match aliases.get(k) {
                        Some(a) => a.clone(),
                        None if alias_values.contains(k) => format!("!{k}"),
                        None => k.clone(),
                    };
                    (key, encode(x, strings, aliases, alias_values))
                })
                .collect(),
        ),
        _ => v.clone(),
    }
}

pub fn full_capture_prompt(packets: &[&RequestPacket]) -> String {
    let events: Vec<&super::pack::PacketEvent> = packets.iter().flat_map(|p| p.packet.events.iter()).collect();
    let base_ts = events.first().map(|e| e.ts).unwrap_or(0);
    let all_parts: Vec<Parts> = events.iter().map(|e| parts(e)).collect();

    let mut counts = HashMap::new();
    let mut order = Vec::new();
    let mut key_counts = HashMap::new();
    for p in &all_parts {
        let env = Value::Object(p.envelope.clone());
        count_strings(&p.payload, &mut counts, &mut order);
        count_strings(&env, &mut counts, &mut order);
        count_keys(&p.payload, &mut key_counts);
        count_keys(&env, &mut key_counts);
    }
    let mut dictionary: Vec<String> = Vec::new();
    for value in order {
        let count = counts[&value];
        let raw = json_size(&json!(value));
        let reference = json_size(&json!(format!("@{}", dictionary.len())));
        if count * raw > raw + count * reference {
            dictionary.push(value);
        }
    }
    let string_index: HashMap<String, usize> = dictionary.iter().enumerate().map(|(i, s)| (s.clone(), i)).collect();

    let mut aliases: HashMap<String, String> = HashMap::new();
    for (key, alias) in KEY_ALIASES {
        if let Some(&count) = key_counts.get(*key) {
            let raw = json_size(&json!(key));
            let a = json_size(&json!(alias));
            if count * raw > count * a + a + 1 + raw {
                aliases.insert((*key).to_owned(), (*alias).to_owned());
            }
        }
    }
    let alias_values: Vec<String> = aliases.values().cloned().collect();

    let mut type_legend: Vec<[String; 2]> = Vec::new();
    let mut descriptors = Vec::new();
    let mut records = Vec::new();
    let mut offset = 0usize;
    let mut idx = 0usize;
    for packet in packets {
        descriptors.push(json!([packet.id, packet.packet.role.as_str(), offset, packet.packet.events.len()]));
        for event in &packet.packet.events {
            let p = &all_parts[idx];
            idx += 1;
            let type_key = [p.source.clone(), p.kind.clone()];
            let type_index = match type_legend.iter().position(|t| *t == type_key) {
                Some(i) => i,
                None => {
                    type_legend.push(type_key);
                    type_legend.len() - 1
                }
            };
            records.push(json!([
                encode(&p.id, &string_index, &aliases, &alias_values),
                event.ts - base_ts,
                type_index,
                encode(&p.payload, &string_index, &aliases, &alias_values),
                encode(&Value::Object(p.envelope.clone()), &string_index, &aliases, &alias_values),
            ]));
            offset += 1;
        }
    }
    let key_legend: BTreeMap<String, String> = aliases.iter().map(|(k, a)| (a.clone(), k.clone())).collect();
    let prompt = json!({
        "v": 2,
        "base_ts": base_ts,
        "key_legend": key_legend,
        "type_legend": type_legend,
        "dictionary": dictionary,
        "packet_format": ["packet_id", "deterministic_role", "event_offset", "event_count"],
        "output_packet_ids": packets.iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
        "packets": descriptors,
        "events": records,
        "record_format": ["id", "ts_delta", "type_index", "payload", "envelope"],
        "encoding": "type_legend[type_index] is [source,type]; raw ts=base_ts+ts_delta; envelope contains every other raw event field; key_legend keys are aliases, ! escapes a literal colliding key, @N means dictionary[N], @@ escapes a literal @",
    });
    prompt.to_string()
}
