# Decisions

Design decisions made where the spec was silent or where a trade-off was
needed. Items marked **review** deserve a look from the owner.

## Architecture

- **Files are read, not memory-mapped, by default.** Reads go through
  `pread` / `seek_read` with an LRU of 64 KiB pages per file (`paged.rs`):
  at most 16 MiB per file and 256 MiB across all files (a file may always
  keep 1 MiB), so hundreds of open files cannot grow the caches to
  gigabytes. When a miss follows the previous page, four pages are read in
  one call; building a merged view walks every file a few lines at a time,
  and one read per page made that several times slower than a mapping.
  Big sequential reads (indexing, view building, search) bypass the cache. A mapping would stop other processes from truncating the
  file on Windows and can SIGBUS on Unix when the file is truncated. The
  cost: search over 5 GB takes ~1.2 s instead of ~0.4 s and the timeline
  pass ~2.5 s instead of ~1.9 s; first screen, indexing and scrolling are
  effectively unchanged. `--mmap` (`SourceOptions::mmap`) restores mapping
  for static files. memmap2 stays a dependency for that opt-in.
- **Chunks own or share their bytes.** `view::Chunk` holds one buffer per
  source: shared for in-memory data (zero copy), or an owned copy for paged
  files. Consecutive lines are read with one call; scattered filtered rows
  go through the page cache. Source locks are released before any file I/O.
- **One `Source` abstraction for files, stdin and gzip.** stdin and `.gz`
  inputs are streamed into a temp file which is then read (paged) and
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
- **Rows are 8 bytes.** `line:36 | source:12 | count:16` packed into a
  `u64`. Limits: 2^36 (68 G) lines per source, 4096 sources per merge,
  repeat counts cap at 65 535 (a longer run just starts a new row). The
  source field was 8 bits (256 files), which real incidents spanning a few
  hundred files ran into.
- **Completion is read before length.** Every background producer appends
  its rows before it sets its "complete" flag, so consumers (view builder,
  search, template mining, the merge, plain mode) read the flag first and
  the length second. The other order lost rows: a consumer that read the
  length, then saw the producer complete, marked itself done with the
  rows appended in between unprocessed. Merges of five or more files hit
  this and stopped after one 32 768-row chunk.
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
- **Template mining is Drain with a few tweaks.**
  - Lines are routed on the first four *non-wildcard* tokens (timestamps at
    the start of a line would otherwise be useless keys), so different
    messages land in different leaves.
  - Similarity ignores positions that are wildcards on both sides, and a
    line that fits a template exactly is a perfect match. Without this,
    lines made mostly of variable parts each started their own template.
  - `key=value` pairs keep their key when the value varies: templates show
    `user=<*>`, not a bare `<*>`.
  - JSON / logfmt lines are clustered on their `level + msg` field instead
    of the raw line. This is decided per line (`Format::for_line`), so the
    odd JSON or logfmt line inside a plain log is handled too; the same
    per-line refinement drives field filters and the line detail view.
  - Inputs larger than `defaults.cluster_cap` (2 M lines) are sampled with
    a fixed stride and the UI / `hem templates` say so; counts are scaled
    estimates.
  - Known Drain limitation: a variable word among the first four literal
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
  The merge is a k-way merge on a binary heap (it used to scan every source
  for each line, which grew with the file count), parses timestamps for
  several sources in parallel, and buffers at most 4 M pending lines across
  all sources (64 MB) rather than 128 k per source.
- **Tabs build their views when first shown.** Each file is a tab, and with
  `--merge` over hundreds of files only the merged tab is looked at;
  building every per-file view up front scanned all the data twice. A
  tab's view starts (`View::resume`) the first time it is the active tab.
- **Custom formats** from the config (`[formats.<name>]`) are applied
  process-wide: a `level_regex` / `timestamp_regex` whose first capture
  group is used before the built-in detectors. Selected with
  `--format <name>`.

## Terminal UI

- **The minimap colours density, not presence.** Each row is a horizontal
  bar (length = line count, log scale) coloured by the most severe level
  that is unusually dense there: more than three standard deviations above
  that level's average per row, or simply present when the level occurs
  less than once per row on average. Plain "highest severity per bucket"
  painted every row red as soon as errors were sprinkled through a log,
  hiding the bursts the minimap exists to show.
- **Repeat sparklines need timestamps.** A collapsed run is consecutive, so
  without timestamps its members are evenly spread by construction and the
  sparkline would always be a solid bar; such rows show only the `×N` badge.
- **Pins are sticky.** A pinned line is shown in a strip above the log (up
  to five, then "N more"), marked on the minimap, and reachable with `]` /
  `[`. Pins are stored as `(source, line)`, so they survive filter changes;
  a pin the filters hide stays in the strip, dimmed and marked "filtered
  out". Positions in the view are resolved on each tick: a binary search
  for single-source views, an incremental scan (2 M rows per tick) for
  merged ones, whose rows are ordered by time rather than line.
- **Field columns are sampled once per view**, from ~200 rows at the
  current position plus ~200 spread over the view, and kept until the
  filters change. Columns used to be re-derived from whatever rows were on
  screen, so they changed while scrolling and had no header. A column is a
  key present in at least a fifth of the sampled structured lines, as wide
  as 90 % of its values (max 32); timestamp and level come first and the
  message last and unpadded. Timestamp, level and message aliases (`time` /
  `ts`, `msg` / `message`, …) share a column so merged sources line up.
  Collapsed runs get a fixed-width count column instead of a badge, which
  would shift the cells.
- **Scrolling is row-based**, not display-line based. With wrap on, a long
  top row shows from its first display line; `top` always points at a row.
  This keeps every scroll operation O(1) and avoids re-measuring rows.
- **Expanding a collapsed run** shows up to 500 member lines inline (`x`);
  the detail view (`Enter`) shows the representative line with fields and
  context. Members are found by scanning forward from the representative
  line (bounded to 200 k lines), which is exact for single-source views and
  approximate for merged ones.
- **Prompts are inline, modals are centred.** Search / filter / go-to use a
  one-line prompt in the filter-bar slot (like Vim / less); everything else
  is a centred, dimmed-background modal with the focus trapped inside.
  Clicking outside a modal closes it.
- **The tab bar and source picker scroll.** With more tabs than fit, the
  tab bar shows a window around the active tab with "‹ N more" / "N more ›"
  markers (clickable, one tab per click); the source picker scrolls to keep
  the selection in view.
- **The keymap is the single source of truth** for the help modal and the
  command palette; config overrides replace an action's bindings entirely.
- **Settings auto-save** to the config file on every change (and `s` saves
  explicitly). Saving rewrites the whole file through the serializer, so
  comments in a hand-written config are lost. **review**: switch to
  `toml_edit` if comment preservation matters.
- **Colour depth** is detected from `COLORTERM`, `WT_SESSION`,
  `TERM_PROGRAM` and `TERM`; `HEM_COLORS=16|256|truecolor|mono` overrides.
  On Windows with none of those set we assume 24-bit colour (conhost on
  Windows 10+ supports it). `NO_COLOR` drops to a monochrome theme that uses
  bold / reverse / underline only.
- **Mouse clicks are hit-tested against regions recorded during render**
  (`App::hits`), which keeps the renderer and the input code decoupled and
  makes clickable elements testable with `TestBackend`.
- **Redraws are coalesced to ~60 fps**; when nothing is running the event
  loop polls every 200 ms, so idle CPU is effectively 0 %. Background jobs
  set a dirty flag instead of pushing events.
- **Copy** uses `arboard` and falls back to OSC 52; over SSH (`SSH_TTY`
  set) or with `HEM_OSC52=1` it goes straight to OSC 52 so the *local*
  terminal gets the text.

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

## Releases

- Distribution is GitHub Releases (cargo-dist) and crates.io only.
- crates.io publishing lives in its own workflow (`publish.yml`), not in
  cargo-dist's `release.yml`, so regenerating the dist workflow cannot drop
  it. `dist-workspace.toml` keeps `publish-jobs = []` for the same reason.
- Publishing uses crates.io Trusted Publishing (GitHub OIDC exchanged for a
  short-lived token) instead of a stored `CARGO_REGISTRY_TOKEN` secret. The
  job runs in the `crates-io` GitHub environment so required reviewers can
  be added there later without touching the workflow.
- The job refuses to publish when the tag does not match the `hem` version
  in `Cargo.toml`, and skips crates whose version is already on crates.io,
  so a partially failed run can simply be re-run.
- The README demo is a VHS tape (`docs/demo.tape`) rendered in CI on every
  tag (`demo.yml`) and committed to `main` by `github-actions[bot]`, so the
  GIF always matches the latest release. It renders on Linux with a
  deterministic synthetic log from `examples/genlog.rs`. If `main` gets
  branch protection, the job's direct push needs an exception (or switch it
  to opening a pull request).

## Platform notes

- **Writers stay in control of their files.** With the default paged
  reads, another process can truncate (`copytruncate` rotation), rename or
  delete a file while hem has it open, on every platform; tests cover
  truncate-then-append and delete on Windows. Reads past a new, shorter end
  come back zero-filled until the follower's next poll (≤ 250 ms) notices
  and re-indexes. Rotation by rename is detected via file identity
  (`same-file`: volume + file index on Windows, dev + inode on Unix).
- **With `--mmap` the old limits return:** Windows refuses to truncate a
  file while hem maps it ("user-mapped section open"), and on Unix a
  truncation can fault the mapping (SIGBUS) before the next poll.
- Deleting a file while hem has it open: hem keeps showing what it already
  indexed and reports the removal as a notice.

## Dependencies worth justifying

- `ratatui` + `crossterm`: as specified. `ratatui` 0.30 pulls in several
  sub-crates (`ratatui-core`, `ratatui-widgets`, …) but no extra system deps.
- `arboard` (default features off): system clipboard. On Linux it links
  X11/Wayland client libraries at runtime only; nothing extra is needed to
  build. OSC 52 is the fallback everywhere.
- `aho-corasick`: level keyword scanning (already a transitive dependency of
  `regex`).
- `same-file`: file identity for rotation detection; tiny.
- `flate2` (miniz_oxide backend): `.gz` support, pure Rust.
- `notify` 8 with default features off except `macos_fsevent`: filesystem
  events, with a 250 ms polling heartbeat as the fallback.
- `chrono` (std + clock features only): user-facing time formatting and
  input parsing; the hot-path timestamp parser is hand written.
- `criterion` with default features off: benchmarks only.

## Known gaps

- Follow-mode throughput at 10 k lines/s has not been measured on a real
  writer; the design (250 ms poll + notify, incremental index/view/search)
  is built for it. The 5 GB index runs at ~2.5 GB/s from page cache.
- `--debug` logs actions and notices, not every event.
- Light theme and 16-colour degradation have been checked by eye in tests
  only (`TestBackend`), not on a wide range of real terminals.
