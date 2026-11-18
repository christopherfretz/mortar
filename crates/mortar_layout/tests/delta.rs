//! U-4's tests (`spec/incr.md` IN-72). WP-213 (U-1): the entry point's
//! "until it lands" value; U-4 replaces it.

use mortar_core::incr::{IncrReason, LayoutInput, MemoryLayout, StoredLayout, Unimplemented};
use mortar_elf::{Class, Machine};
use mortar_layout::delta::layout;
use mortar_layout::target::Target;

// spec: incr.md IN-72 (until U-4)
#[test]
fn layout_until_it_lands() {
    let rows = MemoryLayout::default();
    let stored = StoredLayout::new(&rows);
    let target = Target {
        machine: Machine::X86_64,
        class: Class::Elf64,
        max_page: 4096,
        image_base: 0x40_0000,
        islands: None,
    };
    assert_eq!(
        layout(&stored, &LayoutInput::default(), &target).err(),
        Some(IncrReason::UpdateNotImplemented(Some(Unimplemented::Layout)))
    );
}
