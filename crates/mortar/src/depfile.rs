//! `--dependency-file` (`spec/input.md` §10.4): a Make rule whose target
//! is the output and whose prerequisites are the distinct found paths of
//! the link, in load order, then one empty rule per path (so a deleted
//! input does not stop `make`). The layout is lld's; every path, the
//! target included, is escaped as GCC's `-MD` escapes (lld leaves the
//! target unescaped, GNU ld escapes nothing).

/// A path Make cannot read back: it holds a newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unwritable(pub Vec<u8>);

/// `path` as a Make word: a space or a tab after a backslash, `#` after a
/// backslash, `$` doubled; a run of backslashes before a space, a tab or
/// the word's end doubled, so that it stays a run of backslashes.
fn escape(out: &mut Vec<u8>, path: &[u8]) -> Result<(), Unwritable> {
    if path.contains(&b'\n') {
        return Err(Unwritable(path.to_vec()));
    }
    let mut run = 0;
    for &b in path {
        match b {
            b'\\' => {
                run += 1;
                out.push(b);
                continue;
            }
            b' ' | b'\t' => {
                out.extend(std::iter::repeat_n(b'\\', run + 1));
                out.push(b);
            }
            b'#' => out.extend_from_slice(b"\\#"),
            b'$' => out.extend_from_slice(b"$$"),
            _ => out.push(b),
        }
        run = 0;
    }
    out.extend(std::iter::repeat_n(b'\\', run));
    Ok(())
}

/// The dependency file of a link writing `target` from `paths`.
///
/// # Errors
///
/// The first path (or the target) that holds a newline.
pub fn render(target: &[u8], paths: &[&[u8]]) -> Result<Vec<u8>, Unwritable> {
    let mut out = Vec::new();
    escape(&mut out, target)?;
    out.push(b':');
    for p in paths {
        out.extend_from_slice(b" \\\n ");
        escape(&mut out, p)?;
    }
    out.push(b'\n');
    for p in paths {
        out.push(b'\n');
        escape(&mut out, p)?;
        out.extend_from_slice(b":\n");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: input.md §10.4 (lld's layout, measured: `o: \`, one path per
    // line after a space, then an empty rule per path)
    #[test]
    fn the_layout_is_llds() {
        let got = render(b"prog", &[b"a.o", b"s.ld", b"libp.a"]).unwrap();
        assert_eq!(
            String::from_utf8(got).unwrap(),
            "prog: \\\n a.o \\\n s.ld \\\n libp.a\n\na.o:\n\ns.ld:\n\nlibp.a:\n"
        );
        assert_eq!(render(b"prog", &[]).unwrap(), b"prog:\n");
    }

    // spec: input.md §10.4 (GCC's -MD escapes, the target included; lld
    // writes `a\ b$$\#%.o` for `a b$#%.o`)
    #[test]
    fn paths_are_escaped_for_make() {
        let got = render(b"o ut$.x", &[b"a b$#%.o", b"x\\ y", b"t\tab", b"end\\", b"a\\b"]);
        assert_eq!(
            String::from_utf8(got.unwrap()).unwrap(),
            "o\\ ut$$.x: \\\n a\\ b$$\\#%.o \\\n x\\\\\\ y \\\n t\\\tab \\\n end\\\\ \\\n a\\b\n\
             \na\\ b$$\\#%.o:\n\nx\\\\\\ y:\n\nt\\\tab:\n\nend\\\\:\n\na\\b:\n"
        );
        assert_eq!(render(b"o", &[b"a\nb"]), Err(Unwritable(b"a\nb".to_vec())));
    }
}
