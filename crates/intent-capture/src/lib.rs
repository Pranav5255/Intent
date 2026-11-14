//! Capture-side security boundary for Intent 2.0.
//!
//! Pure, synchronous building blocks that `intentd` wires behind its HTTP routes:
//!
//! - [`validate`]: strict event validation (`EventIn` → [`RawEvent`]).
//! - [`redact`]: server-side final redaction (URLs, document text, shell commands).
//! - [`blocklist`]: domain blocklist (`blocked-domains.yaml`).
//! - [`config`]: consent / workspace / filesystem / retention configs plus the cached
//!   [`config::ConfigStore`].
//! - [`admit`]: the ingest policy pipeline (consent → redaction → blocklist).
//! - [`secrets`]: the single sensitive-path rule set.
//! - [`paths`]: the single XDG path resolver.
//! - [`restore`]: restore planning and launching.
//! - [`preview`]: the static intelligence preview sample.
//!
//! [`RawEvent`]: intent_types::RawEvent

pub mod admit;
pub mod atomic;
pub mod blocklist;
pub mod config;
pub mod paths;
pub mod preview;
pub mod redact;
pub mod restore;
pub mod secrets;
pub mod url;
pub mod validate;

pub use admit::{Admission, AdmissionPolicy, DropReason, admit};
pub use blocklist::Blocklist;
pub use config::{ConfigError, ConfigStore};
pub use paths::Paths;
pub use redact::{RedactionError, redact_event, redact_shell_command};
pub use validate::{EventIn, ValidationError, validate_event};
