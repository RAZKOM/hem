<p align="center">
  <img src="docs/logo.svg" alt="hem logo">
</p>

# hem: Human-readable Event Monitor

*tail, but tailored.*

`hem app.log` opens a full-screen, mouse-aware terminal UI that makes it
obvious **where things went wrong** in a log, not just what the last lines
say, and stays instant on multi-gigabyte files.

- **Instant on huge files.** Memory-mapped, indexed in the background:
  a 5 GB log is scrollable in ~13 ms and fully indexed in ~2 s.
- **Answers "what happened?"** A timeline minimap coloured by severity,
  repeat collapsing with counts and sparklines, next/previous error, and a
  template view that surfaces rare lines.
- **Feels like an app.** Panes, tabs, clickable filter chips, a command
  palette, modals, smooth 60 fps redraws, dark and light themes.
- **Behaves as a Unix tool.** Files, globs, stdin and `.gz`; degrades to
  colourised plain output when piped; sensible exit codes.
- **Read-only.** hem never modifies the files it opens.

## Install

| Channel | Command |
|---|---|
| Shell installer (macOS/Linux) | `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/RAZKOM/hem/releases/latest/download/hem-installer.sh \| sh` |
| PowerShell installer (Windows) | `powershell -ExecutionPolicy Bypass -c "irm https://github.com/RAZKOM/hem/releases/latest/download/hem-installer.ps1 \| iex"` |
| cargo-binstall | `cargo binstall hem` |
| cargo | `cargo install hem` |
| Prebuilt archives | [GitHub Releases](https://github.com/RAZKOM/hem/releases): Windows x64/ARM64, macOS x64/ARM64, Linux x64/ARM64 (musl) |

From source: `cargo install --path .` (Rust 1.89+).

## Formats

Detected per source from a sample (override with `--format`): JSON lines,
logfmt, nginx/Apache combined, syslog (RFC 3164/5424), plain. Timestamps are
found in ISO 8601 / RFC 3339, `YYYY-MM-DD HH:MM:SS[,fff]`, `YYYY/MM/DD`,
syslog, Apache/nginx bracket form and epoch seconds/millis/micros/nanos
(including JSON `time`/`ts`/`timestamp`/`@timestamp` fields).

## Plain mode

When stdout is not a terminal, or with `--plain`, hem prints lines instead
of starting the TUI. Colour is used when stdout is a terminal (respecting
`NO_COLOR`), or with `--color`.

```sh
hem app.log --level error --plain | wc -l          # count errors (with their stack traces)
hem app.log --filter 'user=bob' -x health --plain   # include + exclude
hem api.jsonl --field 'status>=500' --field 'path~^/api' --plain
hem a.log b.log --merge --plain                     # interleaved, with a source column
hem app.log --collapse --plain                      # repeated runs become "×N line"
hem app.log -f --plain                              # like tail -F, with filters
hem templates app.log -n 20 --rare                  # the 20 rarest templates
hem templates app.log --json | jq .
```

Exit codes: `0` ok, `1` runtime error (unreadable file, bad regex, bad
config), `2` usage error.

