//! The generator of the synthetic workloads (`spec/harness.md` "bench",
//! "Synthetic workloads"): C and C++ sources that take one property of a
//! link to an extreme. A pure function of the shape and its two
//! parameters: the same request gives the same bytes, on any machine.

use std::fmt::Write as _;

/// Changes whenever the generator's output changes for any request: part
/// of the key a prepared workload is cached under.
pub const GENERATOR: u32 = 1;

/// The property a synthetic workload takes to an extreme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Many objects, each with many functions that call into other
    /// objects: symbols and relocations. Parameters: objects, functions
    /// per object.
    Objects,
    /// Few objects with very many sections (built with
    /// `-ffunction-sections -fdata-sections`). Parameters: objects,
    /// functions per object; a function has a section of code and one of
    /// data.
    Sections,
    /// C++ translation units that all define the same inline functions and
    /// variables: COMDAT groups of which the link keeps one each.
    /// Parameters: translation units, inline functions.
    Comdat,
    /// Objects whose data is string literals, half of them the same in
    /// every object: mergeable string sections. Parameters: objects,
    /// strings per object.
    Strings,
    /// A chain of archives: half the members of each are needed, each by a
    /// member of the archive before it. Parameters: archives, members per
    /// archive.
    Archives,
}

impl Shape {
    pub const ALL: [Self; 5] = [Self::Objects, Self::Sections, Self::Comdat, Self::Strings, Self::Archives];

    pub fn name(self) -> &'static str {
        match self {
            Self::Objects => "objects",
            Self::Sections => "sections",
            Self::Comdat => "comdat",
            Self::Strings => "strings",
            Self::Archives => "archives",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|shape| shape.name() == name)
    }

    /// The file extension of the sources.
    pub fn extension(self) -> &'static str {
        if self == Self::Comdat { "cc" } else { "c" }
    }
}

/// One generated source file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// The file's name, without a directory.
    pub name: String,
    pub text: String,
    /// Compiled to an object (a header is not).
    pub compiled: bool,
    /// The archive the object becomes a member of (`Shape::Archives`).
    pub archive: Option<usize>,
}

/// What a request generates: `count()` sources, each made on demand, so
/// that the largest sizes are never in memory at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub shape: Shape,
    /// The shape's two parameters, each at least 1.
    pub params: (usize, usize),
}

impl Request {
    /// The number of sources: the units, plus `main` and, for
    /// `Shape::Comdat`, the header.
    pub fn count(&self) -> usize {
        let (a, b) = self.params;
        match self.shape {
            Shape::Objects | Shape::Sections | Shape::Strings => a + 1,
            Shape::Comdat => a + 2,
            Shape::Archives => a * b + 1,
        }
    }

    /// The source at `index` (below [`Request::count`]). The last is
    /// `main`.
    pub fn source(&self, index: usize) -> Source {
        let (a, b) = self.params;
        let ext = self.shape.extension();
        let unit = |name: String, text: String| Source { name, text, compiled: true, archive: None };
        if index + 1 == self.count() {
            return unit(format!("main.{ext}"), self.main());
        }
        match self.shape {
            Shape::Objects => unit(format!("o{index:05}.c"), objects_unit(index, a, b)),
            Shape::Sections => unit(format!("s{index:05}.c"), sections_unit(index, b)),
            Shape::Strings => unit(format!("t{index:05}.c"), strings_unit(index, b)),
            Shape::Comdat if index == a => {
                Source { name: "common.h".into(), text: comdat_header(b), compiled: false, archive: None }
            }
            Shape::Comdat => unit(format!("u{index:05}.cc"), comdat_unit(index)),
            Shape::Archives => {
                let (archive, member) = (index / b, index % b);
                Source {
                    name: format!("a{archive:05}_m{member:05}.c"),
                    text: archive_member(archive, member, a, b),
                    compiled: true,
                    archive: Some(archive),
                }
            }
        }
    }

    /// `main`: calls into every unit, so that a link has nothing to drop.
    fn main(&self) -> String {
        let (a, b) = self.params;
        let mut text = String::new();
        let (decl, call): (fn(usize) -> String, fn(usize) -> String) = match self.shape {
            Shape::Objects => (
                |i| format!("int {}(int);\n", objects_name(i, 0)),
                |i| format!("  sum += {}(sum);\n", objects_name(i, 0)),
            ),
            Shape::Sections => (
                |i| format!("int mortar_bench_sections_s{i:05}_entry(int);\n"),
                |i| format!("  sum += mortar_bench_sections_s{i:05}_entry(sum);\n"),
            ),
            Shape::Strings => (
                |i| format!("extern const char *const mortar_bench_strings_t{i:05}[];\n"),
                |i| format!("  sum += mortar_bench_strings_t{i:05}[0][0];\n"),
            ),
            Shape::Comdat => (
                |i| format!("int mortar_bench_comdat_unit_{i:05}(int);\n"),
                |i| format!("  sum += mortar_bench_comdat_unit_{i:05}(sum);\n"),
            ),
            Shape::Archives => (
                |m| format!("int {}(int);\n", archive_name(0, m, 0)),
                |m| format!("  sum += {}(sum);\n", archive_name(0, m, 0)),
            ),
        };
        // The archives' chain starts at the needed members of the first.
        let units = if self.shape == Shape::Archives { needed_members(b) } else { a };
        for i in 0..units {
            text.push_str(&decl(i));
        }
        text.push_str("int main(void) {\n  int sum = 1;\n");
        for i in 0..units {
            text.push_str(&call(i));
        }
        text.push_str("  return sum == 0;\n}\n");
        text
    }
}

/// A small deterministic mix of two numbers (SplitMix64's finalizer): what
/// picks callees and string contents. Not a random number generator: the
/// same arguments give the same result, always.
fn mix(a: usize, b: usize) -> usize {
    let mut z = (a as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ (b as u64).wrapping_add(0x632b_e59b_d9b4_e019);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    ((z ^ (z >> 31)) >> 16) as usize
}

fn objects_name(object: usize, function: usize) -> String {
    format!("mortar_bench_objects_o{object:05}_function_{function:05}")
}

/// Object `index` of `objects`: `functions` functions, each calling one
/// function of the next object and one of an object further on (none
/// calls backwards, so the program ends), and reading the object's table.
fn objects_unit(index: usize, objects: usize, functions: usize) -> String {
    let mut text = String::new();
    let callees = |f: usize| {
        let near = (index + 1 < objects).then(|| (index + 1, mix(index, f) % functions));
        let ahead = objects - index - 1;
        let far = (ahead > 1).then(|| (index + 2 + mix(f, index) % (ahead - 1), mix(f, f + index) % functions));
        [near, far]
    };
    let mut declared = std::collections::BTreeSet::new();
    for f in 0..functions {
        for (object, function) in callees(f).into_iter().flatten() {
            if declared.insert((object, function)) {
                let _ = writeln!(text, "int {}(int);", objects_name(object, function));
            }
        }
    }
    let _ = writeln!(text, "int mortar_bench_objects_o{index:05}_table[16] = {{{index}, 1, 2, 3}};");
    for f in 0..functions {
        let _ = write!(text, "int {}(int x) {{\n  int r = x + {f}", objects_name(index, f));
        for (object, function) in callees(f).into_iter().flatten() {
            let _ = write!(text, " + {}(x)", objects_name(object, function));
        }
        let _ = writeln!(text, ";\n  return r + mortar_bench_objects_o{index:05}_table[{}];\n}}", f % 16);
    }
    text
}

/// Object `index`: `functions` functions, each with a variable of its
/// own, each calling the one before it; the entry runs the last.
fn sections_unit(index: usize, functions: usize) -> String {
    let name = |f: usize| format!("mortar_bench_sections_s{index:05}_f{f:06}");
    let mut text = String::new();
    for f in 0..functions {
        let _ = writeln!(text, "int {}_v = {f};", name(f));
        let _ = write!(text, "int {}(int x) {{ return x + {}_v", name(f), name(f));
        if f > 0 && f % 64 != 0 {
            // Chains of 64, so that no call nests deeper than that.
            let _ = write!(text, " + {}(x)", name(f - 1));
        }
        text.push_str("; }\n");
    }
    let _ = writeln!(text, "int mortar_bench_sections_s{index:05}_entry(int x) {{\n  int r = x;");
    for f in (0..functions).filter(|f| (f + 1) % 64 == 0 || f + 1 == functions) {
        let _ = writeln!(text, "  r += {}(x);", name(f));
    }
    text.push_str("  return r;\n}\n");
    text
}

/// The words the strings are made of.
const WORDS: [&str; 16] = [
    "section", "symbol", "relocation", "archive", "segment", "layout", "merge", "string", "table", "offset",
    "address", "object", "linker", "output", "input", "header",
];

/// Object `index`: a table of `strings` literals. Those at even positions
/// are the same in every object; the others have the object's number in
/// them.
fn strings_unit(index: usize, strings: usize) -> String {
    let mut text = format!("const char *const mortar_bench_strings_t{index:05}[] = {{\n");
    for s in 0..strings {
        let owner = if s % 2 == 0 { "every object".to_owned() } else { format!("object {index}") };
        let _ = write!(text, "  \"string {s} of {owner}:");
        for w in 0..3 + mix(s, 7) % 6 {
            let _ = write!(text, " {}", WORDS[mix(s, w) % WORDS.len()]);
        }
        text.push_str("\",\n");
    }
    text.push_str("  0\n};\n");
    text
}

/// The header every C++ unit includes: `inlines` inline functions, an
/// inline variable for every eighth, a class template with a member
/// function, and one inline function that uses them all.
fn comdat_header(inlines: usize) -> String {
    let mut text = String::from(
        "#pragma once\n\
         template <class T> struct mortar_bench_comdat_box {\n  T value;\n  \
         T get(T x) const { return value + x; }\n};\n",
    );
    for i in 0..inlines {
        if i % 8 == 0 {
            let _ = writeln!(text, "inline int mortar_bench_comdat_variable_{i:05} = {i};");
        }
        let _ = writeln!(
            text,
            "inline int mortar_bench_comdat_inline_{i:05}(int x) {{\n  \
             mortar_bench_comdat_box<int> box{{{i}}};\n  \
             return box.get(x) + mortar_bench_comdat_variable_{:05};\n}}",
            i - i % 8
        );
    }
    text.push_str("inline int mortar_bench_comdat_all(int x) {\n  int r = x;\n");
    for i in 0..inlines {
        let _ = writeln!(text, "  r += mortar_bench_comdat_inline_{i:05}(x);");
    }
    text.push_str("  return r;\n}\n");
    text
}

/// C++ unit `index`: one function of its own that uses every inline
/// function of the header, and so makes the unit define them all.
fn comdat_unit(index: usize) -> String {
    format!(
        "#include \"common.h\"\nint mortar_bench_comdat_unit_{index:05}(int x) {{\n  \
         return mortar_bench_comdat_all(x) + {index};\n}}\n"
    )
}

/// How many members of an archive of `members` the chain needs: half,
/// rounded up.
fn needed_members(members: usize) -> usize {
    members.div_ceil(2)
}

fn archive_name(archive: usize, member: usize, function: usize) -> String {
    format!("mortar_bench_archives_a{archive:05}_m{member:05}_f{function}")
}

/// Member `member` of archive `archive`: eight functions. If the chain
/// needs the member, its first function calls the same member of the next
/// archive; the members of the second half are needed by nothing.
fn archive_member(archive: usize, member: usize, archives: usize, members: usize) -> String {
    let mut text = String::new();
    let next = (member < needed_members(members) && archive + 1 < archives)
        .then(|| archive_name(archive + 1, member, 0));
    if let Some(next) = &next {
        let _ = writeln!(text, "int {next}(int);");
    }
    for f in 0..8 {
        let _ = write!(text, "int {}(int x) {{ return x + {f}", archive_name(archive, member, f));
        if let Some(next) = next.as_ref().filter(|_| f == 0) {
            let _ = write!(text, " + {next}(x)");
        }
        text.push_str("; }\n");
    }
    text
}

// spec/harness.md "bench", "Synthetic workloads".
#[cfg(test)]
mod tests {
    use super::super::record::Fnv;
    use super::*;

    fn all(request: &Request) -> Vec<Source> {
        (0..request.count()).map(|index| request.source(index)).collect()
    }

    fn hash(request: &Request) -> String {
        let mut hash = Fnv::default();
        for source in all(request) {
            hash.field(source.name.as_bytes());
            hash.field(source.text.as_bytes());
        }
        hash.text()
    }

    // "Synthetic workloads": the same parameters give identical sources;
    // other parameters give other sources.
    #[test]
    fn the_generator_is_deterministic() {
        for shape in Shape::ALL {
            let request = Request { shape, params: (7, 12) };
            assert_eq!(all(&request), all(&request), "{shape:?}");
            // In any order of asking.
            let backwards: Vec<_> = (0..request.count()).rev().map(|i| request.source(i)).collect();
            assert!(backwards.iter().rev().eq(all(&request).iter()), "{shape:?}");
            assert_ne!(hash(&request), hash(&Request { shape, params: (7, 13) }), "{shape:?}");
            assert_ne!(hash(&request), hash(&Request { shape, params: (8, 12) }), "{shape:?}");
            assert_eq!(Shape::parse(shape.name()), Some(shape));
        }
        assert_eq!(Shape::parse("everything"), None);
    }

    // "Synthetic workloads": the output is pinned. A change of the
    // generator changes every synthetic workload, so it is deliberate: it
    // comes with a new `GENERATOR` number and new hashes here.
    #[test]
    fn the_generators_output_is_pinned() {
        assert_eq!(GENERATOR, 1);
        let hashes: Vec<String> =
            Shape::ALL.into_iter().map(|shape| hash(&Request { shape, params: (5, 9) })).collect();
        assert_eq!(
            hashes,
            [
                "fnv1a64:0000000000000000",
                "fnv1a64:0000000000000000",
                "fnv1a64:0000000000000000",
                "fnv1a64:0000000000000000",
                "fnv1a64:0000000000000000",
            ]
        );
    }

    // "Synthetic workloads": what each shape is made of.
    #[test]
    fn the_shapes() {
        let names = |request: &Request| all(request).into_iter().map(|s| s.name).collect::<Vec<_>>();

        let objects = Request { shape: Shape::Objects, params: (3, 4) };
        assert_eq!(names(&objects), ["o00000.c", "o00001.c", "o00002.c", "main.c"]);
        let first = objects.source(0).text;
        assert_eq!(first.matches("(int x) {").count(), 4, "{first}");
        // Calls go forwards only; the last object calls nothing.
        assert!(first.contains("mortar_bench_objects_o00001_function_") && !first.contains("o00000_function_00000(x)"));
        let last = objects.source(2).text;
        assert!(!last.contains("(int);") && !last.contains("(x)"), "{last}");
        let main = objects.source(3).text;
        assert_eq!(main.matches("sum += mortar_bench_objects_o").count(), 3, "{main}");

        let sections = Request { shape: Shape::Sections, params: (2, 130) };
        assert_eq!(names(&sections), ["s00000.c", "s00001.c", "main.c"]);
        let unit = sections.source(1).text;
        assert_eq!(unit.matches("_v = ").count(), 130);
        assert_eq!(unit.matches("  r += ").count(), 3, "chains of 64: 0..63, 64..127, 128..129");

        let comdat = Request { shape: Shape::Comdat, params: (2, 9) };
        assert_eq!(names(&comdat), ["u00000.cc", "u00001.cc", "common.h", "main.cc"]);
        let header = comdat.source(2);
        assert!(!header.compiled);
        assert_eq!(header.text.matches("inline int mortar_bench_comdat_inline_").count(), 9);
        assert_eq!(header.text.matches("inline int mortar_bench_comdat_variable_").count(), 2);
        assert!(comdat.source(0).text.starts_with("#include \"common.h\"\n"));

        let strings = Request { shape: Shape::Strings, params: (2, 6) };
        let (first, second) = (strings.source(0).text, strings.source(1).text);
        let literals = |text: &str| text.lines().filter(|l| l.starts_with("  \"")).map(str::to_owned).collect::<Vec<_>>();
        let (a, b) = (literals(&first), literals(&second));
        assert_eq!((a.len(), b.len()), (6, 6));
        // Even positions are shared, odd ones are the object's own.
        assert!(a[0] == b[0] && a[2] == b[2] && a[1] != b[1] && a[5] != b[5], "{first}{second}");

        let archives = Request { shape: Shape::Archives, params: (3, 4) };
        assert_eq!(archives.count(), 13);
        let member = |archive, member| archives.source(archive * 4 + member);
        assert_eq!(member(1, 2).name, "a00001_m00002.c");
        assert_eq!(member(1, 2).archive, Some(1));
        // The needed half calls on; the other half and the last archive do not.
        assert!(member(0, 1).text.contains("+ mortar_bench_archives_a00001_m00001_f0(x)"));
        assert!(!member(0, 2).text.contains("(x)") && !member(2, 0).text.contains("(x)"));
        let main = archives.source(12);
        assert_eq!((main.name.as_str(), main.archive), ("main.c", None));
        assert_eq!(main.text.matches("sum += mortar_bench_archives_a00000_m").count(), 2);
    }
}
