# Decisions

Design decisions made where the spec was silent or where a trade-off was
needed. Items marked **review** deserve a look from the owner.

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

