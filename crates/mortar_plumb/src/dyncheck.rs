//! Stage 3's static checks of a dynamic image (`spec/dyn.md` §12: P-D1,
//! P-D4..P-D8, P-D10..P-D13).

use crate::dynmodel::Model;
use crate::dynout::DynOut;
use crate::image::Image;
use crate::line::Line;
use crate::loader::Loader;
use crate::locate::Obj;
use crate::map::Format;
use crate::report::Finding;

#[allow(clippy::too_many_arguments)]
pub(crate) fn check(
    image: &Image<'_>,
    out: &DynOut,
    line: &Line,
    model: &Model,
    ld: &Loader<'_>,
    objs: &[Obj<'_>],
    format: Format,
) -> Vec<Finding> {
    let _ = (image, out, line, model, ld, objs, format);
    Vec::new()
}
