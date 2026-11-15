//! Call frame information (`spec/plumb.md` §6.4): every live FDE of the
//! inputs is in the output once, with its CIE, personality and LSDA, and
//! `.eh_frame_hdr` indexes every one of them.

mod common;

use common::{Linker, Size, assert_clean, corpus};

// spec/plumb.md §6.4: the hand-written frames of every augmentation the
// assembler writes, by each linker, with a search table; the FDE of the
// collected function is dropped with it.
#[test]
fn s6_4_frames_of_each_augmentation_are_checked() {
    let Some(c) = corpus(Size::Tls) else { return };
    for linker in Linker::ALL {
        let link = c.get("eh-frames", linker);
        assert_eq!(link.run(), Some(0), "{}: the program fails", link.label());
        let r = link.verify();
        assert_clean(link, &r);
        let cov = &r.coverage;
        let s = r.summary();
        assert_eq!(cov.not_checked_total(), 0, "{}\n{s}", link.label());
        assert_eq!(cov.fdes_checked, 5, "{}\n{s}", link.label());
        assert_eq!(cov.fdes_dropped, 1, "{}\n{s}", link.label());
        assert_eq!(cov.eh_frame_hdr_entries, 5, "{}\n{s}", link.label());
        // Five initial locations, one LSDA, one personality.
        assert_eq!(cov.verified_of(mortar_plumb::Via::EhFrame), 7, "{}\n{s}", link.label());
    }
}
