# Intent shell integration

`intent_os_shell_send.py` reads four NUL-delimited fields from standard input:
command, working directory, exit code, and duration in milliseconds. It emits a
best-effort local event and always exits successfully, so instrumentation cannot
change shell behaviour.

The bash/zsh snippets are opt-in package payloads. The future `intent-osctl
enable-shell` command adds a marked source line to the matching rc file; disable
removes only that line. Commands matching likely secret patterns are stored as
`<redacted>`.
