# Decisions

Design decisions made where the spec was silent or where a trade-off was
needed. Items marked **review** deserve a look from the owner.

## Architecture

- **One `Source` abstraction for files, stdin and gzip.** stdin and `.gz`
  inputs are streamed into a temp file which is then memory-mapped and
  followed exactly like a growing file. This keeps a single indexing path
  and lets the UI show lines while decompression / piping is still running.
  Temp files live in the platform temp dir and are deleted on exit.
- **Line index = `Vec<u64>` of line starts.** Exactly 8 bytes per line
  (the budget in §8). After every `\n` at byte `p` we push `p + 1`, so a
  file that ends with a newline has one extra sentinel offset; `line_count`
  accounts for it. The vector is shrunk to fit once the initial scan
  completes, so capacity doubling does not inflate memory.
- **Background jobs stream in order.** `par::ordered_scan` runs chunk work
  on up to 8 threads and emits results strictly in chunk order, so the view
  builder, search and merge can append to their output vectors as prefixes
  complete. Memory is bounded by a sliding window of `2 × threads` chunks.
- **A view is one pass.** Filtering, repeat collapsing, the timeline
  histogram, level statistics and the time range are all computed in the
  same background pass over the base rows, so the minimap always reflects
  what is actually displayed. Unfiltered, uncollapsed single-source views
  are an "identity" view: rows are virtual (no allocation), only the
  histogram pass runs.
- **Rows are 8 bytes.** `line:40 | source:8 | count:16` packed into a `u64`.
  Limits: 2^40 lines per source, 256 sources per merge, repeat counts cap at
  65 535 (a longer run just starts a new row). **review**: if files with
  more than ~1 T lines are a realistic target, widen the line field.
- **Level inheritance.** A line without a level (stack-trace frame,
  wrapped message) inherits the level and timestamp of the previous line
  for filtering, so `--level error` keeps the trace under an `ERROR` line.
  The histogram and statistics count such lines as "no level" so they do
  not inflate error counts.
- **Timestamps without a zone are treated as UTC.** hem only needs
  timestamps to be mutually consistent for ordering, bucketing and
  `--since/--until`; the original text is what gets displayed (unless the
  timestamp display setting is changed). Year-less syslog timestamps use the
  current year.
- **Lower-case level words need a structural prefix.** `ERROR` is accepted
  anywhere in the first 4 KB of a line; `error` / `Error` only at the start
  of the line or right after `[ < = : " ' (`. This avoids marking prose such
  as "a connection error happened" while still catching `level=error`,
  `"level":"error"` and `[error]`. Numeric pino levels (10–60) and syslog
  severities (0–7) are understood in `level` fields.
- **Format detection needs a clear majority** (> 50 % of a 200-line sample)
  so a few JSON lines inside a plain log do not flip the whole source.
- **Repeat normalisation** masks hex/decimal runs that contain a digit,
  quoted strings and whitespace runs (`collapse::normalize`). Words made of
  hex letters only (`dead`, `cafe`) are kept. The 64-bit FNV hash of the
  normalised form is compared, never stored per line.
- **Template mining is Drain with two tweaks**: lines are routed on the
  first two *non-wildcard* tokens (timestamps at the start of a line would
  otherwise be useless keys), and JSON / logfmt lines are clustered on their
  `level + msg` field instead of the raw line (quoted strings are masked by
  normalisation, which would otherwise fold every JSON line into one
  template). Inputs larger than `defaults.cluster_cap` (2 M lines) are
  sampled with a fixed stride and the UI / `hem templates` say so; counts are
  scaled estimates. Known Drain limitation: a variable word in the first two
  tokens (`User bob …` / `User alice …`) yields separate templates.
- **Search runs over the view, not the file**, so results are row indices
  that `n`/`N` can binary-search. Identity views use a fast path: one
  `find_iter` over each chunk's contiguous bytes instead of one `is_match`
  per line.
- **Merged streams** (`--merge` or the source picker) interleave sources by
  timestamp with per-source timestamp inheritance; a source that has no
  pending lines but is still being written blocks the merge for at most one
  poll interval (250 ms) so follow mode stays live. Lines before the first
  timestamp of a source sort first. Any source rotation resets the merge.
- **Custom formats** from the config (`[formats.<name>]`) are applied
  process-wide: a `level_regex` / `timestamp_regex` whose first capture
  group is used before the built-in detectors. Selected with
  `--format <name>`.

## CLI / plain mode

- `--follow` is `-f` (tail convention); `--filter` has no short flag.
- Plain mode never collapses unless `--collapse` is passed, so
  `hem app.log --level error --plain | wc -l` counts lines.
- Collapsed rows in plain mode print a `×N` prefix; merged streams print a
  padded source name column.
- `--format` accepts the built-in names and any `[formats.<name>]` from the
  config.
- `hem templates` prints a table (or `--json` lines) and uses the same
  sampling cap as the TUI.
- A broken pipe (`| head`) exits 0 silently.

