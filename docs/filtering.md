# Filtering in hem

This guide covers how hem narrows a log down to the lines you care about,
from a single file up to a few hundred files and many gigabytes.

## Five tools, five jobs

hem has several ways to "find" lines. They do different things, and most
sessions use more than one.

| Tool | Key | What it does to the view |
|---|---|---|
| **Filter** | `&` `!` `=` `l` `F` | Removes lines. Only matching lines are shown, counted, and drawn on the minimap. |
| **Search** | `/`, then `n` / `N` | Removes nothing. Marks matches and jumps between them. |
| **Highlight** | `*` | Removes nothing. Colours every occurrence of a value, such as a request id. |
| **Pin** | `p`, then `]` / `[` | Keeps a line on screen in the pinned strip above the log, marks it on the minimap (`◆`), and lets you jump back to it. |
| **Templates** | `t` | Groups similar lines. `Enter` on a template filters to its occurrences. |

A good rule: **filter** to cut the noise, **search** inside what is left,
**pin** the lines you will want to come back to.

## How filters combine

Every active filter appears as a chip in the bar under the log. Click a
chip's `✕` to remove it, or press `X` to clear them all.

Filters are combined with AND:

- every include pattern must match,
- no exclude pattern may match,
- the level must be at or above the threshold,
- the timestamp must be inside the time range,
- every field predicate must hold.

To get OR, put it in one regex: `&` `timeout|refused|reset` shows lines
containing any of the three words.

Filters belong to a tab. Each file you open is its own tab with its own
filters, and so is the merged tab. A tab you open from inside hem (`o`, or
merging from the palette) starts with the current tab's filters.

## Text filters: `&` include, `!` exclude

Both take a regular expression ([Rust regex syntax](https://docs.rs/regex/latest/regex/#syntax)).

- **Smart case.** An all-lowercase pattern ignores case (`timeout` matches
  `Timeout` and `TIMEOUT`). A pattern with any upper-case letter is
  case-sensitive (`ERROR` does not match `error`).
- **Literal text.** Turn on "literal" in the filter editor (`F`), or pass
  `--literal`, to match text exactly. Use it when a pattern is full of
  regex characters, like `GET /api/v1/orders?id=42`. Without it you would
  need `GET /api/v1/orders\?id=42`.
- **Several at once.** Press `&` again to add another include, or type
  several in the filter editor separated by `&&`.

Examples:

```text
&  payment                       lines mentioning payment (any case)
&  user=(bob|alice)\b            bob's or alice's lines, not "bobby"
!  healthz|readyz|metrics        hide probe noise
!  ^\s*at                        hide Java stack frames
&  \b5\d\d\b                     any 5xx status written in the text
```

## Level: `l`

`l` cycles the minimum level: all, then debug, info, warn, error, fatal,
then back to all. On the command line use `--level warn` (or `-l warn`).

Recognised levels: `trace`, `debug`, `info`, `warn`, `error`, `fatal`, plus
`warning`, `critical`, `panic`, `err`, and numeric pino (10 to 60) and syslog
(0 to 7) levels in a `level` field.

**Inheritance.** A line with no level of its own (a stack frame, the second
line of a wrapped message) takes the level and timestamp of the line above
it. So `level >= error` keeps the whole stack trace under an `ERROR` line,
not just its first line.

Upper-case `ERROR` is found anywhere near the start of a line. Lower-case
`error` only counts at the start of the line or right after `[ < = : " ' (`,
so prose like "a connection error happened" is not treated as an error.

## Time range

Set it in the filter editor (`F`) or with `--since` / `--until`.

| You type | Meaning |
|---|---|
| `15m`, `2h`, `1d` | that long ago |
| `now`, `today` | right now / since midnight |
| `10:30` or `10:30:00` | that time today |
| `2026-10-13 10:30` | that minute |
| `2026-10-13T10:30:00Z` | an exact instant |
| `13/Oct/2026:10:30:00 +0000` | nginx / Apache form |
| `Oct 13 10:30:00` | syslog form |
| `1760351400` | epoch seconds (millis work too) |

Timestamps without a time zone are treated as UTC.

`L` (go to) takes the same formats when you only want to jump to a time
without hiding anything.

## Field filters: `=`

Field filters work on structured lines: JSON, logfmt, nginx / Apache access
logs and syslog. Press `=` and type a predicate, or pass `--field` (repeat
it to add more).

| Syntax | Meaning | Example |
|---|---|---|
| `key=value` | equal | `user=bob`, `status=404` |
| `key!=value` | not equal (a missing key counts as not equal) | `env!=prod` |
| `key>n` `key>=n` `key<n` `key<=n` | compare | `status>=500`, `duration_ms>1000` |
| `key~regex` | value matches a regex | `path~^/api/v2`, `msg~time.?out` |
| `key!~regex` | value does not match | `user_agent!~bot` |
| `key?` | key exists | `trace_id?`, `error?` |

Details worth knowing:

- **Numbers compare as numbers** when both sides are numeric (`9 < 10`,
  `1.5e3 = 1500`), including JSON strings that hold a number. Otherwise
  values compare as text.
- **Equality ignores case** for text: `level=ERROR` matches `"level":"error"`.
- **Nested JSON** keys are flattened with dots: `{"ctx":{"user":"bob"}}` is
  `ctx.user=bob`. Arrays are kept as JSON text, so match inside them with a
  regex: `tags~beta`.
- **Lines without the key** fail every predicate except `!=` and `!~`.
  `status>=500` therefore also hides plain-text lines in a mixed log.
- **Field names for text formats**:
  - nginx / Apache: `remote_addr`, `ident`, `user`, `time`, `request`,
    `method`, `path`, `protocol`, `status`, `bytes`, `referer`,
    `user_agent`
  - syslog: `host`, `app`, `pid`, `msg`
- **Not sure what the keys are called?** Open a line with `Enter`. The
  detail view lists every field with its value. `Tab` picks a field and `*`
  highlights its value everywhere.

## The filter editor: `F`

`F` shows every filter of the current tab in one form: include, exclude,
fields, level, since, until, literal, then Apply and Clear.

- `Tab` / `↓` and `Shift+Tab` / `↑` move between rows.
- In the include, exclude and fields rows, separate several entries with
  `&&`: `timeout && retry` adds two include patterns (both must match).
- On the level row, `←` / `→` or `Space` changes the level.
- `Enter` applies (on the literal row it toggles; on Clear it clears
  everything). `Esc` closes without applying.
- A bad regex or time shows an error in the editor instead of closing it.

## Templates as filters: `t`

The templates view groups lines that differ only in their variable parts
(numbers, ids, quoted strings) and counts each group. Sort by count
ascending (`s` to pick the column, `r` to reverse) to find the rare lines:
one-off errors, the first occurrence of something new. `Enter` filters the
log to that template; it shows up as a chip you can remove like any other.

From the command line: `hem templates app.log --rare -n 20`.

## Presets

Filters you use often can live in the config file:

```toml
[presets.errors]
level = "warn"
include = ["timeout"]
exclude = ["healthz"]
fields = ["status>=500"]
since = "1h"
```

Apply one with `hem --preset errors`, or pick "Preset: errors" from the
command palette (`:`).

## Working across many files

When the problem spans services or hosts, put the files into one stream:

```sh
hem 'logs/**/*.log' --merge          # quote globs; hem expands them itself
hem logs/*.log.gz --merge            # gzip works the same way
```

`--merge` (or "Merge all sources by timestamp" in the palette) adds a **merged** tab that
interleaves every file by timestamp and shows each line's source in a
column. A merged view takes at most 256 files.

A workflow that works on large incidents:

1. **Narrow the window first.** `--since` / `--until`, or click the burst on
   the minimap and note the time. Shrinking the time range is the cheapest
   filter and makes everything after it faster.
2. **Cut the noise.** `l` to `warn` or `error`, then `!` for the known-noisy
   lines (`healthz|metrics|heartbeat`).
3. **Find the thread.** Open a suspicious line (`Enter`), pick its request
   id or trace id with `Tab`, and press `*` to highlight it across every
   file. Or filter on it: `=` `trace_id=abc123`.
4. **Pin as you go.** `p` on the first error, the deploy line, the config
   reload. Pinned lines stay in the strip above the log while you scroll,
   show as `◆` on the minimap, and `]` / `[` jumps between them. A pin that
   the current filters hide stays in the strip, marked "filtered out".
5. **Loosen and compare.** `X` clears the filters but keeps your pins, so
   you can see what happened around them.

The same filters work outside the TUI, which is handy for sharing or
counting:

```sh
hem 'logs/*.log' --merge --since 10:00 --until 10:15 --level error --plain > incident.txt
hem api.jsonl --field 'status>=500' --field 'path~^/api' --plain | wc -l
```

## Field columns: `C`

For JSON and logfmt logs, `C` lays the fields out as a table. The columns
are picked once from a sample of the view (fields present in at least a
fifth of the lines, timestamp and level first, the message last) and stay
the same while you scroll; they are picked again when the filters change.
A header row names each column. Fields that did not get a column follow the
message as dim `key=value` pairs. Use `←` / `→` (or `h`) to scroll sideways;
the header scrolls with the rows and shows `◂` when you are scrolled.

In a merged view, `time` / `ts` / `timestamp`, `level` / `severity` and
`msg` / `message` share a column, so sources that name them differently
still line up.
