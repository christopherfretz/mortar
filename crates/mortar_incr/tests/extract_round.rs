//! WP-261 (rulings "After WP-261's first hand-back" item 3, option (a)):
//! a stored archive member that is neither `LAZY` nor of a round reads as
//! IN-71b's `--whole-archive` member. When the update's line has the file
//! as a lazy member (its changed row is `LAZY` or of a round), the stored
//! row is an extracted lazy member stored without its round (a version-6
//! state's datum, or a capture's defect), and `extract` answers
//! `state-corrupt` before any removal cone is built from it.

#![allow(clippy::many_single_char_names, clippy::too_many_lines)]

#[allow(dead_code, clippy::all)]
mod model {
    include!("support/resolve_model.rs");

    /// `a.o` refers to `f`; the member `m.o` defines `f` (extracted, round
    /// 1). The update edits `m.o` (it also defines `g`). `zero` stores
    /// `m.o`'s row with round 0 (not `LAZY`).
    pub fn edit_member(zero: bool) -> Result<resolve::ResolveDelta, IncrReason> {
        let mut old = Link::new();
        old.add(R, false, vec![undef("f")]);
        let m = old.add(2 * R, true, vec![def("f")]);
        let mut new = old.clone();
        new.files[m as usize].as_mut().unwrap().ments = vec![def("f"), def("g")];
        let j = join_patched(&old, &new, |state, _| {
            let row = &mut state.files[m as usize];
            assert_eq!((row.member, row.flags & file_flags::LAZY, row.round), (1, 0, 1));
            if zero {
                row.round = 0;
            }
        });
        j.result
    }

    /// The same edit with `m.o` a `--whole-archive` member: its stored row
    /// (member, not `LAZY`, round 0) and its changed row (member, not
    /// `LAZY`, round 0) agree, and the update is not `state-corrupt`.
    pub fn edit_whole_archive_member() -> Result<resolve::ResolveDelta, IncrReason> {
        let mut old = Link::new();
        old.add(R, false, vec![undef("f")]);
        let m = old.add(2 * R, false, vec![def("f")]);
        let mut new = old.clone();
        new.files[m as usize].as_mut().unwrap().ments = vec![def("f"), def("g")];
        let mut keys = Vec::new();
        let mut state = old.state(&mut keys);
        state.files[m as usize].member = 1;
        let (files, appeared) = changes(&old, &new);
        let mut changed = assemble(&state, &new, &files, &appeared);
        for c in &mut changed.files {
            if c.file.0 == m {
                c.row.member = 1;
            }
        }
        let view = state.view();
        let options = new.options();
        let x = extract::extract(&view, &changed, &options)?;
        match resolve::resolve(&view, &changed, &x, &options)? {
            resolve::Resolution::Delta(d) => Ok(*d),
            resolve::Resolution::DrawIn(l) => Err(IncrReason::StateCorrupt(format!("{l:?}"))),
        }
    }
}

use mortar_core::incr::IncrReason;

// spec: incr.md IN-71b (WP-261 option (a)): the line says lazy, the stored
// row says `--whole-archive`: `state-corrupt`.
#[test]
fn extracted_member_stored_with_round_zero_is_state_corrupt() {
    let r = model::edit_member(true);
    let Err(IncrReason::StateCorrupt(why)) = &r else { panic!("{r:?}") };
    assert!(why.contains("round 0"), "{why}");
    // The control: the same update over the stored round 1 is not refused.
    let ok = model::edit_member(false);
    assert!(!matches!(ok, Err(IncrReason::StateCorrupt(_))), "{ok:?}");
    // A `--whole-archive` member (stored and changed rows agree) is not.
    let w = model::edit_whole_archive_member();
    assert!(!matches!(w, Err(IncrReason::StateCorrupt(_))), "{w:?}");
}
