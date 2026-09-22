//! Repeat collapsing: normalise the variable parts of a line so that
//! consecutive "same shape" lines can be folded into one row.
//!
//! Normalisation replaces:
//! * runs of hex digits that contain at least one decimal digit (numbers,
//!   hex ids, UUID segments, IP octets, timestamps) with `#`;
//! * double- or single-quoted strings with `"#"`;
//! * runs of whitespace with a single space.

#[inline]
fn is_hexish(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

/// Feed the normalised form of `line` into `sink` byte by byte.
fn normalize_into(line: &[u8], mut sink: impl FnMut(u8)) {
    let n = line.len();
    let mut i = 0;
    let mut last_space = true;
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
                sink(b'#');
                i = j;
                last_space = false;
                continue;
            }
            for &c in &line[i..j] {
                sink(c);
            }
            i = j;
            last_space = false;
            continue;
        }
        if b == b'"' || b == b'\'' {
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
                sink(q);
                sink(b'#');
                sink(q);
                i = j + 1;
                last_space = false;
                continue;
            }
            sink(b);
            i += 1;
            last_space = false;
            continue;
        }
        if b.is_ascii_whitespace() {
            if !last_space {
                sink(b' ');
            }
            last_space = true;
            i += 1;
            continue;
        }
        sink(b);
        last_space = false;
        i += 1;
    }
}

/// The normalised form of a line, as bytes.
pub fn normalize(line: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len());
    normalize_into(line, |b| out.push(b));
    while out.last() == Some(&b' ') {
        out.pop();
    }
    out
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
}
