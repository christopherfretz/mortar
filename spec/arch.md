# spec: the `Arch` trait (`mortar_arch`)

The contract between the architecture-independent linker and one
architecture. `mortar_arch` defines the trait, the vocabulary it speaks
and the generic parts every architecture shares (target kinds, demand
classes, the fixed point of range extension, the overflow rule);
`mortar_a64` (`spec/a64.md`), `mortar_x64` (`spec/x64.md`, WP-029) and
`mortar_x86` (`spec/x86.md`, WP-030) implement it.

The trait is written against `spec/model.md` draft 2. It adds no table:
everything an architecture decides is either a pure function of a
relocation record and the bytes around it, a demand flag in `Demands`
(model §3.7), a slot in `Slots` (§4.3), a patch in `Placement` (§4.4), or
a value computed in `fill` (§5). Requested changes to the model are
listed in §9, not made.

Appendices A and B walk x86-64 and i386 through the trait; Appendix C
lists what they forced. A statement this document could not ground is
marked **[open]** and collected in §11.

## 1. Principles

1. **Classification is input-only.** What a relocation type means
   (formula, field, check, demand class) depends on the type and on the
   bytes of the input section around the place, nothing else. It
   satisfies the model's I-file invariant and is recomputed whenever
   needed rather than stored (model §3.2: relocations are not copied).
2. **Scan is a union.** Scan reads `Inputs`, `Resolution`, `Liveness`
   and the options, and raises demand flags whose merge is commutative and
   idempotent (model §6). It never reads addresses, and never reads
   `Demands` (it writes them).
3. **Fill is a function.** Applying a relocation, including any
   relaxation, reads the tables of model §3 and §4, the input bytes and
   the relocation records of the same section, and writes only the bytes
   of its own window (§3.6). Its result does not depend on the order in
   which relocations or units are filled.
4. **Demand is conservative** (model §3.7), with the proof obligation of
   §5.2: whatever form fill chooses, the slots it needs exist.
5. **One trait for three architectures**, generic over ELF class (word
   size 8 or 4) and over RELA or REL with implicit addends. An
   architecture is a value (`&'static dyn Arch`) chosen from
   `Inputs.target`; nothing generic matches on the machine.
6. **Correct or refused by name.** A type, field form or sequence the
   architecture does not implement is refused at scan with class
   `relocation-type` or `feature` (diag.md §4), never applied
   approximately.
7. **Two implementations.** Every function here is specified so that
   `mortar_plumb` can recompute it independently from the architecture
   spec (`docs/AGENTS.md` rule 10). The trait's code is not shared with
   the verifier; the architecture spec is.

## 2. Vocabulary

### 2.1 Output and target kinds

```rust
pub enum OutputKind { Static, Exec, Pie, Shared, Relocatable }
pub enum TargetKind {
    Local,                 // defined in this output, not preemptible, not IFUNC, not SHN_ABS
    Abs,                   // defined, SHN_ABS: does not move with the load address
    Preemptible,           // defined in this output (Shared), may be preempted at load time
    Shlib { func: bool },  // defined only in a shared object (Exec, Pie)
    Ifunc,                 // non-preemptible STT_GNU_IFUNC defined in this output
    Weak0,                 // undefined weak, not dynamic: value 0
    WeakDyn,               // undefined weak, in .dynsym and preemptible
    Undefined,             // undefined, not weak, defined nowhere and not made dynamic (§3.4)
}
```

- `Static`: ET_EXEC without `.dynamic`; `Exec`: ET_EXEC with `.dynamic`;
  `Pie`: ET_DYN with `DF_1_PIE` (a static PIE included); `Shared`: ET_DYN
  shared object. `Relocatable` (`-r`) applies no relocation (§3.3); it is
  listed so that addend write-back has a kind.
- `target_kind(SymKey) -> TargetKind` is a generic function of
  `Resolution` (`def`, the merged visibility, `-Bsymbolic*`, export
  rules) and the output kind. A preemptible IFUNC is `Preemptible`.
  `target_kind` is a property of the **key**, never of the relocation
  class. A weak undefined key is `WeakDyn` or `Weak0` by `spec/dyn.md`
  D-25, per key: a pre-pass after liveness (`mortar_scan`, WP-040
  step 0k) classes each live relocation naming the key with
  `weak_use` (§3.12; each architecture spec carries its column of
  D-25's class table) and decides the key from the union of its classes.
  In `Static` and a static PIE (`dyn.md` D-1 `StaticPie`, no `.dynsym`
  beyond the null entry) every weak undefined key is `Weak0`. This
  ends the former circle with `spec/resolve.md` §5.4 (a64.md Q4).
- A `Local(FileId, u32)` key is `Local`, or `Abs` for an `SHN_ABS` local,
  or the kind of the section symbol's section (always `Local`).

### 2.2 Values

A relocation's value is an expression over these terms (each
architecture spec defines them against model.md's tables; a64.md §3.1 is
the reference):

```rust
pub enum Base {
    Zero,
    S,                 // the symbol's address (its PLT/IPLT entry when canonical)
    L,                 // the PLT (or IPLT) entry when the class demanded one, else S
    G(SlotKind),       // the address of the symbol's GOT slot of that kind
    GotBase,           // the arch's GOT base (§3.1 `got_base`)
    TpRel,             // TP offset of S + A (§3.8; its sign is the variant's)
    DtpRel,            // S + A − tls.start
    Size,              // st_size of the resolved definition (x86 SIZE32/SIZE64)
}
pub enum SlotKind { Addr, TpOff { negated: bool }, TlsGd, TlsLd, TlsDesc, PltGot }
pub enum Minus { None, P, PageP, GotBase, PageGotBase }
pub struct Value { pub base: Base, pub add_a: bool, pub page: bool, pub negate: bool, pub minus: Minus }
```

`X = ±((page ? Page(B + a) : B + a) − M)`, with `a = A` when `add_a`
(else 0), `Page` the architecture's page function, and `negate` applying
the sign last. `TpRel` and `DtpRel` take `S + A` themselves (`add_a` is
then false). `SlotKind::PltGot` is the `.got.plt` word of the symbol's PLT
entry (x86-64 `GOTPLT64`). `TpOff { negated: true }` is a slot holding the
negated offset (i386 `TLS_IE_32`).

### 2.3 Fields, checks, addends

```rust
pub enum Field {
    None,                                  // a marker: writes nothing
    Data { bytes: u8 },                    // 1, 2, 4, 8, little-endian at P
    Insn { form: u16 },                    // an architecture's instruction form (a64.md §3.2)
}
pub enum Check { None, Signed(u8), Unsigned(u8), Either(u8), MovSigned(u8) }
pub struct Howto {
    pub name: &'static str,                // the ABI name, for diagnostics and refusals
    pub value: Value,
    pub field: Field,
    pub check: Check,
    pub align: u8,                         // a(k): X mod k = 0, 1 when none
    pub class: DemandClass,                // §4.3
    pub addend: AddendRule,                // Any, or MustBeZero (GOT forms that ignore A)
    pub implicit: Option<Implicit>,        // REL: where the addend is read (§3.3)
    pub window: Window,                    // bytes a rewrite may touch around the field (§3.6)
    pub status: Status,                    // M1, M2, Later, Refused (§3.2)
}
pub struct Window { pub before: u8, pub after: u8 }
pub struct Implicit { pub bytes: u8, pub signed: bool, pub shift: u8 }
```

Checks are a64.md §3.3's (`Signed(n)` is `s(n)`, `Either(n)` is
`su(n)`, `MovSigned(n)` is `ms(n)`).

**Values and checks by ELF class.**

- **ELFCLASS64.** `X` is computed in 64-bit two's complement, and the
  check applies to that value.
- **ELFCLASS32** (i386). Addresses are 32-bit, and the processor's
  arithmetic on them wraps modulo 2^32. `X` is computed modulo 2^32 and
  then read as a 32-bit value, signed for `Signed` checks and unsigned
  for `Unsigned` checks. `Either(32)` and `Data{4}` with `Check::None`
  accept every 32-bit value. Checks on narrower fields (16, 8 bits)
  apply to that 32-bit-reduced value.
- Two valid i386 links the 64-bit reading would refuse, and which GNU ld
  and lld accept:
  - `call tgt` with `tgt = 0xF0000000` at `P = 0x1000`. The
    mathematical `X = 0xEFFFEFFB` is above 2^31, but modulo 2^32 it is
    `−0x10001005`, which satisfies `Signed(32)`. At run time the branch
    wraps to `tgt`.
  - `.long sym − 16` with `sym < 16`. The value is negative and stored
    as its 32-bit two's complement, which `Either(32)` accepts.

## 3. The trait

```rust
pub trait Arch: Sync + 'static {
    fn facts(&self) -> &'static Facts;                                       // §3.1

    fn classify(&self, r_type: u32, site: Site<'_>) -> Result<Howto, Refusal>; // §3.2
    fn read_addend(&self, h: &Howto, field: &[u8]) -> i64;                   // §3.3, REL only
    fn write_addend(&self, h: &Howto, field: &mut [u8], addend: i64)
        -> Result<(), FieldError>;                                           // §3.3, REL only

    fn scan(&self, cx: &ScanCx<'_>, at: &RelocAt<'_>, out: &mut DemandSink<'_>); // §3.4
    fn apply(&self, cx: &FillCx<'_>, at: &RelocAt<'_>, out: &mut SectionOut<'_>)
        -> Result<(), FieldError>;                                           // §3.6
    fn unit_window(&self, output: OutputKind, at: &RelocAt<'_>, names: &dyn SymNames)
        -> Option<UnitWindow>;                          // §3.4 Overlap, §3.5; default: field window
    fn consumed(&self, output: OutputKind, at: &RelocAt<'_>, names: &dyn SymNames)
        -> bool;                                        // §3.4, §6 step 1; default: false
    fn sequence_extent(&self, records: &dyn Records, i: u32) -> u32; // §3.5; default: 1

    fn got(&self) -> &'static GotShape;                                      // §3.7
    fn plt(&self, v: PltVariant) -> &'static PltShape;
    fn plt_variant(&self, cx: &OutputCx<'_>) -> Result<PltVariant, Refusal>;
    fn write_plt(&self, v: PltVariant, what: PltPart, cx: &SynthCx<'_>, out: &mut [u8])
        -> Result<(), FieldError>;
    fn branch(&self, h: &Howto) -> Option<Branch>;                           // §3.7, §6
    fn thunk(&self) -> Option<&'static ThunkShape>;
    fn write_thunk(&self, cx: &PatchCx<'_>, out: &mut [u8]) -> Result<(), FieldError>;
    fn thunk_target_ok(&self, cx: &PatchCx<'_>) -> Result<(), Refusal>;
    fn errata(&self) -> &'static [Erratum];                                  // §3.7
    fn erratum_sites(&self, e: Erratum, code: &Code<'_>, out: &mut Vec<ErratumSite>)
        -> Result<(), Unproved>;
    fn erratum_in_place(&self, e: Erratum, cx: &PatchCx<'_>) -> bool;
    fn patch(&self, e: Erratum) -> Option<&'static ThunkShape>;              // §3.7: an erratum patch's size, alignment
    fn patch_reach(&self, e: Erratum) -> Reach;                              // §3.7, §6 step 1
    fn write_patch(&self, e: Erratum, cx: &PatchCx<'_>, out: &mut [u8]) -> Result<(), FieldError>;

    fn tls(&self) -> &'static TlsAbi;                                        // §3.8
    fn eh(&self) -> &'static EhEncodings;                                    // §3.9
    fn dyn_type(&self, role: DynRole) -> Option<u32>;                        // §3.10
    fn tombstone(&self, section_name: &[u8]) -> u64;                         // §3.11; default: tombstone(name, Facts.class)
    fn weak0(&self, h: &Howto) -> Weak0Rule;                                 // §3.12
    fn weak_use(&self, r_type: u32, place: &[u8], writable: bool)
        -> Option<WeakClass>;                                                // §3.12 [M2]
    fn lazy_target(&self, n: PltIndex) -> LazyTarget;                        // §3.7 [M2]
    fn dyn_tags(&self, cx: &OutputCx<'_>) -> &'static [DynTag];              // §3.10 [M2]
    fn property_merge(&self, pr_type: u32) -> Option<PropMerge>;             // §3.13
}
```

`fn e_flags` (§3.1) is also a method. All methods are pure: no interior
mutability, no I/O, no allocation except into the `out` vectors given. `Site`, `RelocAt` and the contexts
are read-only views; their contents are listed where they are used.

### 3.1 Facts

```rust
pub struct Facts {
    pub machine: Machine, pub class: Class,  // mortar_elf's
    pub e_machine: u16,
    pub rel_form: RelForm,                   // the one relocation section type accepted: Rela or Rel
    pub word: u8,                            // 8 or 4
    pub osabi: u8,                           // EI_OSABI written
    pub max_page: u64, pub common_page: u64, // defaults of -z max-page-size / common-page-size
    pub image_base: u64,                     // non-PIE default; PIE and shared are 0
    pub page_mask: u64,                      // Page(x) = x & !page_mask (AArch64 0xFFF); 0 if unused
    pub got_base: GotBase,                   // GotStart (AArch64) or GotPltStart (x86-64, i386)
    pub got_header: u8,                      // words reserved at the start of .got with .dynamic
    pub got_plt_reserved: u8,                // words reserved at the start of .got.plt: with .dynamic
                                             // on every target; also in static outputs on x86 (synth.md S-67)
    pub max_reach: Window,                   // (before, after): derived, see below
    pub code_fill: &'static [u8],            // the trap pattern (layout.md L-82): AArch64 4 zero bytes, x86 0xCC
    pub code_nop: &'static [u8],             // the no-op of .init/.fini padding (L-82): AArch64 NOP, x86 0x90
    pub seq_leads: &'static [u32],           // the lead types RelocAt.leads gates (§3.5): AArch64 [513]; x86-64 [19, 20]; i386 [18, 19]
}
fn e_flags(&self, inputs: &mut dyn Iterator<Item = u32>) -> Result<u32, Refusal>; // 0 for all three
```

`max_reach: Window` is the largest `before` and `after` of any write
window of the architecture, over every `Howto` window (§3.2) and every
member window of a recognized form (`unit_window`, §3.5). `mortar_arch`
computes it from those tables; it is never written by hand
(`spec/image.md` I-111). AArch64 gives `(0, 4)` (the markers' word and
the GD→LE `CALL26` member, `spec/a64.md` §8.4); x86-64 `(4, 15)`
(`spec/x64.md` §8.6); i386 `(3, 6)` (`before` 3 from the SIB `TLS_GD`
form, `after` 6 from the 12-byte base-register GD and LD forms,
`spec/x86.md` X86-40). `code_fill` is positioned by address modulo its length.

The output's `e_flags` is a function of the live inputs' `e_flags`; for
the three targets it is 0. A relocation section of the other form
(`SHT_REL` where `Rela` is accepted, and the reverse) is refused by name
at resolve (`SectionRefusal::RelForm`, class `section-type`, name
`SHT_REL` or `SHT_RELA`): an `SHT_REL` section in an ELFCLASS64 input
(AArch64, x86-64) and an `SHT_RELA` section in an `EM_386` input. Before
WP-030c a 64-bit `SHT_REL` section was accepted with A = 0, which wrote
wrong bytes.

### 3.2 `classify`

```rust
pub struct Site<'a> { pub bytes: &'a [u8], pub offset: u64 } // the input section and r_offset
```

- **Contract**: a total function of `(r_type, the bytes of the
  section)`. It reads at most `max_window(r_type)` bytes around the
  field, a per-type constant, and none at all for most types.
- **The window depends on the matched form**, not only on the type.
  - i386 `TLS_GD` has a 7-byte SIB `lea` and a 6-byte
    `leal x@tlsgd(%reg)`.
  - Its call is 5 bytes with `PLT32` and 6 bytes with `GOT32X`.
  - x86-64 `TLSLD` is followed by a 5-byte `call` or a 6-byte
    `call *…(%rip)`, or (large model) by `movabs`, `add` and
    `call *%rax` (A.4).

  `classify` returns the window of the unrelaxed form, which is the
  field alone for every type here except the single-relocation x86
  rewrites of Appendix A.2/A.5. A sequence (§3.5) whose pattern matches
  has a window of its own, the union of the bytes it rewrites. The
  architecture spec states that window per matched form.
- Returns the `Howto` of §2.3. `status` is the type's milestone;
  `classify` returns `Refusal{RelocationType, name}` for `Refused` types
  and for `Later` and future-milestone types the build does not enable
  (the gate of `docs/ROADMAP.md`), so that refusal happens at scan with a
  place.
- An unknown number is `Refusal{RelocationType, "0x…"}` (diag.md §4.2).
- The formula may depend on the instruction (i386 `GOT32`: `G + A −
  GOT` with a base register, `G + A` without, decided by the ModRM byte
  inside the window). This is why `classify` sees the bytes (Appendix C,
  1).
- A field that lies outside the section, or whose window does, is
  `Malformed` (diag.md `malformed-input`) at scan.

### 3.3 Addends

- **RELA**: `A = r_addend`. `read_addend`/`write_addend` are not called.
- **REL**: `A = read_addend(h, field)`: the `Implicit` description of the
  type gives the width (`bytes`), signedness and scale (`shift`) of the
  value stored in the field of the input section; instruction forms read
  the immediate as the architecture spec says. `Implicit` addends of every
  width are read sign-extended. Where a reference linker reads a narrow
  field unsigned, the field's bits are unchanged and only the check
  differs (`spec/x86.md` X86-6). Read once per use from the input bytes;
  never stored.
- **REL addends without resident bodies** (`spec/x86.md` R-13). `A` is
  read from the input bytes of its field where a rule needs it, never
  from bytes a relocation has written, and no second copy of a section
  body or of a clip is made for it. `RelocAt.addend` is `Some(r_addend)`
  on RELA and `None` on REL; the architecture calls `read_addend` on the
  bytes `site` gives (scan) or `SectionOut::input` gives (fill, §3.6).
  The **read window** of a unit (a record, or a matched sequence) is
  every byte its `classify`, `scan` and `apply` read; it contains its
  write window, and the architecture spec states it per type and form.
  Within a matched sequence the architecture reads every member's addend
  and pattern bytes before it writes any byte of the unit (i386: the D1
  partner's `A = −4` lies inside the lead's window, which the lead
  overwrites). `Records` gives a REL record's `addend` as whatever its
  caller read, or 0; a REL architecture does not use it. What scan reads
  on REL, and what the image does at fill, are `spec/image.md`'s
  (`spec/x86.md` R-13 (a), (d), (e)).
- **Write-back**, for REL outputs only:
  - `-r` (M4): the field of the output holds the output relocation's
    addend: `A + delta`, where `delta` is what `spec/image.md` adds for a
    relocation against a section symbol whose section moved within its
    output section. `write_addend` checks it fits `Implicit` and returns
    `FieldError::Range` otherwise (the `relocation-out-of-range`
    diagnostic, §7).
  - `--emit-relocs`: the field holds the final value, as in any
    executable; the emitted relocation's addend is not recoverable from
    the output for REL, which is what GNU ld writes **[open: Q3]**.
- Section-symbol relocations into split sections (model §3.5) resolve
  their piece by the addend, read as above.

### 3.4 `scan`

```rust
pub struct RelocAt<'a> {
    pub file: FileId, pub section: SectionId, pub index: u32, // record index in its relocation section
    pub r_offset: u64, pub key: SymKey, pub r_type: u32,
    pub addend: Option<i64>,               // Some(r_addend) on RELA, None on REL (§3.3)
    pub howto: &'a Howto,
    pub site: Site<'a>,
    pub records: &'a dyn Records,          // the same relocation section, §3.5
    pub proof: &'a [ProofRun],             // the instruction proof of the section, §3.6 rule 1
    pub leads: bool,                       // the section may hold a record of a type in Facts.seq_leads, §3.5
}
impl RelocAt<'_> { pub fn is_insn(&self, off: u64) -> bool; } // a proved instruction word
pub trait SymNames: Sync { fn name(&self, key: SymKey) -> &[u8]; } // a sequence's callee (a64.md §8.4)
pub trait ScanTables: SymNames { fn sym(&self, key: SymKey) -> SymFacts; }
pub trait FillTables: SymNames { /* sym, slot, plt, flags, redirect */ }
pub struct ScanCx<'a> { /* tables: &dyn ScanTables (Inputs, Resolution, Liveness):
                           target kind, symbol type, st_other, STT_TLS, and the event facts
                           dead, comdat_only, gnu_warning; OutputKind; PlaceFacts;
                           name: the place section's name */ }
pub struct DemandSink<'a> { /* over a `dyn DemandOut`, which has the same methods:
                               raise(SymKey, DemandFlags); raise_global(GlobalFlags);
                               count(DynRole, n) for this section; error(record, ScanError);
                               reference(record, EventClass, SymKey) for the record being scanned */ }
pub enum EventClass { Undefined, ComdatMismatch, DiscardedSection, GnuWarning } // image.md §4.4
pub struct PlaceFacts { pub alloc: bool, pub writable: bool, pub exec: Option<bool> } // the place's section
```

- **`PlaceFacts.exec`** (WP-030c, an additive change): the place
  section's `SHF_EXECINSTR`, `None` where the caller cannot know it.
  i386 refuses a `GOT32`/`GOT32X` in a non-executable section at scan
  when it is `Some(false)` (`spec/x86.md` X86-13 step 1), and at fill
  (`SectionOut.exec`) when it is `None`. Existing constructors keep
  compiling: `PlaceFacts::new(alloc, writable)` gives `exec: None`.

- **Contract**: given one live relocation, raise the demands the forms
  `apply` may choose need (§5.2), count the dynamic relocations it puts at
  its own place, or report why it cannot be satisfied. Generic code
  implements it from the demand class (§4.3); an architecture adds the
  flags of its relaxation rules (`GOT_NORELAX`) and of sequences (§3.5).
- **Parallel**: scan runs per file in parallel. `raise` is a bitwise OR
  into the per-key flag word (commutative, idempotent); `raise_global` the
  same into one word; `count` adds into a per-section counter owned by the
  unit that scans that section (one file); errors are keyed
  `(70, FileId, section index × 2^32 + record index)` (model §8). The
  result is independent of scheduling and thread count.
- **Checks made at scan** (they need no address): the type's status; the
  addend rule; the instruction the field form requires (a64.md §3.2); the
  window lies in the section; the target kind against the class table
  (`relocation-incompatible`); overlapping windows.
- **Overlap.** Each relocation, and each matched sequence taken as one
  unit, has the window of the form `apply` will write. Two units whose
  windows overlap are refused (feature `overlapping relocations`).
  - The members of one matched sequence are exempt from each other.
    The x86-64 `TLSGD` lead's window `[P − 4, P + 12)` contains the
    field of its own `PLT32`, which is not an overlap.
  - A sequence whose pattern does not match is not a unit. Each member
    then has its ordinary field window, and an unmatched x86-64 `TLSGD`
    with its call does not overlap.
  - Markers (`Field::None`) occupy no bytes unless a rule rewrites
    their word.
  - **REL** (`spec/x86.md` R-13 (b)): on a target whose relocation
    sections are `SHT_REL`, the check compares read windows (§3.3), not
    only write windows, and it also refuses two records with the same
    `r_offset` unless they are members of one matched sequence, or one of
    them is a marker (width 0, e.g. `R_386_NONE`), which reads and writes
    nothing; the refusal is the same, at the scan key of the later record. No unit
    then reads a byte another unit writes, so the bytes a unit reads at
    fill are input bytes, whatever the record order and the clip.
- **Places in non-alloc sections** (no `SHF_ALLOC`): §4.3's
  non-alloc rule applies before the class table.
- **Undefined symbols are reported by scan**, not by resolution.
  - **Which symbols.** A symbol has target kind `Undefined` when it is
    undefined and not weak, no input defines it (including shared
    objects), and `spec/resolve.md` has not made it a dynamic undefined.
    A dynamic undefined is a `Shlib` target, under `--unresolved-symbols`
    or `-z undefs`, the default for `Shared`.
  - **What is reported.** For every live relocation against an
    `Undefined` target that still needs its symbol, scan reports the
    event `reference(Undefined, key)` and no diagnostic; the file and
    section are those of the section being scanned, as for `error`. The
    scan driver (`spec/image.md` §4.4, §4.5) turns the events into one
    `undefined-symbol` per key: GNU ld's phrase ``undefined reference to
    `<name>'`` in the head, the first three references in `(FileId,
    section index, record index)` order as `>>> referenced by
    file:(section+0xoff)`, then `>>> referenced N more times`.
    `comdat-mismatch` (event `ComdatMismatch`, per key; reported instead
    of `Undefined` when `SymFacts.comdat_only`),
    `discarded-section-reference` (event `DiscardedSection`, per symbol
    and referring section) and the per-key `gnu-warning` (event
    `GnuWarning`, for every surviving reference to a key with
    `SymFacts.gnu_warning`) are reported the same way. The sink names
    the key; the driver forms the event's group from it (`spec/image.md`
    §4.4: the key itself, or for `DiscardedSection` the referring
    section index and the local key's symbol index). An `Undefined`
    whose verdict is "nothing" is dropped by the driver, which holds the
    verdict per key (`spec/image.md` I-19).
  - **The key** of each grouped diagnostic is its least reference's
    `(70, FileId, section index × 2^32 + record index)`: model §8's scan
    key.
    It names the place's `SectionId` (`FileId`, section index), and the
    record index orders the places within it. The record index stands in
    for the offset because two records can share one offset. The result
    is independent of scheduling, because the first reference is chosen
    by that order after all events are in.
  - **Relocations consumed by a sequence make no reference.** A relocation
    consumed by a matched sequence (§3.5) does not need its symbol: the
    x86 GD→LE/IE and LD→LE call (Appendix A.3, A.4, B.5), and AArch64
    R4's `CALL26` (a64.md §8.4). A static link in which `__tls_get_addr`
    is undefined must therefore link when every reference to it is
    consumed. `g++ -static` on x86-64 and i386 depends on it:
    `libstdc++.a(eh_globals.o)` references `__tls_get_addr`, and no
    `libc.a` member defines it. Archive extraction (`spec/resolve.md`)
    still counts the reference, so a definition is pulled in when one
    exists.
  - **Only input-only facts decide.** Whether a relocation needs its
    symbol is decided at scan from input-only facts: the pattern, the
    output kind, and the target kind, which are the kind-1 rules of
    §5.2. A kind-2 relaxation, one that depends on addresses or on
    `Demands`, never decides it. Its unrelaxed form references the
    symbol, so the reference stands. This is the conservative-demand
    rule applied to references: scan raises the demands and reports the
    references of every form `apply` may write (§5.2), and a consumed
    `CALL26` is in no form.
  - **Hidden symbols.** A `STV_HIDDEN` or `STV_INTERNAL` undefined
    symbol with a surviving reference is `undefined-symbol` whatever
    `--unresolved-symbols` or `-z undefs` says, because it cannot be
    resolved at load time.
  - **Non-alloc places** (§4.3) are reported like any other place.
- Scan sees only live sections. A relocation in a live section against a
  symbol in a section that is not live (`SymFacts.dead`) demands nothing
  and is classified by `spec/resolve.md` §6.3's table of referring
  sections (`mortar_arch::dead_reference(name, alloc)`): from a debug
  name it is accepted and `fill` writes the tombstone (§3.11); from
  `.gcc_except_table` the field is 0; from `.eh_frame` the FDE is dead
  (`spec/model.md` §3.6) and nothing is reported (`DeadRef::DeadFde`,
  WP-027d: the row was matched by name in the scan); from any other
  section scan reports the event `DiscardedSection`.

### 3.5 Sequences

Some rewrites span several relocations (AArch64 R4 and R5, x86-64 and
i386 TLSGD/TLSLD with their call, Appendix A.3). They are expressed
without per-relocation state:

```rust
pub trait Records { fn len(&self) -> u32; fn get(&self, i: u32) -> Option<Reloc>; }
```

- A sequence has a **lead** record and **followers** at fixed record
  distances after it (`+1`, `+2`), recognized by type, `r_offset`
  distance, symbol and instruction bytes as the architecture spec states.
- The **pattern** predicate is a pure function of the lead's record, the
  followers' records and the section bytes. Each member evaluates it from
  the lead it finds by looking back a fixed distance; all members
  therefore agree, in scan and in fill, in any order.
- A member whose pattern does not hold is an ordinary relocation.
- `unit_window` gives each relocation the window of the unit it belongs
  to for §3.4's overlap check: by default its field window, with its own
  record index as the unit; a member of a matched sequence has the
  window of the words it owns (a64.md §8.4: the `CALL26` owns
  `[o+8, o+16)`) and the lead's index as the unit. `consumed` says that
  a member makes no reference, raises no demand and is not a branch
  site (a64.md §8.4's `CALL26`). Both are functions of the output kind,
  the record, its section and `SymNames` only (a kind-1 rule).
- The look-back is bounded by the architecture (AArch64: 2; x86-64 and
  i386: 1), so `scan` and `fill` stay linear.
- `sequence_extent(records, i) -> u32`: the number of records from `i`
  that a sequence recognized at `i` may consume (1 for a record that
  starts no sequence). It reads only the records. The scan driver does
  not split a section's records between blocks inside an extent
  (`spec/image.md` §4.1). AArch64: 3 at a 513 followed by a 514 and a
  `CALL26`, 2 at a 311 followed by a 312.
- **`RelocAt.leads`** is `false` only when no record of the relocation
  section has a type in `Facts.seq_leads`; `true` is always correct.
  Scan passes `true`, and records the exact bit per section (one
  comparison per record, as `spec/image.md` I-29's order check) for
  fill. An architecture skips the patterns of the sequences those types
  lead when it is `false`, so an ordinary `CALL26` in an output with no
  GD sequence pays no pattern test (AArch64 R4).
- Because the pattern is input-only, a demand it removes (the follower's
  `PLT` for `__tls_get_addr`) may be removed (model §3.7 rule for
  resolution-only conditions).

### 3.6 `apply`

```rust
pub struct FillCx<'a> { /* the tables of model §3, §4: sym(SymKey) -> address, kind, size,
                           tls (STT_TLS: the address is then in the TLS template);
                           slot(SymKey, SlotKind) -> address; plt(SymKey) -> address;
                           got_base(); tls(); flags(SymKey) -> DemandFlags;
                           redirect(SectionId, offset) -> patch address; output kind; options */ }
pub struct SectionOut<'a> { /* the unit's output bytes in the clip [a, b) of section
                               offsets, pre-filled with the input bytes; P of offset 0;
                               written through write(off, bytes); writes outside the
                               clip are discarded; the section's name, SHF_ALLOC and
                               SHF_EXECINSTR (exec: Option<bool>, None when the caller
                               does not know it); for input(): the resident section,
                               or the clip's two margins of input bytes */ }
impl SectionOut<'_> {
    /// The input bytes at section offsets [off, off + dst.len()): from the
    /// resident section, the window buffer, or the clip's two margins.
    /// false when a byte is in none of them.
    fn input(&self, off: u64, dst: &mut [u8]) -> bool;
}
```

- **`SectionOut.exec`** (WP-029b): x86-64 decides `DTPOFF32`/`DTPOFF64`
  by it (`spec/x64.md` X-46: `TpRel` in an executable section of an
  executable, `DtpRel` elsewhere). The image phase sets it from the
  output section's flags; an architecture whose rule needs it refuses by
  name (`DTPOFF relocation in an allocated section of unknown kind`)
  when it is `None`, never
  guesses.

- **Contract**: compute `X` for one relocation and write it, or write the
  relaxed form when the architecture's rule says so. Reads: the
  context, the input bytes (`at.site`), the relocation records. Writes
  only `[r_offset − window.before, r_offset + width + window.after)` of
  `out`. It reads input bytes of the place only through
  `SectionOut::input`, only inside its own read window (§3.3), and before
  its unit writes; it never reads bytes a relocation has written. (A RELA
  architecture reads none: its addends and pattern bytes come from
  `at.site`; REL reads the addend this way, `spec/x86.md` R-13 (d).)
  `out` may hold only a clip of the section
  (`spec/image.md` I-35): `apply` writes through `SectionOut`'s methods,
  and the bytes it writes inside the clip are those of an unclipped
  call. A relocation whose window straddles two clips is applied once
  per clip, and the union is the unclipped result. The field lying
  outside the section is judged against the input section (`at.site`),
  never against the clip.
- **Order independence**: windows of different sequences do not overlap
  (checked at scan), and within a sequence each member writes its own
  words (the architecture spec assigns every rewritten word to exactly
  one member). Filling relocations in any order gives the same bytes.
- **Relaxations** are rules inside `apply`. Each architecture spec gives,
  per rule: its precondition, split into an input-only part (`Inputs`,
  `Resolution`, output kind, bytes, records) and an address part; the
  exact output words; and how the verifier recognizes the taken form. A
  precondition that reads `Demands` (AArch64 R5's `GOT_NORELAX`) counts
  as an address part for §5.2.
- **Checks**: the value's check and alignment, the slot's existence
  (`slot()` returning none is an internal error: §5.2 was violated), the
  redirect's reach. Failures return `FieldError` and become §7's
  diagnostic.
- `apply` is not called for `Relocatable` outputs.
- **Rule 1: proof of instruction.** A rewrite of a word that carries no
  relocation, or a decision taken by decoding such a word, requires proof
  that the word is an instruction. A word with a relocation is an
  instruction of the class its field form requires (a64.md §3.2), and
  that is checked. A word without one is proved by the architecture's
  instruction ranges:
  - **AArch64:** the `$x` mapping-symbol ranges of a64.md §3.4. This
    covers R4's `o+12`, the BTI check of a thunk target, and every word
    of the 843419 predicate.
  - **x86:** no rule rewrites a word outside a relocation's window. The
    window bytes are tied to the relocation and checked by its pattern.
  - **Synthetic chunks:** every word of mortar's own code chunks is an
    instruction.

  A word that cannot be proved is not rewritten. When skipping it would
  be unsafe (an erratum candidate), the link is refused by name (a64.md
  §11.1).

### 3.7 Shapes

```rust
pub struct GotShape { pub slot_words: fn(SlotKind) -> u8 }   // Addr 1, TpOff 1, TlsGd 2, TlsLd 2, TlsDesc 2 (x86-64 2, i386 2)
pub enum PltVariant { /* per arch: AArch64 Plain, Bti, Pac, BtiPac; x86-64 Lazy, Ibt;
                         i386 Abs, Pic, IbtAbs, IbtPic */ }
pub struct PltShape {
    pub chunks: &'static [PltChunk],   // one per output section the PLT occupies, in address order
    pub call_chunk: u8,                // the chunk whose entry is the symbol's PLT address (L)
    pub lazy_chunk: u8,                // the chunk whose entry the .got.plt word initially points to
    pub iplt_entry: u32,               // bytes of an IPLT entry (in call_chunk's section)
}
pub struct PltChunk { pub name: &'static str, pub header: u32, pub entry: u32, pub align: u32 }
pub enum PltPart { Header(u8), Entry { chunk: u8, n: u32 }, Iplt(u32) }
pub struct Branch { pub reach: Reach, pub veneer: bool }   // Reach { min: i64, max: i64 }
pub struct ThunkShape { pub size: u32, pub align: u32 }
pub enum Erratum { A53_843419 }
```

- **Sizes are not addresses**: every size here is a function of the
  variant and the counts in `Slots`, so synth fixes them before layout
  (§5.1). `write_plt` and `write_thunk` compute their words from the
  addresses in their context (`SynthCx`: the entry's and header's
  addresses, its `.got.plt` word, the GOT base, the entry's `.rela.plt`
  index; `PatchCx`: the patch's address, its target, the site).
- **PLT variant** is a function of the output kind, the merged GNU
  properties and the options (`-z pac-plt`, `-z force-ibt`), chosen once
  per link.
- **Lazy binding [M2]** (`spec/dyn.md` D-49). A `JUMP_SLOT` word of
  `.got.plt` initially holds its lazy target at the link-time address
  (in a PIE the loader adds the bias): `lazy_target(n)` names it,
  `LazyTarget::Plt0` (the PLT header: AArch64, a64.md §10.2) or
  `LazyTarget::EntryPlus(k)` (the entry's own lazy stub at offset `k`
  of entry `n` of `lazy_chunk`: x86-64 `k = 6`, the `push`, x64.md
  §10.3; i386 [M4] likewise, x86.md X86-52). The word is a function of
  the shape and the PLT index, never of the symbol.
- **`-z now` [M2]** keeps every shape: PLT0 and the lazy stubs are still
  written and every size is the lazy variant's (D-49; all three
  references). What `-z now` changes is generic: `.got.plt` becomes its
  own chunk inside RELRO (`spec/layout.md` L-60 rank 7), and
  `DT_FLAGS`/`DT_FLAGS_1` (D-30). `-z now` is a `now` flag beside the
  variant (WP-040 step 0c's `PltVariant::Now`, revised so that it
  combines with AArch64's `Bti`; WP-041), and it changes neither the
  `PltShape` nor the bytes of the variant. There is no `.plt.got` (D-49): a
  key with both a GOT and a PLT use gets a PLT entry and a GOT slot.
- **Thunks**: `branch(h)` says which types are branches, their reach, and
  whether a veneer may be inserted (AArch64 `CALL26`/`JUMP26`). `thunk()`
  is `None` where no type allows one (x86-64, i386). `thunk_target_ok`
  refuses a target the thunk cannot reach correctly (AArch64 BTI landing
  pads, a64.md §9).
- **Errata**: `errata()` lists the errata the options enable.
  `erratum_sites` reports the sites (`ErratumSite { q, f }`: the
  addresses of `W1` and of the final word) in the words of one whole
  output code section (`Code { bytes, addr, proof }`), in address order
  across input-section and chunk boundaries, given its address and the
  instruction/data ranges (a64.md §11.1), or the first candidate that is
  neither proved code nor safely skipped (`Unproved`, a64.md §11.1
  condition 5), which the caller refuses; `patch_reach` is the reach of
  each of the two branches between a site's final word and its patch
  (AArch64: `s(28)` each, so the window of a64.md §11.2); `erratum_in_place` says
  whether a site is fixed without a patch at these addresses (AArch64:
  ADRP → ADR); `patch(e)` gives a patch's size and alignment as
  `thunk()` gives a thunk's (AArch64 843419: 8 bytes, alignment 4);
  `write_patch` writes a patch.
- **The unproved refusal.** `Unproved` names the unit of the unproved
  word, `W1`'s code offset `q`, and the unit and offset of `W1`
  (`q_unit`, `q_unit_offset`): the caller tags the proof runs with its
  units, so it maps them to the candidate's input section. The
  diagnostic is `mortar_arch::diag::unproved(e)` (a64.md §11.1's name
  and the option that turns the fix off) at that place, key
  `diag::unproved_key(ChunkId of the section)`, `(90, 0, ChunkId)`.
- **Sites in synthetic chunks cannot occur.** a64.md §11.1 ("Sites and
  synthetic chunks") proves that no site has a word in a synthetic code
  chunk: no shape mortar writes (PLT header and entries, IPLT entries,
  thunks, erratum patches) holds a `W1` that starts one, and layout
  never starts a synthetic code chunk of non-zero size at a page offset
  `0xFFC`, `0x000`, `0x004` or `0x008` (`spec/layout.md` L-118), so no
  input `W1` runs into one; the padding is data and breaks the
  candidate. A site whose final word is not in an input section is
  therefore an internal error (`internal-error`), never a user
  diagnostic, and tests enforce both halves (a64.md §16).

### 3.8 TLS

```rust
pub struct TlsAbi {
    pub variant: TlsVariant,            // I { tcb: 16 } (AArch64) or II (x86-64, i386)
    pub get_addr: &'static [u8],        // "__tls_get_addr", "___tls_get_addr" (i386)
    pub module_base: Option<&'static [u8]>, // "_TLS_MODULE_BASE_" (AArch64, x86-64 TLSDESC LD)
}
```

With `tls = Placement.tls` and the validity rule `tls.start ≡ 0 (mod
tls.align)` (§9, M-3):

- variant I: `TpRel(x) = x − tls.start + align_up(tcb, tls.align)`;
- variant II: `TpRel(x) = x − tls.start − align_up(tls.size, tls.align)`
  (negative);
- `DtpRel(x) = x − tls.start` for both.

The executable's module id is 1. Which relaxations exist, and their
words, are the architecture spec's.

### 3.9 `.eh_frame`

```rust
pub struct EhEncodings { pub hdr_ptr: u8, pub hdr_count: u8, pub hdr_table: u8 }
```

`.eh_frame_hdr` is written with `eh_frame_ptr` encoding `0x1b`
(`pcrel|sdata4`), `fde_count` `0x03` (`udata4`), table `0x3b`
(`datarel|sdata4`) on all three targets (the LSB's `.eh_frame_hdr`
format). The pointer encodings inside CIEs and FDEs are the producers';
the FDE's initial location is read through its first relocation, whose
class must be `Pc` or `AbsWord` (§4.3); another is refused by name.

### 3.10 Dynamic relocations

```rust
pub enum DynRole { Relative, Symbolic, GlobDat, JumpSlot, Copy, IRelative,
                   DtpMod, DtpOff, TpOff { negated: bool }, TlsDesc }
```

`dyn_type(role)` is the architecture's type number (a64.md §6.2), `None`
where the architecture has none (i386 has `TpOff{negated: true}` as
`TLS_TPOFF32`, the others do not). RELR eligibility is generic: role
`Relative`, place aligned to `word`, the value stored at the place equal
to the addend (`-z pack-relative-relocs`, [M4], `spec/dyn.md` D-27,
D-62).

**Record form per role [M2]** (`spec/dyn.md` D-46). What the place holds
when the record is written is generic, from `Facts.rel_form`:

| role | RELA: place | RELA: `r_addend` | REL (i386, [M4]): place |
|---|---|---|---|
| `Relative` | `S + A` | `S + A` | `S + A` |
| `Symbolic` | 0 | `A` | `A` |
| `GlobDat` | 0 | 0 | 0 |
| `JumpSlot` | the lazy target (§3.7 `lazy_target`, D-49), under `-z now` too | 0 | the lazy target |
| `IRelative` | per architecture (`spec/dyn.md` §15 Q7): AArch64 the resolver's link address (`S + A`, a64 §6.2); x86-64 0 (x64 X-39) | the resolver's address | the resolver's address (the word is the addend) |
| `Copy` | none: the allocation is `NOBITS` | 0 | none |
| `TpOff`, `DtpMod`, `DtpOff`, `TlsDesc` against a shared key | 0 | 0 | 0 |

`dyn.md` D-46 is normative for which record a slot or word gets, and
D-48 for the TLS records (an executable's own TLS is relaxed to LE and
has no record). The place contents of the TLS roles under REL are [M4]
with the i386 rules (`spec/x86.md`).

**Dynamic tags per architecture [M2].** `dyn_tags(cx)` lists the
architecture's own `.dynamic` entries and their conditions, which the
generic writer places at `spec/dyn.md` D-30 rows 24-25: AArch64
`DT_AARCH64_BTI_PLT` (the output has the BTI property) and
`DT_AARCH64_VARIANT_PCS` (some `JUMP_SLOT` key is
`STO_AARCH64_VARIANT_PCS`), `spec/a64.md` §10.3; x86-64 and i386 none
(`DT_X86_64_PLT*` is never written in an executable, D-30). Each
condition depends on no address.

### 3.11 Tombstones

A relocation in a non-alloc section with a debug name (`spec/resolve.md`
§6.3) whose target is in a section that is not live writes
`tombstone(name, class)` truncated to the field instead of `X`
(`spec/image.md` I-47): 1 for `.debug_ranges` and `.debug_loc`; 0 for
`.debug_rnglists`, `.debug_loclists` and `.debug_addr` (an address there
is paired with a length, which all ones would wrap); all ones
for `.debug_aranges` and `.debug_frame`; for every other debug name all
ones in ELFCLASS64 and 0 in ELFCLASS32. In `.debug_aranges` the tuple's
length is also written 0 (I-112): that write is the image phase's, not
`apply`'s, because it depends on whether another record has its field
there; `apply` writes only its field, `max_reach` does not include it,
and the image raises its own search bound (I-35). `.gcc_except_table`
writes 0, and other names are refused at scan (`spec/resolve.md` §6.3,
§3.4 above). `-z dead-reloc-in-nonalloc=` is refused by name, later.

### 3.12 Undefined weak

```rust
pub enum Weak0Rule { SZero, SIsP, XZero, XConst(i64) }
```

For a `Weak0` target, `weak0(h)` says how the value is formed: `S = 0`,
`S = P`, `X = 0`, or a constant `X` (AArch64 branches: 4). a64.md §7 gives
the AArch64 table; x86 uses `SZero` for every class.

**`WeakDyn` or `Weak0` [M2]** (`spec/dyn.md` D-25, normative):

```rust
pub enum WeakClass { Got, AbsWord, Branch, Unfillable }
```

`weak_use(r_type, place, writable)` gives the class of one live
relocation that names a weak undefined key: `place` is the input
section's bytes up to the place (the x86 branch-opcode test reads
offsets −1 and −2), `writable` whether the place's output section has
`SHF_WRITE` after `spec/layout.md`'s section rules (RELRO sections
count as writable). `None` is a type that does not use the key (D-25's
last row: `NONE`, x86 `SIZE*`, `GOTPC*`). A type absent from the
architecture's column is `Unfillable`, never `None`. Each architecture
spec carries its column of D-25's table (`spec/a64.md` §7, `spec/x64.md`
X-16, `spec/x86.md` §5). The decision is generic: in `Exec` and `Pie`
the key is `WeakDyn` when some use is `Got` or `AbsWord` and none is
`Unfillable`; in `Pie` a key with an `Unfillable` use is refused once,
`weak-undefined-pc-in-pie`; otherwise `Weak0`. Against a `WeakDyn` key
every `Branch` demands `PLT` (§4.3), and the GOT relaxations are off.

### 3.13 GNU properties

`property_merge(pr_type)` gives the rule for a processor-range type
(0xc0000000..0xdfffffff): `And` (an input without the property counts as
0; the property is omitted when the result is 0; AArch64 `FEATURE_1_AND`
with BTI, PAC and GCS, x86 `FEATURE_1_AND` and the rest of
`0xc0000002..0xc0007fff`), `Or` (the OR of the inputs
that have it, present if any has it; x86 `ISA_1_NEEDED` and the other
`0xc0008000..0xc000ffff` types, and `COMPAT_ISA_1_NEEDED` 0xc0000001), `OrAll` (the OR of the inputs, present
only if every input has it; x86 `ISA_1_USED`, `FEATURE_2_USED` and the
other `0xc0010000..0xc0017fff` types, and `COMPAT_ISA_1_USED` 0xc0000000; `spec/x64.md` X-41), `EqualOrAbsent` (AArch64
`FEATURE_PAUTH`: copied when every input that has it agrees, refused
when two disagree) or `None` for a type the architecture does not know,
which `spec/synth.md` S-95 drops with the warning `unknown-property`.
The generic types and ranges (below 0xc0000000, including `Max` for
`GNU_PROPERTY_STACK_SIZE`) are S-95's, not the architecture's.

## 4. Demands

### 4.1 Target kinds

§2.1. Computed by generic code, once per key, from `Resolution`; not
stored.

### 4.2 Flags

```rust
bitflags DemandFlags: u16 {          // per SymKey (model §3.7)
    GOT, PLT, CANON_PLT, COPY, EXPORT,
    IPLT,                            // non-preemptible IFUNC: an IPLT entry, its address everywhere
    GOT_TPREL, TLS_GD, TLS_DESC,
    GOT_NORELAX,                     // some use forbids the GOT relaxation (a64.md R5)
    TLSIE_NORELAX,                   // some use forbids IE -> LE (a64.md R3)
}
bitflags GlobalFlags: u8 {           // per output
    GOT_BASE,                        // the GOT base symbol is referenced
    TLS_LD,                          // the module slot of local-dynamic TLS
    STATIC_TLS,                      // DF_STATIC_TLS
}
```

`IPLT`, `GOT_NORELAX`, `TLSIE_NORELAX` and the global word are additions
to model §3.7 (§9, M-1). A flag means "some valid layout may use this"
(or, for the `*_NORELAX` flags, "some use forbids this rewrite"); it is
never cleared within a link.

**Flags that reach other files' bytes.** For some flags, a flag raised
by a relocation in file F changes the bytes `fill` writes for a
relocation in another file G, even with no change in G:

| flag | why it reaches other files' bytes |
|---|---|
| `GOT_NORELAX` | It turns every R5 pair against the symbol back into a GOT load. |
| `TLSIE_NORELAX` | It turns every R3 use back into IE. |
| `CANON_PLT` | It changes `S` of a function, for every file, to its PLT entry. |
| `COPY` | It moves the symbol to the copy allocation. |

These four must be in the incremental dependency index (model M-8, §9).
`GOT`, `PLT`, `IPLT`, `TLS_GD`, `TLS_DESC`, `GOT_TPREL` and `EXPORT`
reach other files only through `Slots` and addresses, which the index
already covers. `IPLT` is raised by every reference to a non-preemptible
IFUNC, so any file that uses the entry raised the flag itself.

### 4.3 Demand classes

Every `Howto` names one class. The table gives, per output and target
kind, the demands or the error. `R`: a `Relative` dynamic relocation at
the place (counted); `D`: a `Symbolic` one, with `EXPORT`; `C`: `COPY`
for a data symbol, `CANON_PLT` for a function; `I`: `IPLT`; `E`: the
error `relocation-incompatible` (§7; its M1 meaning: the relocation
cannot express the target in this output kind); `TX`: the error
`text-relocation` when the place is in a read-only section, since the
record it needs would be a text relocation (`spec/dyn.md` D-47; no
`DT_TEXTREL` is ever written); `—`: nothing. Two `E` cells of a dynamic
`Pie` carry the names of their `spec/dyn.md` rules: `Pc` against
`Shlib{func: true}` is `canonical-plt-in-pie` (D-44), and a `Weak0` key
with an unfillable use is refused per key, `weak-undefined-pc-in-pie`
(D-25). The kinds are registered in `mortar_diag` by WP-041 (and listed
in `spec/diag.md` with them).
`Static` never has `Preemptible`, `Shlib` or `WeakDyn` targets.

| class | output | Local | Abs | Preemptible | Shlib | Ifunc | Weak0 | WeakDyn |
|---|---|---|---|---|---|---|---|---|
| `AbsWord` | Static, Exec | — | — | | writable `D`, else `C` | `I` | — | writable `D`, else `E` |
| | Pie, Shared | `R` + `TX` | — | `D` + `TX` | `D` + `TX` | `I`, `R` + `TX` | — | `D` + `TX` |
| `AbsNarrow` | Static, Exec | — | — | | `C` | `I` | — | `E` |
| | Pie, Shared | `E` | — | `E` | `E` | `E` | — | `E` |
| `Pc` | Static, Exec | — | — | | `C` | `I` | — | `E` |
| | Pie, Shared | — | `E` | `E` | `Pie`: `C` against `Shlib{func: false}` (`spec/dyn.md` D-43), `E` against `Shlib{func: true}` (D-44); `Shared`: `E` | `I` | the architecture spec's §7 (x86-64: `E`, `spec/x64.md` X-15; AArch64: `S = P`) | `E` |
| `Lo` (low bits of an address paired with a `Pc` page) | any | — | — | as `Pc` | as `Pc` | `I` | — | as `Pc` |
| `Branch` | Static, Exec | — | — | | `PLT` | `I` | — | `PLT` |
| | Pie, Shared | — | `E` | `PLT` | `PLT` | `I` | — | `PLT` |
| `Got` | any | `GOT` | `GOT` | `GOT` | `GOT` | `GOT`, `I` | `GOT` | `GOT` |
| `GotRel` (value relative to the GOT base) | any | `GOT_BASE` | Static/Exec `GOT_BASE`; else `E` | `E` | `E` | `I`, `GOT_BASE` | `E` | `E` |
| `GotBaseRef` (the GOT base itself, x86 `GOTPC`) | any | `GOT_BASE` | | | | | | |
| `Size` | any | — | — | `E` | — | — | — | `E` |
| `TlsGd`, `TlsLd`, `TlsIe`, `TlsDesc` | any | the architecture spec's TLS table (a64.md §8, §10.1, §12) | | | | | | |
| `TlsLe` | Static, Exec, Pie | — if the target is TLS and defined here, else `E` | | | | | | |
| | Shared | `E` | | | | | | |
| `TlsDtpRel` | any | — if defined here, else `E` | | | | | | |
| `None` | any | — | | | | | | |

**Non-alloc places.** The table is for places in `SHF_ALLOC` sections. A
place in a section without `SHF_ALLOC`:

- is filled with link-time addresses in every output kind;
- raises no demand (except `GotRel`'s `GOT_BASE`, below) and no `E` of
  the table, and never gets a dynamic relocation;
- takes `S` as the link-time value. That is 0 for a `Weak0` target, and
  0 for a `Shlib`, `Preemptible` or `WeakDyn` target that is not defined
  in this output. An `Undefined` target is still the `undefined-symbol`
  error of §3.4, as GNU ld 2.44 and mold 2.37 report a reference from
  `.debug_info` (lld 22 accepts it).
- against a target in a section that is not live, gets
  `tombstone(section name)` (§3.11) in place of `X`, truncated to the
  field.

Only data relocations are allowed there:

- `None`;
- `AbsWord`, `AbsNarrow`, `Pc`, `GotRel`, `TlsDtpRel` and `Size`, each
  with a `Field::Data` field. `GotRel` writes `S + A − GOT` (x86-64
  `GOTOFF64` in GCC's large-model `.debug_info`, `spec/x64.md` X-53) and
  raises `GOT_BASE`, so that the base exists even when the code the
  debug entry describes is not live.
  `AbsWord` and `AbsNarrow` against an `STT_TLS` target write
  `DtpRel(S + A)`, the offset in the TLS block, not an address (GCC
  `-gsplit-dwarf` `.debug_addr` entries for `DW_OP_form_tls_address`,
  `spec/x64.md` X-53); this is the one exception to the rule that a
  non-TLS class against a TLS symbol is `E`, and it holds at non-alloc
  places only.

An instruction form of one of those classes is not a data relocation.
a64.md §13.2 lists the AArch64 types: `NONE`, `ABS64/32/16`,
`PREL64/32/16`, `TLS_DTPREL64`, and (M2) `GOTREL64/32`. Any other relocation is `relocation-incompatible` (reason `not
allowed in a non-allocated section`). This is what every `-g` PIE needs.
GNU ld, lld and mold resolve `.debug_*` relocations statically (a64.md
§13.2).

An `Undefined` target (§2.1) has no column, because every class gives
`undefined-symbol` (§3.4). The one exception is a relocation consumed by
a matched sequence, which references nothing.

The `Weak0` columns show demands; what is written is §3.12's rule. A
`Got`-class relocation against `Weak0` demands a slot that holds 0 with no
dynamic relocation. A non-TLS class against a TLS symbol, and a TLS class
against a non-TLS symbol, are `E` (at a non-alloc place, `AbsWord` and
`AbsNarrow` against a TLS symbol are the exception above). Each architecture spec lists which of
its types belong to which class (a64.md §5).

The `WeakDyn` column's `E` cells are unreachable in `Exec` and `Pie`
[M2]: `spec/dyn.md` D-25 makes a key with a use of D-25's `unfillable`
class `Weak0` in `Exec`, and refuses it per key in `Pie`
(`weak-undefined-pc-in-pie`, §3.12) before any cell is read; a
`WeakDyn` key therefore meets only `AbsWord` in a writable place, `Got`
and `Branch` (which demands `PLT`, with a `JUMP_SLOT`, in both kinds).
In a dynamic `Pie` the `Weak0` cell of `Pc` is likewise preceded by
that refusal; it stands for a static PIE.

### 4.4 Slot contents

The content of each slot, and the dynamic relocation it carries, per
output and target kind, is the architecture spec's table (a64.md §10.1);
the dynamic relocation types come from `dyn_type`. For `Exec` and `Pie`
the record each slot and absolute word gets is `spec/dyn.md` D-46
(normative, one table for every architecture), the TLS records D-48,
and the place contents per record §3.10.

## 5. What is decided when

### 5.1 The order

| what | depends on | computed by | kept in |
|---|---|---|---|
| `Howto` of a relocation | the type, the section bytes in the window | `classify` | nowhere (recomputed) |
| target kind | `Resolution`, output kind | generic | nowhere |
| demand flags, dynamic relocation counts, scan errors | `Howto`, target kind, input-only relaxation and sequence predicates | `scan` | `Demands` |
| PLT variant | output kind, merged properties, options | `plt_variant` | synth's input |
| slot indices; PLT, IPLT, GOT and `.got.plt` sizes | `Demands`, shapes | synth (`spec/synth.md`) | `Slots` |
| thunks, erratum patches, their redirects | addresses (§6) | layout with `branch`, `thunk`, `erratum_*` | `Placement.patches` |
| whether a relaxation is taken | its precondition (input part and address part) | `apply` | nowhere |
| whether an erratum site is fixed in place | addresses and the redirects | `erratum_in_place` in layout, then `apply` | nowhere (absence of a patch) |
| field values, PLT and thunk words | everything above | `apply`, `write_*` | the image |

Nothing in a later row is an input to an earlier one. In particular no
size depends on an address except patch chunks, which are in
`Placement.sized` (model §4.4).

### 5.2 Conservative demand: the proof obligation

For each relocation `r`, let `forms(r)` be the forms `apply` may write
for it (the ABI operation and each relaxed form an architecture spec
lists), and `needs(f)` the slots and flags a form `f` uses. For each
rule `ρ` with precondition `ρ_in ∧ ρ_addr`:

1. If `ρ_addr` is identically true (no address part, no read of
   `Demands`), scan may evaluate `ρ_in` and raise `needs` of the form `ρ`
   selects only.
2. Otherwise scan raises `needs(unrelaxed)` and, when `ρ_in` holds,
   `needs(relaxed)` too.

**Obligation**: `needs(f) ⊆ Demands` for the `f` that `apply` writes,
for every layout valid for the tables. Each architecture spec states,
per rule, whether it is of kind 1 or 2 (a64.md §8: R1, R2, R4 kind 1;
R3 and R5 kind 2, because they read `TLSIE_NORELAX`/`GOT_NORELAX`), which
discharges it rule by rule. It is checked at run time for free:
`apply` looks every slot it uses up in `Slots`, and a missing one is an
internal error (a bug of the scan rule, never a user diagnostic); the
test of §12 links every relocation of the corpus in every output kind
under layouts permuted by the model's tests (model §11) and must never hit
it.

## 6. Range extension and patches: a monotone fixed point

Layout (`spec/layout.md`) owns where patch chunks ("islands") go; this
section fixes what any policy must satisfy, so that the result is
deterministic and its correctness checkable.

**State**: the layout without patches, then `Placement.patches`: a set
of patches, each `(ChunkId, offset, kind, target)`, and the redirect
list `(SectionId, offset) → patch`. Kinds: `Thunk(Sym(SymKey, addend))`,
`Thunk(Plt(SymKey))`, `Erratum(e)` with the site key `(SectionId, offset
of the key instruction)`.

**Addresses are a function of the patch set.** The addresses `A` are a
pure function of the patch-free layout and of the set of patches, each
with its island and its place in the island's order. Redirects change
no size, so they do not enter. At M2 the state is (patch set, the sizes in
`sized`), with each `sized` size only growing, because `.relr.dyn`
precedes the code (`spec/layout.md` L-21, L-121); the termination
argument below then counts size increases as well as patches (each
size is bounded, so their increases are finitely many).

**Round** `k`, at the addresses `A_k` it starts with:

1. **Needs.** Every condition in this step is judged at `A_k`.
   - **Unredirected branches:** every branch site with no redirect,
     whose class allows a veneer, and whose final target `T` is out of
     `branch(h).reach` of `P`.
   - **Branches out of reach of their thunk:** every branch site whose
     redirect's thunk is out of reach of `P`.
   - **Erratum sites:** every erratum site (`erratum_sites` over each
     output code section's words, a64.md §11.1) with no redirect, that
     `erratum_in_place` does not fix (the ADR reach).
   - **Erratum redirects out of range:** every erratum redirect whose
     patch is out of range, because `b patch` from `F` or `b F + 4` from
     the patch is outside the branch reach. It is a need like the
     others, re-patched to a patch within range, and not a
     `relocation-out-of-range` failure (a64.md §11.2).

   A branch that a matched sequence consumes (a64.md §8.4) is not a
   site.
2. If there are no needs, stop: the layout is the fixed point.
3. **Add.** Visit the needs in output-address order, with ties broken by
   `(FileId, section index, record index)`. Every reach in this step is
   judged at `A_k` as well. A patch created in round `k` has no address
   at `A_k`: its reach is judged from round `k + 1` on, where it exists,
   and if it is out of reach there that is a need like any other. A
   judgement that the round's own insertions make wrong is caught by
   round `k + 1`. This is what makes the step a function of `A_k` and
   the needs, independent of insertion effects inside the step.
   - A thunk need is redirected to the thunk of its key that existed at
     `A_k`, is in reach of `P` and has the lowest address, if there is
     one. Else it goes to a thunk of its key in the island the policy
     chooses within reach of `P` (judged at `A_k`, by the island's
     position): the one created earlier in this step if that island
     already has one (condition (C) allows one per key per island), else
     a new one.
   - An erratum need gets a new patch within range of its final word.
   - If no island within reach can be placed, that is §7's error.
4. Recompute the addresses (`A_{k+1}`) from the new patch set, and start
   the next round.

**Monotone.** Patches are only added. A redirect is replaced only when
its patch is out of reach at the round's addresses, and the replacement
is an existing patch in reach or a new one. No patch is removed, so a
patch can end up unused (requested change M-4). The one removal is an
erratum redirect whose final word is no longer a site when the iteration
ends (a64.md §11.2). It is dropped after the last round, which moves no
address and changes no other site.

`apply` follows the redirect when one exists (the patch form), and the
direct or in-place form otherwise. It never takes the shortcut "the
redirect exists but the direct branch reaches, so branch directly".

**Termination.** Distances do not grow monotonically under alignment
padding, so the argument does not use them. A counterexample: a chunk of
8 bytes aligned to 4 at offset 0, then one aligned to 16 at offset 16.
An 8-byte island before the first chunk shrinks the distance between
them from 12 to 4. The argument that does hold:

1. A round that adds no patch leaves the patch set, and so the
   addresses, unchanged: `A_{k+1} = A_k`.
2. Erratum needs always add a patch, so such a round had only thunk
   needs. Each one was given a thunk in reach at `A_k`, which is
   `A_{k+1}`. Round `k + 1` therefore finds no need and stops.
3. So of any two consecutive rounds, the first adds a patch or the
   second is the last. With `N` the final number of patches, the number
   of rounds is at most `2N + 1`.
4. **Under policy condition (C)**, `N` is bounded and the iteration
   terminates. (C): patches are placed only in islands from a set fixed
   before round 1, with at most one thunk per key and one erratum patch
   per erratum key in each island. Then `N ≤ islands × (thunk keys +
   erratum keys)`, and every term is fixed before round 1, since an
   erratum key is an input word.
5. **Without (C)**, `N` is not bounded by this argument.
6. **The cap is part of the definition**, for every policy. If a link
   needs more than `R_max = 32` rounds, it is refused by name:
   `refused`, class `feature`, name `range-extension fixed point`, key
   `(90, 0, 0)`. This is not a defence that a correct implementation
   never reaches. It is the specified result of such a link. The output
   is never written from a layout that has not reached the fixed point.
7. The corpus test measures the round count. The expected value is ≤ 3.

**Determinism.**
- Each round is a function of the previous round's tables.
- Needs are collected in parallel and sorted (model §6.3).
- Thread count and scheduling do not enter.

**Incremental links.** An incremental link starts from the previous
layout's patches, which are part of the layout, and runs the same rounds.
Before round 1 it must invalidate the following:

- **Redirects whose key names an input section that was edited or
  removed.** The key no longer names the same word. Their patches stay
  as unused chunks wherever the old layout region is reused.
- **Redirects of sites whose relocation record changed:** a different
  target, type or addend under the same key.
- **Thunks whose key's symbol now resolves to a PLT entry or back.**
  Their kind is `Sym` or `Plt`, so the key changed.
- **Every reach.** This needs no special step: round 1 re-evaluates
  every site against the new addresses.

The dependency index (model §7.2) lists:
- for each patch, the units holding its redirected sites;
- for each site, its patch chunk.

The result need not equal the patches a fresh link would choose. Model
§7.3 requires only that a full link given these tables produce the same
bytes, and that the verifier accept them.

**What `Placement.patches` records**: per patch its chunk and offset (the
address follows), its kind and target; and the sorted redirects. The
patch's bytes are not recorded: `write_thunk`/`write_patch` compute them
from the addresses in `fill`. The verifier checks: every redirected site
branches to its patch; every thunk reaches its target; every erratum
patch holds the replaced instruction, relocated at its new address, and
branches back; every erratum redirect names a site; no erratum site
remains unfixed in the output (a64.md §11.3 states the exact rule).

## 7. Diagnostics

One rule for a value that does not fit, and two siblings. All three are
new kinds requested of `spec/diag.md` (§10):

| id | when | place | key |
|---|---|---|---|
| `relocation-out-of-range` | `apply` or `write_*` computes `X` outside the check of its field | the relocation `file:(section+0xoff)`; for a synthetic chunk (PLT entry, thunk, patch) the output section and offset | `(100, FileId, section index × 2^32 + record index)`; `(100, 0, ChunkId)` for synthetic chunks |
| `relocation-misaligned` | `X mod align ≠ 0` | as above | as above |
| `relocation-incompatible` | scan: the class table gives `E`; a non-zero addend on a `MustBeZero` type; a TLS class against a non-TLS symbol or the reverse; the instruction at the place does not match the field form; a disallowed class in a non-alloc section | the relocation | `(70, FileId, section index × 2^32 + record index)` |

- The value and range in the message are those of the check (`X` and
  the check's bounds), in decimal, as lld prints them; for ELFCLASS32 the
  32-bit-reduced value (§2.3).
- Layout does not report overflow: a layout mortar computes is filled,
  and the fill reports it; a layout loaded from a link state that
  violates rule 6 is not reused (phase 15, `spec/incr.md`). So each
  overflow has exactly one key.
- The verifier (phase 110) reports its own kinds (`spec/plumb.md`).
- A relocation whose target is the PLT entry carries the symbol's name;
  a section symbol's relocation names the section and the offset
  (`.text+0x40`).

## 8. Determinism and purity obligations

- `classify`, `scan`, `apply`, `write_*`, `erratum_*` are pure and
  total over their inputs; `scan` writes only through the sink; `apply`
  only its window.
- No method iterates a hash table in an order that reaches its output
  (model §6.5).
- Every rule an architecture spec states is stated in terms of: the
  type, the record, its neighbors (§3.5), the section bytes, the
  `Resolution`-derived target kind and symbol type, `Demands` flags, and
  addresses from the layout. The verifier evaluates exactly these.

## 9. Requested changes to `spec/model.md`

Not edited; for the model's owner.

| # | section | current text | proposed text | reason |
|---|---|---|---|---|
| M-1 | §3.7 | "per `SymKey`, a set of flags (needs a GOT slot, a PLT entry, a TLS descriptor, a TLS module/offset pair, a thread-pointer offset slot, a copy relocation, a canonical PLT address, export to the dynamic symbol table);" | "per `SymKey`, a set of flags (needs a GOT slot, a PLT entry, an IPLT entry, a TLS descriptor, a TLS module/offset pair, a thread-pointer offset slot, a copy relocation, a canonical PLT address, export to the dynamic symbol table, and the inhibitors of the GOT and the IE-to-LE relaxations); per output, a set of flags (the GOT base is referenced, a local-dynamic module slot, static TLS);" | IPLT entries are demanded by references to non-preemptible IFUNCs (a64.md §10.4); AArch64's GOT relaxation is valid only if every use is paired, a fact over all relocations of a symbol (a64.md R5); IE→LE is correct only all-or-none per symbol (a64.md R3, review finding 1); the module slot and the GOT base are not per symbol. |
| M-2 | §3.7 | "A relaxation that depends on distance (§5) never removes a demand; one that depends only on `Resolution` (the symbol is local to the output) may." | "A relaxation that depends on distance, or on `Demands` themselves, never removes a demand; one that depends only on `Resolution`, the output kind and the bytes and relocation records of its own section may (`spec/arch.md` §5.2)." | Sequence patterns (a64.md R4, x86 TLSGD) are input-only and must be allowed to remove the `__tls_get_addr` PLT demand; R5's inhibitor is read from `Demands`. |
| M-3 | §4.4 rule 3 | "… in particular the TLS initialized chunks are directly followed by the uninitialized ones and `tls` describes them; …" | "… in particular the TLS initialized chunks are directly followed by the uninitialized ones, `tls` describes them, and `tls.start` is a multiple of `tls.align`; …" | glibc computes the executable's TP offset from `p_align` alone (a64.md §12.2); a misaligned template gives wrong offsets at run time. |
| M-4 | §4.4 rule 4 | "every site that needs a patch has one in range (…), and every patch is used;" | "every site that needs a patch has one in range (…); every thunk redirect names a thunk of the key its site needs; every erratum redirect names a site of that erratum at the layout's addresses; a patch no site uses is allowed;" | The fixed point is monotone and never removes a patch (arch.md §6); requiring use would make it non-monotone. Erratum redirects that stopped being sites are dropped at the end, without moving any address (a64.md §11.2), so the stronger rule holds for them. |
| M-5 | §4.3 | "the GOT and PLT index of each `SymKey` that demands one;" | "the index of each slot of each `SymKey` that demands one, per slot kind (address, thread-pointer offset, module/offset pair, descriptor), of the output's local-dynamic module slot, and the PLT or IPLT index of each `SymKey` that demands one;" | One key can demand several slots (a64.md §10.1); the module slot is per output. |
| M-6 | §5, "Relaxations are functions of the tables" | "… is decided by a rule in `spec/arch.md` from `Resolution`, `Demands` and the addresses the layout gives, evaluated in `fill`." | "… is decided by a rule in `spec/arch.md` from `Resolution`, `Demands`, the addresses the layout gives, and the bytes and relocation records of the same input section, evaluated in `fill`." | Sequences (arch.md §3.5) read neighboring records; this is input data, not a table. |
| M-7 | §4.4 `patches` row | "an erratum patch (AArch64 843419);" | "an erratum patch (AArch64 843419), keyed by the `(SectionId, offset)` of the instruction it replaces;" | Makes the redirect of an erratum patch the same shape as a thunk's. |
| M-8 | §7.2, the dependency index | "for each thing whose address or content can change (a section, a fragment, a slot, a symbol-anchored allocation, a symbol's definition), the **units** (§5) whose bytes depend on it." | "for each thing whose address or content can change (a section, a fragment, a slot, a symbol-anchored allocation, a symbol's definition, a patch and its redirects, and each per-symbol demand flag that another file's bytes depend on: `GOT_NORELAX`, `TLSIE_NORELAX`, `CANON_PLT`, `COPY`, `spec/arch.md` §4.2), the **units** (§5) whose bytes depend on it." | These flags are unions over every file. An edit in one file can set or clear one, and that changes the bytes of an unchanged unit (an R5 pair turning back into a GOT load, an IE use staying IE, a function's address becoming its PLT entry), which the index as written does not track. Patches: arch.md §6, incremental invalidation. |

## 10. Requested changes to `spec/diag.md`

Three kinds, in a new area "relocations" between "symbols" and
"refusals" (diag.md §3.3). `undefined-symbol` belongs to the symbols
area (`spec/resolve.md`); this spec fixes only that scan emits it at
phase 70 (§3.4).

| id | variant | message |
|---|---|---|
| `relocation-incompatible` | `RelocationIncompatible { r_type, name, reason }` | `relocation {r_type} cannot be used against symbol '{name}'; {reason}` |
| `relocation-misaligned` | `RelocationMisaligned { r_type, value, align }` | `improper alignment for relocation {r_type}: {value} is not aligned to {align} bytes` |
| `relocation-out-of-range` | `RelocationOutOfRange { r_type, value, min, max, name }` | `relocation truncated to fit: {r_type} against '{name}': {value} is not in [{min}, {max}]` |

`{reason}` is one of: `recompile with -fPIC`; `recompile with -fPIE`;
`it is in a read-only section` (a text relocation); `the addend must be
0`; `it is not thread-local`; `it is thread-local`; `local-exec TLS in a
shared object`; `the instruction does not match the relocation`; `not
allowed in a non-allocated section`. Wording after lld (the alignment text is lld's; the
out-of-range text is GNU ld's phrase with lld's value and bounds, `spec/x64.md` X-34; the
incompatible text is lld's shape with a reason field).
`name` goes through the demangling hook.

## 11. Open questions

| # | question | recommendation |
|---|---|---|
| Q1 | The class table (§4.3) is written from AArch64 and checked against x86 on paper (Appendices). x64.md and x86.md may find a case the table does not express. | Their WPs may add a class; changing an existing row needs this spec's review. |
| Q2 | `R_max = 32` is part of the fixed point's definition (§6), and its value is a choice: a link that needs more is refused. Termination is proved only under policy condition (C). | Keep 32; `spec/layout.md` adopts (C) so that the cap is never reached for a terminating policy; the corpus test measures the actual maximum, expected ≤ 3. |
| Q3 | `--emit-relocs` on REL outputs: GNU ld leaves the final value in the field, so the emitted addend is lost. | Do the same; consumers of `--emit-relocs` (BOLT, post-link tools) target RELA platforms. Confirm in x86.md. |
| Q4 | Keeping the GOT slot of a relaxed GOT load (AArch64 R5, x86 `GOTPCRELX`) costs one word per symbol; lld drops it when every use is relaxed. | Keep (conservative demand, model §3.7); measure in the bench and revisit with an address-independent criterion if it matters. |
| Q5 | Thunk reuse picks "the existing thunk with the lowest address in reach". | Keep (any deterministic rule is correct); the layout policy may prefer the nearest, if measured to need fewer rounds. |
| Q6 | x86 TLSLD sequences that match none of the recognized forms (Appendix A.4: `call`, `call *GOTPCRELX` and the large-model `movabs; add; call *%rax`, i386 `PLT32` and `GOT32X`), in an executable, are refused, because their `DTPOFF` fields would otherwise need per-sequence knowledge. | Keep; GCC and Clang emit only the recognized forms (the `-fno-plt` forms are in A.4 after the review). |
| Q7 | The x86 TLS special cases (A.4, A.6: `_TLS_MODULE_BASE_` under TLSDESC) are from the review's experiment with clang `-mtls-dialect=gnu2`; GCC's gnu2 local-dynamic output was not inspected. | Answered (`spec/x64.md` X-28, X-47): GCC 14 and Clang 22 both use `_TLS_MODULE_BASE_` under `-mtls-dialect=gnu2 -ftls-model=local-dynamic`; both run with GNU ld's and lld's convention. |
| Q8 | `undefined-symbol` from a non-alloc place (§3.4) follows GNU ld and mold; lld 22 accepts it. | Keep (one experiment, 2026-11-14: `.xword undef_sym` in `.debug_info`). |

## 12. Tests

- §3.2: `classify` of every type number 0..=0xFFFF of each
  architecture: a `Howto` or a refusal with the right name; no panic.
- §3.4/§4.3: table-driven: class × output kind × target kind gives the
  table's demands or error, per architecture.
- §3.5: sequences evaluated from each member give the same verdict;
  removing a follower turns the sequence off.
- §3.6: fill each section's relocations in reverse and shuffled order;
  bytes equal (with model §11's permuted-unit test).
- §5.2: every corpus object linked in every output kind with
  `debug_assert` on missing slots; zero hits.
- §6: a synthetic layout needing thunks in two rounds; monotone growth;
  the result equal under thread counts 1, 2, 7, 64; the cap refusal on a
  policy injected to oscillate.
- §7: each diagnostic's exact text and key.
- §2.3: the two i386 wrap-around links are accepted; a 64-bit value just
  outside `Signed(32)` is refused.
- §3.4: an unmatched x86-64 `TLSGD`+call is not an overlap; two
  matched sequences sharing a byte are.
- §4.2/M-8: an incremental link that adds an unpaired GOT use in one file
  refills the R5 pairs of another; likewise `TLSIE_NORELAX`.
- §4.3: a `-g` PIE produces no dynamic relocation in `.debug_*`.
- §3.4: `g++ -static` objects with a consumed `__tls_get_addr` call and no
  definition link; an unconsumed call is `undefined-symbol` with the
  phase-70 key; a hidden undefined with `--unresolved-symbols=ignore-all`
  is still an error.
- §6: a round that adds no patch is followed by a stopping round; the
  alignment counterexample; an erratum patch pushed out of range is
  re-patched.
- §3.6: every clip of every unit writes the unclipped bytes inside it
  (per architecture, every type and sequence); §3.1 `max_reach` equals
  the maximum of the derived windows.
- §3.4: each `EventClass` reported once per surviving reference, none
  for a consumed record; §3.11 the tombstone by name and class.
- Appendices: the fit checks become x64.md's and x86.md's first tests.

## Appendix A. Fit check: x86-64

Types are `R_X86_64_*`, numbers from the SysV x86-64 psABI. "Fits" means
expressible by §2-§3 with no change to the trait as it now stands; the
changes it forced are in Appendix C.

Grounding: the psABIs, plus one experiment (`landed/arch/WP-013.md`) with
the container's x86-64 and i686 cross GCC 14, GNU ld 2.44 and lld 19. It
confirmed byte for byte:

- A.3's and B.5's GD → LE words;
- A.2's `REX_GOTPCRELX` `mov` → `lea` (lld) and → `mov $imm` (GNU ld,
  non-PIE);
- A.8's IBT `.plt`/`.plt.sec`;
- B.4's two i386 PLTs, including the `push 8n`.

The other items rest on the psABIs alone.

**A.1 `PLT32` (4), `PC32` (2).** `PLT32`: class `Branch` (no veneer:
`branch().veneer = false`, `thunk()` is `None`), `X = L + A − P`, `Data{4}`,
`Signed(32)`. Overflow is §7's error (no thunk exists on x86).
`PC32`: class `Pc`, `X = S + A − P`; against `Shlib` in `Exec` it is
`C` (copy relocation, canonical PLT); in `Pie` it is `C` (a copy) against
shared data and `E` against a shared function (`spec/dyn.md` D-43,
D-44). Fits.

**A.2 `GOTPCREL` (9), `GOTPCRELX` (41), `REX_GOTPCRELX` (42).** Class
`Got`; `X = G(Addr) + A − P` (`add_a`: the addend, typically −4, applies
outside the slot, `AddendRule::Any`). Relaxations:

- `mov foo@GOTPCREL(%rip), %reg` (`GOTPCRELX` `8b`, or `REX_GOTPCRELX`
  `48|4c 8b`, ModRM `00 reg 101`) → `lea foo(%rip), %reg`: the byte at
  `P − 2` becomes `8d` and the field `S + A − P`, in every output kind
  (GNU ld's `mov $foo` is not taken). Window `before: 2` (`3` for
  `REX_`).
- `call *foo@GOTPCREL(%rip)` (`ff 15`) → `addr32 call foo` (`67 e8`),
  the rel32 staying at `P`; `jmp *foo@GOTPCREL(%rip)` (`ff 25`) → `jmp
  foo; nop` (`e9 <rel32> 90`), the rel32 moving to `P − 1` and holding
  `S + A − P + 1`. Window `before: 2`.
- In `Static` and `Exec` only (the non-PIE kinds, whose `S` does not
  move: `spec/x64.md` X-20, `spec/dyn.md` D-46), `REX_GOTPCRELX` binary
  operations (`03 0b 13 1b 23 2b 33 3b`) and `test` (`85`) → `81 /n
  imm32` and `f7 /0 imm32` (REX.R moved to REX.B) when `S` is in [0,
  2^31).

Every form needs a `Local` target that is not `Abs`, `Ifunc` or `Weak0`,
and `A = −4`; plain `GOTPCREL` is never relaxed (`spec/x64.md` §8.1).
This is the one scope of x86-64 GOT relaxation, in every output kind
(`spec/dyn.md` D-46, request R-A7; x64.md X-19, X-20 [M2 for `Exec` and
`Pie`]): a key that is not `Local` (`Preemptible`, `Shlib`, `WeakDyn`)
keeps its slot and its record.

The precondition has an address part (`S + A − P` in `Signed(32)`; the
absolute value in range), so the rule is kind 2 of §5.2: the GOT slot
stays demanded. Single-relocation rewrites: fits with the window.

**A.3 `TLSGD` (19).** The sequence is `66 48 8d 3d <TLSGD>` (`data16 lea
x@tlsgd(%rip), %rdi`) followed by the call. The call is `66 66 48 e8
<PLT32 __tls_get_addr>`, or with `-fno-plt` `66 48 ff 15 <GOTPCRELX
__tls_get_addr>`; the follower types are `PLT32` or `PC32` after `e8`,
`GOTPCRELX` or `GOTPCREL` after `ff 15`, `PLTOFF64` after `48 b8` (GCC 14 `-fPIC -fno-plt
-Wa,-mrelax-relocations=no` writes `GOTPCREL`; `spec/x64.md` X-25,
X-30). The lead is the `TLSGD`, and the follower is the call
relocation at record `+1`. In both forms the follower's field is at
`P + 8`. GCC 14's `-fno-plt` output was checked: the lead is at 0x8 and
the follower at 0x10. Both forms are 16 bytes, `[P − 4, P + 12)`.
In an executable: LE `64 48 8b 04 25 00 00 00 00 48 8d 80 <tpoff32>`
(`mov %fs:0, %rax; lea x@tpoff(%rax), %rax`) over `[P − 4, P + 12)`,
written entirely by the lead (window `before: 4, after: 8`); IE `mov
%fs:0, %rax; add x@gottpoff(%rip), %rax`. The follower writes nothing when
the pattern holds and raises no `PLT` demand (input-only, §3.5). Fits;
this case and AArch64 R4 are why §3.5 exists.

A third form is GCC's `-fPIC -mcmodel=large`: `48 8d 3d <TLSGD>; 48 b8
<PLTOFF64 __tls_get_addr at P + 6>; (48|4c) 01 (c0 | r << 3); ff d0` (22
bytes, `[P − 3, P + 19)`), relaxed to `64 48 8b 04 25 00 00 00 00; 48 8d
80 <tpoff32>; 66 0f 1f 44 00 00` (window `before: 3, after: 15`;
`spec/x64.md` §8.3).

**A.4 `TLSLD` (20) with `DTPOFF32` (21).** There are three forms, each with
its own window.

- `48 8d 3d <TLSLD>; e8 <PLT32 __tls_get_addr>` (12 bytes, follower field
  at `P + 5`) becomes `66 66 66 64 48 8b 04 25 00 00 00 00` in an
  executable.
- The `-fno-plt` form `48 8d 3d <TLSLD>; ff 15 <GOTPCRELX
  __tls_get_addr>` (13 bytes, follower field at `P + 6`) becomes `66 66
  66 66 64 48 8b 04 25 00 00 00 00`.
- the 22-byte large-model form (A.3's, with `TLSLD`) becomes `66 66 66 66 2e 0f 1f
  84 00 00 00 00 00 64 48 8b 04 25 00 00 00 00` (`spec/x64.md` §8.4).

The follower types are `PLT32` or `PC32` after `e8`, `GOTPCRELX` or
`GOTPCREL` after `ff 15`, `PLTOFF64` after `48 b8` (`spec/x64.md` X-27, X-30).

The first two were checked with GCC 14, GNU ld 2.44 and lld 19, which write the
same words. The `DTPOFF32` fields that follow use the module base the
call returned: once the sequence is relaxed they must hold `TpRel`
instead of `DtpRel`. They are not linked to a particular `TLSLD`, so the
rule is per output: in an executable every `TLSLD` is relaxed and every
`DTPOFF32`/`DTPOFF64` in code is written as `TpRel` (a kind-1 rule: output
kind only).

**The `_TLS_MODULE_BASE_` special case (x86-64 TLSDESC
local-dynamic).** Clang's `-mtls-dialect=gnu2 -ftls-model=local-dynamic`
takes a descriptor for `_TLS_MODULE_BASE_` and adds `DTPOFF32` offsets
to its result. In an executable, A.6's LE rewrite of a descriptor for
`_TLS_MODULE_BASE_` writes `0` (`mov $0x0, %rax`), **not** the TP offset
of the symbol. The `DTPOFF32` fields hold the full `TpRel`, as with
`TLSLD`. The sum is `0 + TpRel(x)`.

The review's experiment ran this: GNU ld and lld write exactly this
(`mov $0, %rax`, then `%fs:-0x80`), and the program printed the expected
value. Writing `TpRel(_TLS_MODULE_BASE_)` there would count the offset
twice. This is the opposite of AArch64, where the base's descriptor
becomes `TPREL(tls.start)` and DTPREL fields keep `DtpRel` (a64.md
§12.4). Each architecture spec states its convention. A `TLSLD` that does not match the pattern in an executable is
refused by name (feature `unrecognized local-dynamic TLS sequence`,
§11 Q6). Fits; no trait change.

**A.5 `GOTTPOFF` (22).** `mov x@gottpoff(%rip), %reg` (`48|4c 8b`) → `mov
$tpoff, %reg` (`REX' c7 c0+r`, REX.R → REX.B); `add x@gottpoff(%rip), %reg`
(`48|4c 03`) → `add $tpoff, %reg` (`REX' 81 c0+r`; `spec/x64.md` §8.2). Window `before: 3`. Kind 1
(target `Local` in an executable). Fits.

**A.6 TLSDESC: `GOTPC32_TLSDESC` (34), `TLSDESC_CALL` (35).** `lea
x@tlsdesc(%rip), %reg` (`48|4c 8d`, ModRM `00 reg 101`; GCC also uses
`%rbp` and `%rbx`) → LE `mov $tpoff, %reg` (`REX' c7 c0+reg`, REX.R moved
to REX.B; `48 c7 c0` for `%rax`, `spec/x64.md` §8.5),
IE `mov x@gottpoff(%rip), %rax` (`48 8b 05`); `call *x@tlscall(%rax)`
(`ff 10`, a marker: `Field::None`, window `after: 2`) → `xchg %ax, %ax`
(`66 90`). Members rewrite independently, as AArch64 R1. The LE value is
`TpRel(S + A)`, except for `_TLS_MODULE_BASE_`, which gives 0 (A.4). Fits.

**A.7 `32S` (11), `32` (10) in a PIE.** Class `AbsNarrow`: `E` in `Pie`
and `Shared` (`recompile with -fPIE`) except against `Abs`; checks
`Signed(32)` and `Unsigned(32)`. Fits.

**A.8 IBT PLT.** With `X86_FEATURE_1_IBT` set in the merged property (an
`And` merge) the PLT is two chunks: `.plt` (header `ff 35 <GOT+8>; ff 25
<GOT+16>; 0f 1f 40 00`; lazy entries `f3 0f 1e fa; 68 <n>; e9 <PLT0>; 66
90`) and `.plt.sec` (`f3 0f 1e fa; ff 25 <.got.plt[n+3]>; 66 0f 1f 44 00
00`). The symbol's PLT address `L` is its `.plt.sec` entry
(`call_chunk`); its `.got.plt` word initially points to the `.plt` lazy
entry (`lazy_chunk`). The lazy entry pushes the `.rela.plt` index
(`SynthCx`). Fits only with the chunk list (Appendix C, 4).

**A.9 `SIZE64` (33), `SIZE32` (32).** `X = Z + A` with `Z` the
definition's `st_size`: `Base::Size`, class `Size` (no demand; against a
`Preemptible` or `WeakDyn` target `E`, since the size may change at load
time). Fits with the `Size` base (Appendix C, 5).

**A.10 Large-model types.** `64` (1): `AbsWord`. `PC64` (24): `Pc`.
`GOTOFF64` (25): `S + A − GOT`, `GotRel`. `GOTPC32` (26), `GOTPC64`
(29): `GOT + A − P`, `GotBaseRef`. `GOT64` (27): `G + A − GOT` (the psABI's
`G` is the slot's offset), `Got`. `GOTPCREL64` (28): `G + A − P`, `Got`.
`GOTPLT64` (30): the `.got.plt` word of the PLT entry, `G(PltGot) + A −
GOT`, class `Got` plus `PLT`. `PLTOFF64` (31): `L + A − GOT`, `Branch`
without veneer, `Minus::GotBase`. The GOT base is `.got.plt`'s start
(`Facts.got_base`, Appendix C, 6). Also `16`, `PC16`, `8`, `PC8`:
`Data{2|1}` with `Either`/`Signed` checks. Fits.

**A.11 TLS layout.** Variant II: `TpRel(x) = x − tls.start −
align_up(tls.size, tls.align)`; `TPOFF32` (23) `Signed(32)`; `DTPOFF64`
(17) in debug sections. Fits with variant II (Appendix C, 8).

## Appendix B. Fit check: i386

Types are `R_386_*`, numbers from the i386 psABI; relocation sections are
`SHT_REL`, addends implicit.

The normative i386 rules are `spec/x86.md` §3–§9 (REL addends, fields
and checks, the table, undefined weak, `GOT32`/`GOT32X`, TLS); this
appendix is the fit check that shaped the trait, and where it and
x86.md differ, x86.md holds. The facts the first draft got wrong are
corrected here (x86.md R-1):

**B.1 REL implicit addends.** `32` (1), `PC32` (2), `GOT32` (3), `PLT32`
(4), `GOTOFF` (9), `GOTPC` (10), the TLS types: `Implicit { bytes: 4,
signed: true, shift: 0 }`; `16` (20), `PC16` (21): `bytes: 2`; `8` (22),
`PC8` (23): `bytes: 1`. `read_addend` sign-extends the field;
`write_addend` writes the output addend for `-r` (checked against the
width) and the final value is written for `--emit-relocs` (§3.3). Fits.

**B.2 `GOT32` (3), `GOT32X` (43).** `X = G + A − GOT` when the instruction
has a base register, `X = G + A` (absolute slot address, `Static`/`Exec`
only) when it has none. The base test is x86.md X86-13's: the ModRM byte
at `P − 1` (`(b & 0xc7) == 5`: no base), plus the SIB check, which
refuses a SIB `0d` at `P − 1` that the ModRM byte alone misreads
(x86.md §15.2 W1); and the place must be executable. `classify` returns
`Minus::GotBase` or `Minus::None`. In static links GNU ld relaxes
`GOT32X` `mov` to `mov $imm` (`c7 c0+r`), never to `lea`, with base and
without (x86.md X86-25), and `call *`/`jmp *` and the binary operations
as x86.md §8 states; lld relaxes nothing. Fits only because `classify`
sees the bytes (Appendix C, 1).

**B.3 `GOTOFF` (9), `GOTPC` (10).** `GOTOFF`: `S + A − GOT`, `GotRel`;
`GOTPC`: `GOT + A − P`, `GotBaseRef`. The GOT base is `.got.plt`'s start.
Fits.

**B.4 PIC and non-PIC PLT.** `plt_variant` gives `Abs` in `Static`/`Exec`
(header `ff 35 <abs GOT+4>; ff 25 <abs GOT+8>; 00 00 00 00`; entry `ff 25
<abs .got.plt[n+3]>; 68 <8n>; e9 <PLT0>`) and `Pic` in `Pie`/`Shared`
(header `ff b3 04 00 00 00; ff a3 08 00 00 00; 00 00 00 00`; entry `ff a3
<.got.plt[n+3] − GOT>; 68 <8n>; e9 <PLT0>`), each 16 bytes; and the IBT
forms (`endbr32` `f3 0f 1e fb`, two chunks as A.8). The push operand is
the byte offset of the `JUMP_SLOT` in `.rel.plt` (8 per entry), from the
index in `SynthCx`. `PLT32` is `Branch`, `X = L + A − P`. Fits with a
per-output-kind variant (Appendix C, 4). The IPLT entries of static
links are x86.md §11.3's.

**B.5 The TLS family.** `TLS_GD` (18): `leal x@tlsgd(,%ebx,1), %eax`
(`8d 04 1d`) or `leal x@tlsgd(%reg), %eax` (`8d 80+r`) followed by `call
___tls_get_addr@PLT` (`PLT32`) or `call *___tls_get_addr@GOT(%reg)`
(`GOT32X`): a sequence with the call at record `+1`. The matched windows
are 12 bytes (the SIB `lea` + `call`, the base `lea` + `call *`, and the
base `lea` + `call` + `nop`); the base-register GD with a bare `call`
does not exist (x86.md X86-34, X86-36). `TLS_LDM` (19) with
`TLS_LDO_32` (32): windows 11 or 12. The rewritten words per form are
x86.md §9's. The IE forms: `TLS_IE` (15): `G(TpOff{negated: false})`,
absolute; `a1` → `b8`, `8b` → `c7 c0+r`, `03` → `81 c0+r`.
`TLS_GOTIE` (16): `G(TpOff{false}) − GOT`. `TLS_IE_32` (33):
`G(TpOff{negated: true}) − GOT`. `TLS_LE` (17): `TpRel`; `TLS_LE_32`
(34): `TpRel` with `negate`. `TLS_GOTDESC` (39) and `TLS_DESC_CALL` (40):
the LE rewrite of the descriptor load is `8d 05 T` (`lea T, %eax`), not
`c7`, and `ff 10` → `66 90` (x86.md X86-32, X86-37). Dynamic:
`TLS_TPOFF` (14, `TpOff{false}`), `TLS_TPOFF32` (37, `TpOff{true}`),
`TLS_DTPMOD32` (35), `TLS_DTPOFF32` (36), `TLS_DESC` (41).
`TlsAbi.get_addr` is `___tls_get_addr`. Fits with the negated slot and
value (Appendix C, 5).

**B.6 `16` (20), `8` (22), `PC16` (21), `PC8` (23).** `Data{2}`/`Data{1}`.
`16` and `8` are `su(16)`/`su(8)` (`Either`); `PC16`/`PC8`, `X = S + A −
P`, are `s(16)`/`s(8)` (`Signed`); overflow is an error (x86.md X86-11).
The word-size types have no check (X86-10). Implicit 16- and 8-bit
addends, sign-extended (§3.3). Fits.

## Appendix C. What the fit check changed

The trait as first drafted from AArch64 alone was changed in these
places; each change is in §2-§3 above.

1. `classify` takes the section bytes (`Site`): i386 `GOT32`/`GOT32X`
   select the formula from the ModRM byte.
2. `Howto.window`: x86 rewrites change opcode bytes before the field,
   and `jmp *GOT` → `jmp` moves the field. After the review: the window
   is per matched form (i386 `TLS_GD`'s four lengths, x86-64 `TLSLD`'s
   two), and the overlap check exempts a sequence's own members.
3. Sequences with neighboring records (§3.5): x86 `TLSGD`/`TLSLD` with
   their call. AArch64 needed them too (R4, R5), but only the x86 cases
   fixed the look-back bound and the "lead writes the whole window" rule.
4. `PltShape` as a list of chunks with a call chunk and a lazy chunk, and
   `plt_variant` as a function of the output kind: x86 IBT's `.plt.sec`,
   i386's PIC and non-PIC PLTs.
5. `Base::Size` (`SIZE32`/`SIZE64`); `Value.negate` and `SlotKind::TpOff
   { negated }` (i386 `TLS_LE_32`, `TLS_IE_32`, `TLS_TPOFF32`).
6. `Facts.got_base`: the GOT base is `.got`'s start on AArch64 and
   `.got.plt`'s on x86.
7. `read_addend`/`write_addend` with `Implicit` widths 4, 2 and 1.
8. TLS variant II.
9. `property_merge` with an `Or` rule (x86 `ISA_1_NEEDED`).
10. `weak0` per architecture (AArch64 branches to the next instruction;
    x86 uses `S = 0`).
11. After the review: checks modulo 2^32 for ELFCLASS32 (§2.3); the
    non-alloc rule (§4.3); the `_TLS_MODULE_BASE_` convention as an
    architecture fact (A.4).

Nothing in the AArch64 table had to change for x86. The rule "every
`DTPOFF` in an executable is a `TpRel`" (A.4) is an x86 rule inside
`apply`, not a trait change.
