//! Plain mode: print (optionally coloured) lines to stdout, honouring the
//! same filters as the TUI.  Used when stdout is not a terminal or with
//! `--plain`.

use hem_core::filter::FilterSpec;
use hem_core::source::Source;

pub struct PlainOptions {
    pub sources: Vec<Source>,
    pub spec: FilterSpec,
    pub collapse: bool,
    pub follow: bool,
    pub merge: bool,
    pub color: bool,
}

const RESET: &str = "\x1b[0m";

