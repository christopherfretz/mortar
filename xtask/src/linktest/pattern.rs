//! The patterns of a link test's `has`, `not`, `diag` and `no-diag`
//! expectations (`spec/harness.md` "Link tests", "Patterns").
//!
//! A pattern is matched against one line of text at a time. It is a
//! literal substring with three additions: a run of blanks matches any run
//! of blanks, `*` matches any text, and `^` first or `$` last ties the
//! match to that end of the line. Nothing else is special, so the names a
//! linker prints (`.text`, `foo(int)`, `[abc]`) are written as they are.

use std::fmt;

/// One element of a pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    /// This character.
    Char(char),
    /// One blank (a space or a tab).
    Blank,
    /// Any number of blanks, none included.
    Blanks,
    /// Any text, none included.
    Any,
}

/// A parsed pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    /// The elements, with an [`Token::Any`] at an end that is not tied to
    /// the line's: the whole line must match them.
    tokens: Vec<Token>,
    /// As written, for messages.
    source: String,
}

/// The characters a backslash may precede: those that are special
/// somewhere in a pattern.
const ESCAPABLE: [char; 4] = ['\\', '*', '^', '$'];

impl Pattern {
    /// Parses `source`, with the blanks at its ends dropped.
    ///
    /// # Errors
    /// An empty pattern, one that is only anchors, a backslash before a
    /// character that is not special, or a backslash at the end.
    pub fn parse(source: &str) -> Result<Self, String> {
        let source = source.trim_matches([' ', '\t']);
        if source.is_empty() {
            return Err("an empty pattern".to_owned());
        }
        let mut body = source;
        let at_start = body.starts_with('^');
        if at_start {
            body = &body[1..];
        }
        // `$` ends the pattern unless a backslash makes it a character:
        // an odd number of backslashes before it.
        let escaped = body
            .strip_suffix('$')
            .is_some_and(|before| before.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1);
        let at_end = body.ends_with('$') && !escaped;
        if at_end {
            body = &body[..body.len() - 1];
        }
        let mut tokens = Vec::new();
        if !at_start {
            tokens.push(Token::Any);
        }
        let mut chars = body.chars();
        let mut any_element = false;
        while let Some(c) = chars.next() {
            any_element = true;
            match c {
                '\\' => match chars.next() {
                    Some(next) if ESCAPABLE.contains(&next) => tokens.push(Token::Char(next)),
                    Some(next) => {
                        return Err(format!(
                            "'\\{next}' in a pattern: a backslash goes before '\\', '*', '^' or \
                             '$' only"
                        ));
                    }
                    None => return Err("a pattern ends with a backslash".to_owned()),
                },
                '*' => {
                    if tokens.last() != Some(&Token::Any) {
                        tokens.push(Token::Any);
                    }
                }
                ' ' | '\t' => {
                    if tokens.last() != Some(&Token::Blanks) {
                        tokens.extend([Token::Blank, Token::Blanks]);
                    }
                }
                other => tokens.push(Token::Char(other)),
            }
        }
        if !any_element {
            return Err(format!("the pattern '{source}' has nothing between its anchors"));
        }
        if !at_end && tokens.last() != Some(&Token::Any) {
            tokens.push(Token::Any);
        }
        Ok(Self { tokens, source: source.to_owned() })
    }

    /// Whether `line` (without its line terminator) matches.
    pub fn matches_line(&self, line: &str) -> bool {
        // The states of a position automaton: `at[i]` says that the
        // elements before `i` have matched the text read so far.
        let count = self.tokens.len();
        let mut at = vec![false; count + 1];
        at[0] = true;
        self.skip_optional(&mut at);
        let mut next = vec![false; count + 1];
        for c in line.chars() {
            next.fill(false);
            let blank = c == ' ' || c == '\t';
            for (i, token) in self.tokens.iter().enumerate() {
                if !at[i] {
                    continue;
                }
                match token {
                    Token::Char(want) => next[i + 1] |= *want == c,
                    Token::Blank => next[i + 1] |= blank,
                    Token::Blanks => next[i] |= blank,
                    Token::Any => next[i] = true,
                }
            }
            self.skip_optional(&mut next);
            std::mem::swap(&mut at, &mut next);
            if !at.contains(&true) {
                return false;
            }
        }
        at[count]
    }

    /// An element that may match nothing is also passed without text.
    fn skip_optional(&self, at: &mut [bool]) {
        for (i, token) in self.tokens.iter().enumerate() {
            if at[i] && matches!(token, Token::Blanks | Token::Any) {
                at[i + 1] = true;
            }
        }
    }

    /// Whether any line of `text` matches. A line ends at `\n`; a `\r`
    /// before it is not part of the line.
    pub fn matches_any_line(&self, text: &str) -> bool {
        text.lines().any(|line| self.matches_line(line.strip_suffix('\r').unwrap_or(line)))
    }
}

/// The pattern as written.
impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

/// `text` as a pattern that matches exactly it: what a substituted value
/// becomes inside a pattern.
pub fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if ESCAPABLE.contains(&c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::{Pattern, escape};

    fn matches(pattern: &str, line: &str) -> bool {
        Pattern::parse(pattern).unwrap().matches_line(line)
    }

    // spec/harness.md "Link tests", "Patterns": a pattern is a literal
    // substring of the line; no character of a symbol or a section name
    // is special.
    #[test]
    fn a_pattern_is_a_literal_substring() {
        assert!(matches("T main", "0000000000400120 T main"));
        assert!(matches("T main", "0000000000400120 T main_helper"));
        assert!(!matches("T main", "0000000000400120 t main"));
        assert!(matches(".init_array", "  [ 5] .init_array  INIT_ARRAY"));
        assert!(!matches(".init_array", "  [ 5] xinit_array"), "'.' is a dot");
        assert!(matches("foo(int)", "undefined reference to `foo(int)'"));
        assert!(matches("[abc]+?{1}|x", "see [abc]+?{1}|x here"));
        assert!(!matches("abc", "ab"));
        assert!(!matches("abc", ""));
        assert!(matches("é", "caf\u{e9}"));
    }

    // "Patterns": a run of blanks matches a run of blanks of any length,
    // and at least one.
    #[test]
    fn blanks_match_any_run_of_blanks() {
        let row = "  [ 2] .rodata           PROGBITS        00000000004001c8 0001c8 000006 01 AMS";
        assert!(matches(".rodata PROGBITS", row));
        assert!(matches(".rodata   PROGBITS", row));
        assert!(matches(".rodata\tPROGBITS", row));
        assert!(!matches(".rodata PROGBITS", ".rodataPROGBITS"), "at least one blank");
        assert!(matches("a b", "a \t b"));
        // The blanks at the ends of a pattern are not part of it.
        assert!(matches("  main  ", "T main"));
    }

    // "Patterns": `*` matches any text, none included.
    #[test]
    fn a_star_matches_any_text() {
        let row = "  [ 2] .rodata           PROGBITS        00000000004001c8 0001c8 000006 01 AMS";
        assert!(matches(".rodata PROGBITS * 000006 01 AMS", row));
        assert!(!matches(".rodata PROGBITS * 000007 01 AMS", row));
        assert!(matches("undefined*foo", "ld: undefined reference to `foo'"));
        assert!(matches("undefined*foo", "undefinedfoo"));
        assert!(!matches("undefined*foo", "foo is undefined"));
        assert!(matches("a**b", "a-b"));
        assert!(matches("*", ""));
        assert!(matches("a * b", "a  b"), "blank, nothing, blank");
        assert!(!matches("a * b", "a b"), "two blanks are asked for");
    }

    // "Patterns": `^` first ties the match to the start of the line, `$`
    // last to its end; elsewhere they are characters.
    #[test]
    fn anchors_tie_the_match_to_the_ends() {
        assert!(matches("T main$", "0000000000400120 T main"));
        assert!(!matches("T main$", "0000000000400120 T main_helper"));
        assert!(matches("^hello", "hello world"));
        assert!(!matches("^hello", "say hello"));
        assert!(matches("^hello$", "hello"));
        assert!(!matches("^hello$", "hello "));
        assert!(matches("^*T main$", "0000000000400120 T main"));
        assert!(matches("a^b$c", "xa^b$cx"), "not at an end: characters");
        assert!(matches("^ GNU", "  GNU_STACK"), "blanks after the start");
        assert!(!matches("^ GNU", "GNU_STACK"));
    }

    // "Patterns": a backslash makes `\`, `*`, `^` and `$` characters.
    #[test]
    fn a_backslash_makes_a_special_character_literal() {
        assert!(matches(r"\*ABS\*", "0000 *ABS* 0000"));
        assert!(!matches(r"\*ABS\*", "0000 ABS 0000"));
        assert!(matches(r"\^start", "a ^start"));
        assert!(matches(r"cost\$", "the cost$ is"));
        assert!(matches(r"a\\b", r"a\b"));
        assert!(matches(r"a\\$", r"xa\"), "an escaped backslash, then the anchor");
        assert!(!matches(r"a\\$", r"a\b"));
    }

    // "Patterns": what is not a pattern is an error that says why.
    #[test]
    fn pattern_errors() {
        let error = |pattern: &str| Pattern::parse(pattern).unwrap_err();
        assert!(error("").contains("empty pattern"));
        assert!(error("   ").contains("empty pattern"));
        assert!(error("^").contains("nothing between its anchors"));
        assert!(error("^$").contains("nothing between its anchors"));
        assert!(error(r"a\n").contains("a backslash goes before"));
        assert!(error(r"a\").contains("ends with a backslash"));
    }

    // "Patterns": every line is tried; a line ends at a newline, with or
    // without a carriage return.
    #[test]
    fn any_line_of_a_text() {
        let pattern = Pattern::parse("T main$").unwrap();
        assert!(pattern.matches_any_line("0 T _start\r\n1 T main\r\n"));
        assert!(pattern.matches_any_line("1 T main"));
        assert!(!pattern.matches_any_line("0 T _start\n1 T main2\n"));
        assert!(!pattern.matches_any_line(""));
        // A pattern never matches across lines.
        assert!(!Pattern::parse("a*b").unwrap().matches_any_line("a\nb\n"));
        assert_eq!(pattern.to_string(), "T main$");
    }

    // "Substitutions": a substituted value is taken literally in a
    // pattern.
    #[test]
    fn escaped_text_matches_itself_only() {
        for text in ["a*b", r"a\b", "^a$", "plain /lib/ld-linux.so.2"] {
            let pattern = Pattern::parse(&format!("^{}$", escape(text))).unwrap();
            assert!(pattern.matches_line(text), "{text}");
        }
        assert!(!Pattern::parse(&escape("a*b")).unwrap().matches_line("a-b"));
    }
}
