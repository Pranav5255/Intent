//! Ingest load generator / synthetic day seeder for a running `intentd`.
//!
//! ```text
//! cargo run --release -p intentd --example ingest_load -- \
//!     [--url http://127.0.0.1:9477] [--n 5000] [--concurrency 32] [--date YYYY-MM-DD]
//! ```
//! Posts `n` unique, valid events (shell / focus / editor / browser mix over a few
//! projects) and reports events/s and the status-code histogram. With `--date`, the
//! events are spread over that local day (08:00 onwards, ~14 s apart) so the result is
//! a realistic multi-project day for pipeline benchmarks.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use chrono::{Local, NaiveDate, TimeZone};
use serde_json::{Value, json};

struct Args {
    url: String,
    n: usize,
    concurrency: usize,
    date: Option<String>,
}

fn args() -> Args {
    let mut a = Args { url: "http://127.0.0.1:9477".into(), n: 5000, concurrency: 32, date: None };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let v = it.next().unwrap_or_default();
        match flag.as_str() {
            "--url" => a.url = v,
            "--n" => a.n = v.parse().expect("--n"),
            "--concurrency" => a.concurrency = v.parse().expect("--concurrency"),
            "--date" => a.date = Some(v),
            _ => panic!("unknown flag {flag}"),
        }
    }
    a
}

fn uuid(seed: u64, i: usize) -> String {
    let mut x = seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let h = format!("{:016x}{:016x}", next(), next());
    format!("{}-{}-4{}-a{}-{}", &h[0..8], &h[8..12], &h[13..16], &h[17..20], &h[20..32])
}

const PROJECTS: &[(&str, &str, &str)] = &[
    ("/home/lima/src/intent", "crates/intentd/src/ingest.rs", "docs.rs/axum"),
    ("/home/lima/src/website", "src/pages/index.tsx", "developer.mozilla.org"),
    ("/home/lima/src/infra", "terraform/main.tf", "registry.terraform.io"),
];
const COMMANDS: &[&[&str]] = &[
    &["cargo test -p intentd", "cargo clippy --workspace", "git status", "git commit -m wip"],
    &["npm run dev", "npm test", "git diff", "pnpm lint"],
    &["terraform plan", "terraform apply", "kubectl get pods", "git push"],
];

fn event(i: usize, ts: i64, seed: u64) -> Value {
    // Switch project every ~40 events so the day has several distinct tasks.
    let p = (i / 40) % PROJECTS.len();
    let (root, file, domain) = PROJECTS[p];
    let id = uuid(seed, i);
    match i % 5 {
        0 => json!({"id": id, "ts": ts, "source": "shell", "type": "command",
                    "payload": {"cmd": COMMANDS[p][i / 5 % 4], "cwd": root, "exit_code": if i.is_multiple_of(17) { 1 } else { 0 }}}),
        1 => json!({"id": id, "ts": ts, "source": "linux", "type": "app_focus",
                    "payload": {"app": if i.is_multiple_of(2) { "Code" } else { "Firefox" }, "title": format!("{file} - {root}")}}),
        2 => json!({"id": id, "ts": ts, "source": "vscode", "type": "file_save", "payload": {"path": format!("{root}/{file}")}}),
        3 => json!({"id": id, "ts": ts, "source": "firefox", "type": "tab_change",
                    "payload": {"url": format!("https://{domain}/page/{}", i % 7), "title": format!("{domain} docs"), "tab_id": p, "window_id": 1}}),
        _ => json!({"id": id, "ts": ts, "source": "vscode", "type": "file_open", "payload": {"path": format!("{root}/{file}")}}),
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let a = args();
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64 | 1;
    let start_ts = match &a.date {
        Some(d) => {
            let date = NaiveDate::parse_from_str(d, "%Y-%m-%d").expect("--date YYYY-MM-DD");
            Local.from_local_datetime(&date.and_hms_opt(8, 0, 0).unwrap()).earliest().unwrap().timestamp()
        }
        None => chrono::Utc::now().timestamp(),
    };
    let step = if a.date.is_some() { 14 } else { 0 };
    let client = reqwest::Client::new();
    let next = Arc::new(AtomicUsize::new(0));
    let codes: Arc<std::sync::Mutex<BTreeMap<u16, usize>>> = Arc::default();
    let started = Instant::now();
    let mut workers = Vec::new();
    for _ in 0..a.concurrency.max(1) {
        let (client, next, codes, url) = (client.clone(), Arc::clone(&next), Arc::clone(&codes), format!("{}/v1/event", a.url));
        let n = a.n;
        workers.push(tokio::spawn(async move {
            loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    break;
                }
                let body = event(i, start_ts + (i as i64) * step, seed);
                let code = match client.post(&url).json(&body).send().await {
                    Ok(r) => r.status().as_u16(),
                    Err(_) => 0,
                };
                *codes.lock().unwrap().entry(code).or_default() += 1;
            }
        }));
    }
    for w in workers {
        w.await.unwrap();
    }
    let secs = started.elapsed().as_secs_f64();
    println!(
        "{}",
        json!({"events": a.n, "concurrency": a.concurrency, "seconds": (secs * 1000.0).round() / 1000.0,
               "events_per_second": (a.n as f64 / secs).round(), "status_codes": *codes.lock().unwrap()})
    );
}
