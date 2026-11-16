#!/usr/bin/env python3
"""The split-DWARF census (WP-080 stage A item 1; the record is
spec/landed/image/WP-080.md).

    split-census.py --work DIR --mortar BIN [--xtask BIN] [--arch a,b]
                    [--producer p,q] [--form f,g] [-j N] [--tsv OUT]

For each producer (GCC 14.2, Clang 19, Clang 22, kiln++ with hod on an
AArch64 host), each split-DWARF form (FORMS) and each architecture
(aarch64, x86_64, i386), the script compiles a two-file C++ program (a
COMDAT function in both files, a thread_local, a function only
--gc-sections collects), lists the debug sections each relocatable object
carries (name, flags, size, the relocation types against it), and links
the objects with GNU ld, lld 22, mold and mortar through the GCC driver of
the architecture (`-B<wrapper>/`, `-fno-use-linker-plugin`), static
non-PIE, plain and with --gc-sections. Each output's debug sections are
listed and each input section is classed:

    kept     in the output, as many bytes as the inputs bring
    merged   in the output, fewer bytes (string merging)
    dropped  not in the output
    refused  the link failed (its first diagnostic is recorded)

The program is run (natively or under qemu) and its output compared with
GNU ld's; mortar's output goes through the verifier (`xtask plumb`) when
--xtask is given. Results: one TSV row per (producer, form, arch, linker,
mode, section) and a summary on standard output. Everything is written
under DIR (a fresh directory per unit); the caller removes DIR.
"""

import argparse
import concurrent.futures as cf
import os
import re
import shutil
import subprocess
import sys

ARCHES = {
    "aarch64": {"triple": "aarch64-linux-gnu", "qemu": None, "elf32": False},
    "x86_64": {"triple": "x86_64-linux-gnu", "qemu": "qemu-x86_64", "elf32": False},
    "i386": {"triple": "i686-linux-gnu", "qemu": "qemu-i386", "elf32": True},
}

# (name, flags). `-gsplit-dwarf` alone is DWARF 5 for all four producers.
FORMS = [
    ("d5", ["-gsplit-dwarf", "-gdwarf-5"]),
    ("d4", ["-gsplit-dwarf", "-gdwarf-4"]),
    ("single", ["-gsplit-dwarf=single", "-gdwarf-5"]),
    ("single-d4", ["-gsplit-dwarf=single", "-gdwarf-4"]),
    ("d5-gnu-pubnames", ["-gsplit-dwarf", "-gdwarf-5", "-ggnu-pubnames"]),
    ("d4-gnu-pubnames", ["-gsplit-dwarf", "-gdwarf-4", "-ggnu-pubnames"]),
    ("d5-no-pubnames", ["-gsplit-dwarf", "-gdwarf-5", "-gno-pubnames"]),
    ("d5-pubnames", ["-gsplit-dwarf", "-gdwarf-5", "-gpubnames"]),
    ("d5-types", ["-gsplit-dwarf", "-gdwarf-5", "-fdebug-types-section"]),
    ("d4-types", ["-gsplit-dwarf", "-gdwarf-4", "-fdebug-types-section"]),
    ("d5-O2", ["-gsplit-dwarf", "-gdwarf-5", "-O2"]),
    ("d4-O2", ["-gsplit-dwarf", "-gdwarf-4", "-O2"]),
    ("single-O2", ["-gsplit-dwarf=single", "-gdwarf-5", "-O2"]),
    ("single-types", ["-gsplit-dwarf=single", "-gdwarf-5", "-fdebug-types-section"]),
    ("single-d4-types", ["-gsplit-dwarf=single", "-gdwarf-4", "-fdebug-types-section"]),
]

LINKERS = ["bfd", "lld", "mold", "mortar"]

MAIN_CC = r"""#include <cstdio>
template <class T> __attribute__((noinline)) T twice(T x) { return x + x; }
struct Point { int x; double y; };
thread_local int tl_count = 5;
int g_total = 3;
int helper(int);
int main(int argc, char **) {
  Point p{argc, 2.5};
  tl_count += argc;
  std::printf("%d %d %d\n", twice(argc) + g_total, helper(argc), (int)p.y + tl_count);
  return 0;
}
"""

B_CC = r"""template <class T> __attribute__((noinline)) T twice(T x) { return x + x; }
extern thread_local int tl_count;
__attribute__((noinline)) int unused_fn(int x) { return x * 9 + tl_count; }
__attribute__((noinline)) int helper(int x) { return twice(x + 1) + tl_count; }
"""


def run(cmd, cwd, env=None, timeout=120):
    try:
        p = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True,
                           timeout=timeout, errors="replace")
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return -1, "", "timeout"


def producers(arch, host, kiln, hod):
    t = ARCHES[arch]["triple"]
    out = {}
    out["gcc"] = (["g++"] if arch == host else [f"{t}-g++"], None)
    for major in ("19", "22"):
        tgt = [] if arch == host else [f"--target={t}"]
        out[f"clang{major}"] = ([f"clang++-{major}"] + tgt, None)
    if arch == "aarch64" and host == "aarch64" and kiln and hod:
        env = dict(os.environ)
        env["PATH"] = os.path.dirname(hod) + ":" + env["PATH"]
        env.update({"KILN_BACKEND": "hod", "KILN_HOD": hod, "KILN_HOD_STRICT": "1"})
        out["kiln"] = ([kiln], env)
    return out


def readelf(arch, host):
    return "readelf" if arch == host else f"{ARCHES[arch]['triple']}-readelf"


SEC_RE = re.compile(r"^\s*\[\s*\d+\]\s+(\S+)\s+(\S+)\s+([0-9a-f]+)\s+([0-9a-f]+)\s+([0-9a-f]+)"
                    r"\s+([0-9a-f]+)\s+(.*?)\s+(\d+)\s+(\d+)\s+(\d+)\s*$")


def sections(path, re_tool):
    """{name: (type, flags, size)} of every section."""
    rc, out, _ = run([re_tool, "-SW", path], cwd=None)
    secs = {}
    for line in out.splitlines():
        m = SEC_RE.match(line)
        if m:
            name, typ, size, flags = m.group(1), m.group(2), int(m.group(5), 16), m.group(7)
            if name in secs:
                old = secs[name]
                secs[name] = (old[0], old[1], old[2] + size)
            else:
                secs[name] = (typ, flags.replace(" ", ""), size)
    return secs


def alloc_ranges(path, re_tool):
    """[(start, end)] of the allocated sections (flag A)."""
    rc, out, _ = run([re_tool, "-SW", path], cwd=None)
    res = []
    for line in out.splitlines():
        m = SEC_RE.match(line)
        if m and "A" in m.group(7):
            a, s = int(m.group(3), 16), int(m.group(5), 16)
            res.append((a, a + s))
    return res


def hexes(s):
    return [int(x, 16) for x in re.findall(r"0x([0-9a-fA-F]+)", s)]


def addresses(path, re_tool, elf32):
    """The address-holding debug sections of an output, as counts: for
    each section, `<entries>/<tombstones>/<wrong>`. A tombstone is the
    value of spec/image.md I-47 for the section (0 in .debug_addr and
    .debug_rnglists; 1 for both ends in .debug_ranges; an all-ones
    address with length 0 in .debug_aranges; all ones (ELFCLASS64) or 0
    (ELFCLASS32) in DW_LNE_set_address). A wrong entry is any other value
    outside the allocated sections: that is what another linker's
    tombstone looks like to this count."""
    ones = 0xFFFFFFFF if elf32 else 0xFFFFFFFFFFFFFFFF
    al = alloc_ranges(path, re_tool)

    def inside(a):
        return any(s <= a < e for s, e in al)

    counts = {}

    def add(sec, dead, ok):
        c = counts.setdefault(sec, [0, 0, 0])
        c[0] += 1
        c[1] += dead
        c[2] += (not dead and not ok)

    dd = "llvm-dwarfdump-22"
    _, out, _ = run([dd, "--debug-addr", path], cwd=None)
    for line in out.splitlines():
        if re.match(r"^0x[0-9a-f]+$", line.strip()):
            a = int(line.strip(), 16)
            add(".debug_addr", a == 0, inside(a))
    _, out, _ = run([dd, "--debug-aranges", path], cwd=None)
    for line in out.splitlines():
        if line.startswith("["):
            v = hexes(line)
            if len(v) == 2:
                add(".debug_aranges", v[0] == ones and v[1] == ones or v[0] == v[1],
                    inside(v[0]))
    _, out, _ = run([dd, "--debug-ranges", path], cwd=None)
    for line in out.splitlines():
        w = line.split()
        if len(w) == 3 and all(re.fullmatch(r"[0-9a-f]+", x) for x in w):
            a, b = int(w[1], 16), int(w[2], 16)
            add(".debug_ranges", a == 1 and b == 1, inside(a) or a == ones)
    _, out, _ = run([dd, "--debug-rnglists", "--verbose", path], cwd=None)
    for line in out.splitlines():
        m = re.search(r"\[(DW_RLE_\w+)\s*\]:(.*)", line)
        if m and m.group(1) in ("DW_RLE_start_length", "DW_RLE_start_end",
                                 "DW_RLE_base_address"):
            v = hexes(m.group(2).split("=>")[0])
            if v:
                add(".debug_rnglists", v[0] == 0, inside(v[0]))
    _, out, _ = run([dd, "--debug-line", "--verbose", path], cwd=None)
    for line in out.splitlines():
        if "DW_LNE_set_address" in line:
            v = hexes(line)
            if v:
                a = v[-1]
                add(".debug_line", a == (0 if elf32 else ones), inside(a))
    return " ".join(f"{k}={c[0]}/{c[1]}/{c[2]}" for k, c in sorted(counts.items()))


def debugish(name):
    return name.startswith(".debug") or name.endswith(".dwo") or name in (".gdb_index",)


def reloc_types(path, re_tool):
    """{target section: sorted relocation types} for the debug sections."""
    _, syms, _ = run([re_tool, "-sW", path], cwd=None)
    tls = {w[7] for w in (l.split() for l in syms.splitlines()) if len(w) >= 8 and w[3] == "TLS"}
    rc, out, _ = run([re_tool, "-rW", path], cwd=None)
    cur, res = None, {}
    for line in out.splitlines():
        m = re.match(r"Relocation section '\.rela?(\S+)'", line)
        if m:
            cur = m.group(1)
            continue
        if cur and debugish(cur):
            w = line.split()
            if len(w) >= 3 and w[0][:1] in "0123456789abcdef" and w[2].startswith("R_"):
                tgt = w[4] if len(w) > 4 else ""
                kind = w[2] + ("@tls" if tgt in tls else "")
                res.setdefault(cur, set()).add(kind)
    return {k: sorted(v) for k, v in res.items()}


def write_wrappers(work, arch, host, mortar):
    t = ARCHES[arch]["triple"]
    real = {
        "bfd": "ld.bfd" if arch == host else f"{t}-ld.bfd",
        "lld": "ld.lld-22",
        "mold": "mold",
        "mortar": mortar,
    }
    dirs = {}
    for ln, exe in real.items():
        d = os.path.join(work, "wrap", arch, ln)
        os.makedirs(d, exist_ok=True)
        p = os.path.join(d, "ld")
        with open(p, "w") as f:
            f.write(f'#!/bin/sh\nexec {shutil.which(exe) or exe} "$@"\n')
        os.chmod(p, 0o755)
        dirs[ln] = d
    return dirs


def unit(args, arch, prod, pcmd, penv, form, fflags, host, wrappers):
    """Compiles and links one (arch, producer, form): a list of rows."""
    d = os.path.join(args.work, arch, prod, form)
    os.makedirs(d, exist_ok=True)
    for name, text in (("main.cc", MAIN_CC), ("b.cc", B_CC)):
        with open(os.path.join(d, name), "w") as f:
            f.write(text)
    rows = []
    re_tool = readelf(arch, host)
    base_flags = ["-c", "-g", "-ffunction-sections", "-fdata-sections"]
    if "-O2" not in fflags:
        base_flags.append("-O0")
    for src in ("main.cc", "b.cc"):
        obj = src.replace(".cc", ".o")
        rc, _, err = run(pcmd + base_flags + fflags + [src, "-o", obj], cwd=d, env=penv)
        if rc != 0:
            first = (err.strip().splitlines() or ["?"])[0][:160]
            return [(arch, prod, form, "-", "-", "producer", "refused", first)]
    objsecs = {}
    for obj in ("main.o", "b.o"):
        secs = sections(os.path.join(d, obj), re_tool)
        rt = reloc_types(os.path.join(d, obj), re_tool)
        for n, (typ, flags, size) in secs.items():
            if debugish(n) and not n.startswith(".rela") and not n.startswith(".rel."):
                o = objsecs.setdefault(n, [typ, flags, 0, set()])
                o[2] += size
                o[3].update(rt.get(n, []))
    dwos = sorted(f for f in os.listdir(d) if f.endswith(".dwo"))
    for n, (typ, flags, size, rts) in sorted(objsecs.items()):
        rows.append((arch, prod, form, "-", "-", "object", n,
                     f"{typ} {flags or '-'} {size} rel={','.join(rts) or '-'}"))
    rows.append((arch, prod, form, "-", "-", "object", "<dwo files>", " ".join(dwos) or "-"))
    gcc = "g++" if arch == host else f"{ARCHES[arch]['triple']}-g++"
    stdout_ref = {}
    for mode in ("plain", "gc"):
        for ln in LINKERS:
            out = f"prog-{ln}-{mode}"
            cmd = [gcc, "-B" + wrappers[ln] + "/", "-fno-use-linker-plugin", "-static",
                   "-no-pie", "main.o", "b.o", "-o", out]
            if mode == "gc":
                cmd.insert(-2, "-Wl,--gc-sections")
            if ln == "mortar":
                cmd.insert(-2, f"-Wl,--mortar-map={out}.map")
            rc, _, err = run(cmd, cwd=d, timeout=300)
            if rc != 0:
                first = next((l for l in err.splitlines() if l.strip()), "?")[:200]
                rows.append((arch, prod, form, ln, mode, "link", "refused", first))
                continue
            o = os.path.join(d, out)
            secs = sections(o, re_tool)
            for n in sorted(set(objsecs) | {s for s in secs if debugish(s)}):
                insum = objsecs.get(n, [None, None, 0])[2]
                if n not in secs:
                    fate = "dropped"
                else:
                    sz = secs[n][2]
                    fate = "kept" if sz >= insum else "merged"
                    fate += f" {sz}"
                rows.append((arch, prod, form, ln, mode, "section", n, fate))
            q = ARCHES[arch]["qemu"]
            runner = [] if arch == host else [q, "-L", f"/usr/{ARCHES[arch]['triple']}"]
            rc, so, se = run(runner + ["./" + out, "x"], cwd=d, timeout=60)
            res = f"exit {rc} {so.strip()}"
            if ln == "bfd":
                stdout_ref[mode] = res
            same = "same" if stdout_ref.get(mode) == res else f"DIFFERS from bfd ({res})"
            rows.append((arch, prod, form, ln, mode, "run", "-", same))
            rows.append((arch, prod, form, ln, mode, "addresses", "-",
                         addresses(o, re_tool, ARCHES[arch]["elf32"])))
            if ln == "mortar" and args.xtask:
                rc, so, se = run([args.xtask, "plumb", out, "--map", out + ".map",
                                  "--map-format", "mortar"], cwd=d, timeout=300)
                verdict = "clean" if rc == 0 else "FINDINGS " + \
                    " | ".join((so + se).strip().splitlines()[:3])[:300]
                rows.append((arch, prod, form, ln, mode, "plumb", "-", verdict))
    return rows


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--work", required=True)
    ap.add_argument("--mortar", required=True)
    ap.add_argument("--xtask")
    ap.add_argument("--kiln")
    ap.add_argument("--hod")
    ap.add_argument("--arch", default="aarch64,x86_64,i386")
    ap.add_argument("--producer", default="gcc,clang19,clang22,kiln")
    ap.add_argument("--form", default=",".join(f for f, _ in FORMS))
    ap.add_argument("-j", type=int, default=4)
    ap.add_argument("--tsv")
    args = ap.parse_args()
    host = {"aarch64": "aarch64", "x86_64": "x86_64"}[os.uname().machine]
    args.mortar = os.path.abspath(args.mortar)
    os.makedirs(args.work, exist_ok=True)
    jobs = []
    with cf.ThreadPoolExecutor(args.j) as ex:
        for arch in args.arch.split(","):
            wrappers = write_wrappers(args.work, arch, host, args.mortar)
            prods = producers(arch, host, args.kiln, args.hod)
            for prod in args.producer.split(","):
                if prod not in prods:
                    continue
                pcmd, penv = prods[prod]
                for form, fflags in FORMS:
                    if form in args.form.split(","):
                        jobs.append(ex.submit(unit, args, arch, prod, pcmd, penv, form, fflags,
                                              host, wrappers))
        rows = []
        for j in jobs:
            rows.extend(j.result())
    rows.sort()
    text = "\n".join("\t".join(r) for r in rows) + "\n"
    if args.tsv:
        with open(args.tsv, "w") as f:
            f.write("# arch\tproducer\tform\tlinker\tmode\tkind\tsection\tvalue\n" + text)
    sys.stdout.write(text)


if __name__ == "__main__":
    main()
