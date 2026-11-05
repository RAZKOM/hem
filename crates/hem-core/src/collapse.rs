//! Repeat collapsing: normalise the variable parts of a line so that
//! consecutive "same shape" lines can be folded into one row.
//!
//! Normalisation replaces:
//! * runs of hex digits that contain at least one decimal digit (numbers,
//!   hex ids, UUID segments, IP octets, timestamps) with `#`;
//! * double- or single-quoted strings with `"#"` (template mining only, see
//!   below);
//! * runs of whitespace with a single space, dropping leading and trailing
//!   whitespace.
//!
//! Repeat collapsing ([`shape_hash`]) keeps the words inside quotes. In
//! JSON and logfmt every value is quoted, so masking them made every line
//! with the same keys "the same", and a `"level":"fatal"` line folded into
//! the `"level":"info"` run around it.

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[inline]
fn is_hexish(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

/// Feed the normalised form of `line` into `sink` byte by byte.  With
/// `mask_quotes`, quoted strings become `"#"`; without, their contents are
/// normalised like the rest of the line.
fn normalize_into(line: &[u8], mask_quotes: bool, mut sink: impl FnMut(u8)) {
    let n = line.len();
    let mut i = 0;
    // Whitespace is emitted lazily, as one space before the next non-space
    // byte, so leading and trailing whitespace never reach the sink.
    let mut started = false;
    let mut pending_space = false;
    macro_rules! sink {
        ($b:expr) => {{
            if pending_space && started {
                sink(b' ');
            }
            pending_space = false;
            started = true;
            sink($b);
        }};
    }
    while i < n {
        let b = line[i];
        if is_hexish(b) {
            // Scan the maximal [0-9a-fA-F] run; also swallow a `0x` prefix.
            let mut j = i;
            let mut has_digit = false;
            if b == b'0' && matches!(line.get(i + 1), Some(b'x') | Some(b'X')) {
                j = i + 2;
                has_digit = true;
            }
            while j < n && is_hexish(line[j]) {
                has_digit |= line[j].is_ascii_digit();
                j += 1;
            }
            // A run of letters only (e.g. "dead", "face") is a word unless
            // it is glued to a digit run via `-` (UUID) which we handle
            // below by masking digit-containing segments individually.
            let word_boundary_before = i == 0 || !line[i - 1].is_ascii_alphanumeric();
            let word_boundary_after = j >= n || !line[j].is_ascii_alphanumeric();
            if has_digit && (word_boundary_before || word_boundary_after || j - i >= 4) {
                sink!(b'#');
                i = j;
                continue;
            }
            for &c in &line[i..j] {
                sink!(c);
            }
            i = j;
            continue;
        }
        if mask_quotes && (b == b'"' || b == b'\'') {
            // Quoted string: find the closing quote on this line.
            let q = b;
            let mut j = i + 1;
            let mut closed = false;
            while j < n {
                if line[j] == b'\\' {
                    j += 2;
                    continue;
                }
                if line[j] == q {
                    closed = true;
                    break;
                }
                j += 1;
            }
            if closed && j - i <= 512 {
                sink!(q);
                sink!(b'#');
                sink!(q);
                i = j + 1;
                continue;
            }
            sink!(b);
            i += 1;
            continue;
        }
        if b.is_ascii_whitespace() {
            pending_space = true;
            i += 1;
            continue;
        }
        sink!(b);
        i += 1;
    }
}

/// The normalised form of a line, as bytes, with quoted strings masked
/// (used by template mining).
pub fn normalize(line: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len());
    normalize_into(line, true, |b| out.push(b));
    out
}

/// The form [`shape_hash`] hashes: like [`normalize`], but quoted strings
/// keep their words.
pub fn repeat_shape(line: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len());
    normalize_into(line, false, |b| out.push(b));
    out
}

/// 64-bit hash of a line's repeat shape ([`repeat_shape`]), without
/// allocating.  Lines with the same hash collapse into one row.
pub fn shape_hash(line: &[u8]) -> u64 {
    let mut h = FNV_OFFSET;
    normalize_into(line, false, |b| {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    });
    h
}

/// True if two lines collapse together.
pub fn same_shape(a: &[u8], b: &[u8]) -> bool {
    a == b || shape_hash(a) == shape_hash(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> String {
        String::from_utf8_lossy(&normalize(s.as_bytes())).into_owned()
    }

    #[test]
    fn masks_numbers_and_ids() {
        assert_eq!(n("retry 3 of 10"), "retry # of #");
        assert_eq!(n("req 0x7fff12 done"), "req # done");
        assert_eq!(
            n("id=550e8400-e29b-41d4-a716-446655440000 ok"),
            "id=#-#-#-#-# ok"
        );
        assert_eq!(n("from 10.0.0.12:8080"), "from #.#.#.#:#");
        assert_eq!(n("2026-10-13T10:02:11.204Z x"), "#-#-#T#:#:#.#Z x");
    }

    #[test]
    fn masks_quoted_strings_and_space_runs() {
        assert_eq!(
            n(r#"user "bob smith"   logged in"#),
            r##"user "#" logged in"##
        );
        assert_eq!(n("a\t\tb"), "a b");
        assert_eq!(n("unterminated \"quote here"), "unterminated \"quote here");
    }

    #[test]
    fn words_survive() {
        assert_eq!(n("dead beef cafe"), "dead beef cafe");
        assert_eq!(
            n("ERROR connection reset by peer"),
            "ERROR connection reset by peer"
        );
        assert_eq!(n("abc123def"), "#");
    }

    #[test]
    fn hashes_agree() {
        let a = b"connection reset by peer (attempt 4)";
        let b = b"connection reset by peer (attempt 17)";
        assert!(same_shape(a, b));
        assert!(!same_shape(a, b"connection closed by peer (attempt 4)"));
        assert_eq!(shape_hash(a), shape_hash(&repeat_shape(a)));
        // Trailing whitespace does not change the shape.
        assert!(same_shape(b"disk full", b"disk full  "));
    }

    #[test]
    fn structured_lines_keep_their_values() {
        // Every JSON value is quoted: masking them made all of these one run.
        let info = br#"{"level":"info","msg":"server started"}"#;
        let fatal = br#"{"level":"fatal","msg":"out of memory"}"#;
        let bye = br#"{"level":"info","msg":"bye"}"#;
        assert!(!same_shape(info, fatal));
        assert!(!same_shape(info, bye));
        // Numbers and ids inside quotes still vary freely.
        assert!(same_shape(
            br#"{"level":"info","msg":"retry 3","req":"a1b2c3d4"}"#,
            br#"{"level":"info","msg":"retry 17","req":"ffee0912"}"#,
        ));
        assert!(!same_shape(
            br#"level=info msg="cache warm""#,
            br#"level=error msg="cache cold""#,
        ));
        // Template mining still masks quoted strings.
        assert_eq!(n(r#"user "bob" in"#), r##"user "#" in"##);
    }
}
