//! Pure event shaping for the filesystem watcher: batch classification and
//! per-path rate limiting (trailing edge, so the last write is never lost).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use notify::EventKind;
use notify::event::{ModifyKind, RenameMode};
use notify_debouncer_full::DebouncedEvent;
use tokio::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FsAction {
    /// Created, written or renamed into place. May be a file or a directory.
    Touched(PathBuf),
    /// Deleted or renamed away.
    Removed(PathBuf),
}

/// Turn one debounced batch into de-duplicated actions (first-seen order).
pub fn classify_batch(events: &[DebouncedEvent]) -> Vec<FsAction> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |a: FsAction| {
        if seen.insert(a.clone()) {
            out.push(a);
        }
    };
    for ev in events {
        let paths = &ev.event.paths;
        match ev.event.kind {
            EventKind::Create(_)
            | EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Any | ModifyKind::Other)
            | EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
                paths.iter().cloned().map(FsAction::Touched).for_each(&mut push);
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
                if let [from, to, ..] = paths.as_slice() {
                    push(FsAction::Removed(from.clone()));
                    push(FsAction::Touched(to.clone()));
                }
            }
            EventKind::Modify(ModifyKind::Name(_)) => {
                // `From`, `Any`, `Other`: the path no longer exists under that name.
                paths.iter().cloned().map(FsAction::Removed).for_each(&mut push);
            }
            EventKind::Remove(_) => paths.iter().cloned().map(FsAction::Removed).for_each(&mut push),
            // Metadata-only changes and reads are noise.
            EventKind::Modify(ModifyKind::Metadata(_)) | EventKind::Access(_) | EventKind::Any | EventKind::Other => {}
        }
    }
    out
}

/// At most one report per path per `interval`; further writes inside the
/// window are reported once the window ends.
#[derive(Debug)]
pub struct Coalescer {
    interval: Duration,
    last: HashMap<PathBuf, Instant>,
    dirty: HashSet<PathBuf>,
}

impl Coalescer {
    pub fn new(interval: Duration) -> Self {
        Self { interval, last: HashMap::new(), dirty: HashSet::new() }
    }

    /// Returns true if the path should be reported now.
    pub fn touch(&mut self, path: PathBuf, now: Instant) -> bool {
        match self.last.get(&path) {
            Some(&t) if now.saturating_duration_since(t) < self.interval => {
                self.dirty.insert(path);
                false
            }
            _ => {
                self.dirty.remove(&path);
                self.last.insert(path, now);
                true
            }
        }
    }

    pub fn forget(&mut self, path: &std::path::Path) {
        self.dirty.remove(path);
        self.last.remove(path);
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.dirty.iter().filter_map(|p| self.last.get(p)).map(|t| *t + self.interval).min()
    }

    /// Paths whose window has elapsed and that were written during it.
    pub fn due(&mut self, now: Instant) -> Vec<PathBuf> {
        let interval = self.interval;
        let ready: Vec<PathBuf> = self
            .dirty
            .iter()
            .filter(|p| self.last.get(*p).is_none_or(|t| now.saturating_duration_since(*t) >= interval))
            .cloned()
            .collect();
        for p in &ready {
            self.dirty.remove(p);
            self.last.insert(p.clone(), now);
        }
        // Keep memory bounded: forget clean paths whose window is over.
        if self.last.len() > 4096 {
            let dirty = &self.dirty;
            self.last.retain(|p, t| dirty.contains(p) || now.saturating_duration_since(*t) < interval);
        }
        ready
    }

    pub fn len(&self) -> usize {
        self.last.len()
    }

    pub fn is_empty(&self) -> bool {
        self.last.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, DataChange, MetadataKind, RemoveKind};
    use notify::{Event, EventKind};

    fn ev(kind: EventKind, paths: &[&str]) -> DebouncedEvent {
        let mut e = Event::new(kind);
        for p in paths {
            e = e.add_path(PathBuf::from(p));
        }
        DebouncedEvent::new(e, std::time::Instant::now())
    }

    #[test]
    fn batch_classification_dedupes() {
        let batch = vec![
            ev(EventKind::Create(CreateKind::File), &["/w/a.rs"]),
            ev(EventKind::Modify(ModifyKind::Data(DataChange::Content)), &["/w/a.rs"]),
            ev(EventKind::Modify(ModifyKind::Data(DataChange::Any)), &["/w/a.rs"]),
            ev(EventKind::Modify(ModifyKind::Metadata(MetadataKind::Permissions)), &["/w/b.rs"]),
            ev(EventKind::Access(notify::event::AccessKind::Any), &["/w/c.rs"]),
            ev(EventKind::Modify(ModifyKind::Name(RenameMode::Both)), &["/w/.a.rs.tmp", "/w/d.rs"]),
            ev(EventKind::Modify(ModifyKind::Name(RenameMode::To)), &["/w/e.rs"]),
            ev(EventKind::Modify(ModifyKind::Name(RenameMode::From)), &["/w/old"]),
            ev(EventKind::Remove(RemoveKind::Folder), &["/w/gone"]),
            ev(EventKind::Create(CreateKind::Folder), &["/w/newdir"]),
        ];
        let t = |p: &str| FsAction::Touched(PathBuf::from(p));
        let r = |p: &str| FsAction::Removed(PathBuf::from(p));
        assert_eq!(
            classify_batch(&batch),
            vec![
                t("/w/a.rs"),
                r("/w/.a.rs.tmp"),
                t("/w/d.rs"),
                t("/w/e.rs"),
                r("/w/old"),
                r("/w/gone"),
                t("/w/newdir")
            ]
        );
    }

    #[test]
    fn coalescer_is_trailing_edge() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_secs(n);
        let a = PathBuf::from("/w/a");
        let mut c = Coalescer::new(Duration::from_secs(10));
        assert!(c.touch(a.clone(), s(0)), "first write reported immediately");
        assert_eq!(c.next_deadline(), None);
        assert!(!c.touch(a.clone(), s(2)));
        assert!(!c.touch(a.clone(), s(4)));
        assert_eq!(c.next_deadline(), Some(s(10)));
        assert!(c.due(s(9)).is_empty());
        assert_eq!(c.due(s(10)), vec![a.clone()], "burst collapsed into one trailing report");
        assert_eq!(c.next_deadline(), None);
        assert!(!c.touch(a.clone(), s(15)), "window restarts at the trailing report");
        assert!(c.touch(a.clone(), s(21)), "write after the window is reported directly");
        assert_eq!(c.next_deadline(), None, "and clears the pending trailing report");
        let b = PathBuf::from("/w/b");
        assert!(c.touch(b.clone(), s(22)));
        c.forget(&b);
        assert!(c.touch(b, s(23)));
    }
}
