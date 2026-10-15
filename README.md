<p align="center">
  <img src="docs/logo.svg" alt="hem logo">
</p>

# hem: Human-readable Event Monitor

*tail, but tailored.*

`hem app.log` opens a full-screen, mouse-aware terminal UI that makes it
obvious **where things went wrong** in a log, not just what the last lines
say, and stays instant on multi-gigabyte files.

![hem jumping between errors, searching, filtering by level, opening a line's details, and listing the rarest log templates](docs/demo.gif)

- **Instant on huge files.** Indexed in the background: a 5 GB log is
  scrollable in ~15–25 ms and fully indexed in ~2–3 s.
- **Never gets in the writer's way.** hem reads files without memory-mapping
  them, so the program writing the log can still truncate, rotate or delete
  it while hem is open, on Windows too.
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

## Quick start

```sh
hem app.log                         # open in the TUI
hem app.log nginx.log --merge       # one stream, interleaved by timestamp
hem 'logs/*.log'                    # globs are expanded by hem (quote them)
cat app.log | hem                   # stdin
hem app.log.gz                      # transparent gunzip
hem app.log -f                      # follow (tail -F), survives rotation
hem app.log --since 15m --level warn --filter timeout
hem app.log --level error --plain | wc -l
hem templates app.log               # most common line templates, as a table
```

Inside the TUI press `?` for help, `:` for the command palette.

## Keys

The defaults (all configurable, see [Configuration](#configuration)):

| Key | Action |
|---|---|
| `j`/`k`, `↑`/`↓`, wheel | scroll |
| `g` / `G` | top / bottom |
| `PgUp`/`PgDn`, `Ctrl+u`/`Ctrl+d` | page / half page |
| `h` / `←` / `→` | scroll sideways (when not wrapping) |
| `f` | toggle follow (auto-enabled when you hit the bottom of a growing file) |
| `/` | search (regex, smart case) · `n` / `N` next / previous match · `Ctrl+l` clear |
| `&` | filter: show only lines matching a regex |
| `!` | exclude lines matching a regex |
| `=` | field filter (`status>=500`, `user=bob`, `msg~time.?out`, `trace_id?`) |
| `F` | filter editor (include / exclude / fields / level / time range) |
| `X` | clear all filters |
| `l` | cycle minimum level |
| `e` / `E` | next / previous error |
| `c` | toggle repeat collapsing |
| `x` | expand / collapse a repeated run inline |
| `t` | templates view (`s` sort column, `r` reverse, `/` filter, `Enter` show occurrences) |
| `Enter` | line detail (pretty JSON, fields, context; `Tab` picks a field, `*` highlights its value) |
| `p` | pin / unpin line: pinned lines stay in a strip above the log and show as `◆` on the minimap |
| `]` / `[` | next / previous pinned line (wraps around) |
| `*` | highlight all lines containing a value (request id, thread, trace id…) |
| `y` / `Y` | copy line / copy visible lines (system clipboard, OSC 52 over SSH) |
| `w` | toggle wrap |
| `C` | toggle field columns (JSON / logfmt): a table with a header row; `←` / `→` scroll sideways |
| `T` | cycle timestamp display (original, time, iso, local, relative) |
| `Tab` / `Shift+Tab` | switch source tabs · `S` source picker · `o` open a file |
| `L` | go to line / percentage / time (`1234`, `50%`, `10:30`, `2026-10-13T10:30:00Z`, `15m`) |
| `:` or `Ctrl+p` | command palette |
| `,` | settings |
| `?` | help (generated from the live keymap) |
| `q` / `Esc` | close modal / leave templates view / quit |

Everything is clickable too: tabs, filter chips (click ✕ to remove one),
rows (click to select, click again for detail), pinned lines (click to jump),
the minimap (click or drag to jump), the scrollbar, modal items and buttons.

## Filters

The full guide, with examples and a workflow for incidents spanning many
files, is in [docs/filtering.md](docs/filtering.md). The short version:

Filters compose: every include pattern must match, no exclude pattern may
match, the level must be at or above the threshold, the timestamp must be in
range, and every field predicate must hold.

- **Patterns** are regexes with smart case (all-lowercase → case-insensitive).
  `--literal` / the "literal" toggle in the filter editor treats them as
  plain text. In the filter editor, separate several patterns with `&&`.
- **Levels**: `trace`, `debug`, `info`, `warn`, `error`, `fatal` (aliases
  `warning`, `critical`, `panic`, `err`, numeric pino / syslog levels). A
  line without a level inherits the previous line's, so stack traces stay
  with their error.
- **Times** (`--since`, `--until`): any timestamp format hem recognises
  (`2026-10-13T10:30:00Z`, `2026-10-13 10:30`, `13/Oct/2026:10:30:00 +0000`,
  `Oct 13 10:30:00`, epoch seconds/millis), a time of day (`10:30:00`,
  today), or a relative age (`15m`, `2h`, `1d`, `now`, `today`).
- **Field predicates** work on JSON, logfmt, nginx (`remote_addr`, `user`,
  `method`, `path`, `status`, `bytes`, `user_agent`…) and syslog (`host`,
  `app`, `pid`, `msg`) sources:

  | Syntax | Meaning |
  |---|---|
  | `key=value` | equal (numeric or case-insensitive string) |
  | `key!=value` | not equal (missing key counts as not equal) |
  | `key<n` `key<=n` `key>n` `key>=n` | numeric (falls back to string order) |
  | `key~regex` `key!~regex` | regex match / non-match |
  | `key?` | key exists |

  Nested JSON keys are flattened with dots: `ctx.user=bob`.
- **Templates**: pick one in the templates view and `Enter` filters to its
  occurrences; it appears as a chip like any other filter.

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

## Configuration

`%APPDATA%\hem\config.toml` on Windows, `~/.config/hem/config.toml`
elsewhere (`--config` overrides). Every key is optional; unknown keys are
reported with their line number instead of being ignored.

```toml
theme = "dark"            # or "light"

[colors]                  # override any theme colour: #rrggbb, #rgb, an ANSI name, or 0-255
error = "#ff5d8f"
timestamp = "gray"
# names: bg fg dim accent border border_focus selection cursor timestamp
#        trace debug info warn error fatal json_key string number url match
#        current_match highlight pin status_bg status_fg modal_bg modal_border
#        sparkline minimap_bg

[keys]                    # action = "key" or ["key", "key"]; replaces the default binding
next_error = ["e", "ctrl+n"]
quit = "ctrl+q"
# key syntax: a, G, ctrl+p, alt+x, shift+tab, pgup, pgdn, home, end, enter, esc, space, f1..f12
# actions: scroll_up scroll_down scroll_left scroll_right page_up page_down half_page_up
#   half_page_down top bottom toggle_follow search next_match prev_match clear_search filter
#   exclude field_filter filter_editor clear_filters cycle_level next_error prev_error
#   toggle_collapse expand templates detail pin next_pin prev_pin clear_pins highlight_value
#   clear_highlight copy copy_view
#   toggle_wrap toggle_columns cycle_timestamp next_tab prev_tab sources open_file goto palette
#   settings help close quit sort_templates sort_templates_reverse refresh

[defaults]
collapse = true           # collapse repeated lines
wrap = false
follow = false
timestamp = "original"    # original | time | iso | local | relative
ascii = false             # ASCII box drawing / symbols
mouse = true
columns = false           # field columns for JSON/logfmt sources
minimap = true
cluster_cap = 2000000     # lines sampled for templates (0 = all)

[presets.errors]          # hem --preset errors, or "Preset: errors" in the palette
level = "warn"
include = ["timeout"]
exclude = ["healthz"]
fields = ["status>=500"]
since = "1h"

[formats.myapp]           # hem --format myapp
base = "plain"            # plain | json | logfmt | syslog | nginx
level_regex = '\[(\w+)\]'          # first capture group is the level
timestamp_regex = '^(\d{8}T\d{6})' # first capture group is parsed as a timestamp
```

Environment: `NO_COLOR` disables colour; `HEM_COLORS=16|256|truecolor|mono`
forces a colour depth; `HEM_OSC52=1` forces OSC 52 clipboard.

## Performance

Measured with `cargo run --release --example perf -- <file>` on synthetic
logs from `cargo run --release --example genlog -- --size 5G` (mixed plain /
JSON / logfmt / nginx lines, error bursts, repeats, stack traces).
Machine: Windows 11, Ryzen (32 threads), NVMe SSD (Viper VP4300L), files in
page cache after generation.

| | 1 GB (13.4 M lines) | 5 GB (67.2 M lines) |
|---|---|---|
| Open → first screen scrollable | 27 ms | 13–25 ms |
| Full line index (background) | 0.47 s (2.3 GB/s) | 1.7–3.0 s (1.8–3.1 GB/s) |
| Index memory | 108 MB (8.0 B/line) | 537 MB (8.0 B/line) |
| Timeline / statistics pass | 0.50 s | 2.5 s |
| Regex search, first hit | 3.6 ms | 2.7 ms |
| Regex search, complete | 0.20 s | 1.2 s |
| Build collapsed view | 0.65 s | 3.5 s |
| Jump anywhere + fetch a screen | 0.02 ms | 0.02 ms |

These are the default paged reads. `--mmap` memory-maps instead: on the 5 GB
file it finishes a search in 0.40 s instead of 1.2 s and the timeline pass in
1.9 s instead of 2.5 s, but while it is mapped, other programs can't
truncate the file on Windows, and a truncation can crash hem on Linux and
macOS. Use it only for static files. `cargo run --release --example perf --
<file> --mmap` compares the two.

Cold-cache numbers are bounded by disk read speed (an NVMe SSD at ~3 GB/s
indexes 5 GB in roughly 2–4 s). Redraws take well under a millisecond for a
full screen; the event loop coalesces to 60 fps and idles at 200 ms polls.

Many files: `cargo run --release --example merge_perf -- <dir>` merges
every `*.log` in a directory. On 300 files of 4 MB (1.26 GB, 15.5 M lines,
same machine), indexing all of them takes 0.2 s, the timestamp merge 1.2 s
and the merged view 1.2 s. Merges take up to 4096 files, and the page cache
stays under ~256 MB however many are open.

Micro-benchmarks: `cargo bench` (`benches/index.rs`, `benches/search.rs`;
set `HEM_BENCH_FILE=path` to index a real file).

## Development

```sh
cargo test --workspace                 # unit, integration and TestBackend rendering tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run --release --example genlog -- --size 1G --out testdata/big-1g.log
cargo run -- testdata/big-1g.log --debug   # writes ./hem-debug.log
```

Layout: `crates/hem-core` (no terminal deps: indexing, parsing, search,
clustering, timeline, merge), `crates/hem-tui` (ratatui app), `src/main.rs`
(CLI, plain mode, `templates` subcommand), `tests/`, `benches/`, `testdata/`.
Design notes and trade-offs are in [DECISIONS.md](DECISIONS.md).

Releases: bump the version in `Cargo.toml`, commit, then push a `vX.Y.Z` tag.
The tag runs two workflows:

- `.github/workflows/release.yml` (cargo-dist) builds all six targets,
  uploads them to GitHub Releases and generates the shell / PowerShell
  installers.
- `.github/workflows/publish.yml` runs the tests, then publishes `hem-core`,
  `hem-tui` and `hem` to crates.io in that order through crates.io Trusted
  Publishing (no token stored in the repository). Versions already on
  crates.io are skipped, so re-running the workflow is safe.
- `.github/workflows/demo.yml` builds the tagged version, re-records
  `docs/demo.gif` from `docs/demo.tape` with
  [VHS](https://github.com/charmbracelet/vhs) and commits it to `main`. It
  can also be run by hand from the Actions tab. To change the demo, edit the
  tape; to preview it locally, see the comment at the top of the tape.

## License

MIT OR Apache-2.0, at your option.
