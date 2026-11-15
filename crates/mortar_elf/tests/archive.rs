//! Tests of `mortar_elf::archive` against `spec/archive.md`: each section
//! of the spec on hand-built bytes (`spec`), agreement with `ar`, `nm` and
//! their LLVM counterparts on real archives (`tools`), and input that no
//! tool writes (`hostile`).

#[path = "archive/build.rs"]
mod build;
#[path = "archive/hostile.rs"]
mod hostile;
#[path = "archive/spec.rs"]
mod spec;
#[path = "archive/tools.rs"]
mod tools;
