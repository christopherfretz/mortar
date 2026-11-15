//! The build id's tree (I-72, I-73) through `mortar_digest` (WP-026h): a
//! leaf per window, hashed by the worker that filled it before the window
//! is written, set into one `LeafColumn`; the root on one thread.

use mortar_digest::{Leaf, LeafColumn, TreeError, TreeStyle};

/// `id = root[0..L]` of the leaves (any order), for a file of `file_size`
/// bytes.
pub fn build_id(style: TreeStyle, file_size: u64, leaves: &[(u64, Leaf)]) -> Result<Vec<u8>, TreeError> {
    let mut column = LeafColumn::new(style.tree_fn(), file_size)?;
    for (index, leaf) in leaves {
        column.set(*index, leaf)?;
    }
    let root = column.root()?;
    Ok(style.id(&root)?.to_vec())
}
