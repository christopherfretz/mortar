# Part A ok/fail grid from the debugger transcripts. Usage: python3 -I check_a.py ADIR
import sys, os, re
A = sys.argv[1]
LK = ["bfd", "lld22", "lld19", "mold"]

def sect(t, start, end=None):
    i = t.find(start)
    if i < 0: return ""
    j = t.find(end, i + len(start)) if end else -1
    return t[i:] if j < 0 else t[i:j]

checks = {
  "gdb_session": [
    ("pending break libfn stops", lambda t: re.search(r"Breakpoint 1, libfn \(x=5\) at \S*foo\.c:7", t)),
    ("info sharedlibrary lists libfoo+libbar", lambda t: "libfoo.so" in sect(t, "==STOP1", "==STOP2") and "libbar.so" in sect(t, "==STOP1", "==STOP2")),
    ("bt exe->libfoo->libbar", lambda t: re.search(r"#0  bar_fn.*\n#1 .* in libfn \(x=5\).*foo\.c:7\n#2 .* in main \(\) at \S*host\.c:9", t)),
    ("finish returns 11", lambda t: re.search(r"Value returned is \$\d+ = 11", t)),
    ("print lib global 42", lambda t: re.search(r"\$1 = 42", t)),
    ("print lib __thread 7", lambda t: re.search(r"\$2 = 7", t)),
    ("pending break in dlopened lib", lambda t: re.search(r"Breakpoint 3, plugfn \(x=4\) at \S*plug\.c:5", t)),
    ("dlopened lib listed", lambda t: "libplug.so" in sect(t, "==STOP3")),
    ("print dlopened global 99", lambda t: re.search(r"\$\d+ = 99", t)),
    ("program OK", lambda t: "OK host-a" in t)],
  "gdb_after_dlopen": [
    ("break set after dlopen stops", lambda t: re.search(r"==STOP plugfn.*\n#0  plugfn \(x=4\)", t))],
  "gdb_step": [
    ("step exe->PLT->libfn", lambda t: re.search(r"==STEP\nlibfn \(x=5\) at \S*foo\.c:7", t)),
    ("step libfn->PLT->bar_fn", lambda t: re.search(r"\nbar_fn \(x=10\) at \S*bar\.c:4", t))],
  "gdb_cxx_line": [
    ("line break in C++ lib", lambda t: re.search(r"ns::Widget::get \(this=0x[0-9a-f]+\) at \S*cxx\.cpp:9", t)),
    ("print *this", lambda t: "{v = 4}" in t),
    ("print thread_local 4", lambda t: re.search(r"\$2 = 4", t)),
    ("finish returns 40", lambda t: re.search(r"Value returned is \$\d+ = 40", t))],
  "gdb_cxx": [
    ("break by name: PLT location in lib", lambda t: "get() const@plt" in t),
    ("by name reaches the function", lambda t: re.search(r"ns::Widget::get \(this=0x", t))],
  "lldb_session": [
    ("pending break libfn stops", lambda t: re.search(r"libfoo\.so`libfn\(x=5\) at foo\.c:7", t)),
    ("image list libfoo+libbar", lambda t: "libfoo.so" in sect(t, "==STOP1", "==STOP2") and "libbar.so" in sect(t, "==STOP1", "==STOP2")),
    ("bt exe->libfoo->libbar", lambda t: re.search(r"libbar\.so`bar_fn\(x=10\).*\n.*libfoo\.so`libfn\(x=5\) at foo\.c:7.*\n.*host`main at host\.c:9", t)),
    ("finish returns 11", lambda t: "Return value: (int) $0 = 11" in t),
    ("p lib global 42", lambda t: "(int) 42" in t),
    ("pending break in dlopened lib", lambda t: re.search(r"libplug\.so`plugfn\(x=4\) at plug\.c:5", t)),
    ("dlopened lib listed", lambda t: "libplug.so" in sect(t, "==STOP3")),
    ("p dlopened global 99", lambda t: "(int) 99" in t),
    ("program OK", lambda t: "OK host-a" in t)],
  "lldb_step": [("step-in exe->PLT->libfn", lambda t: re.search(r"stop reason = step in\n.*libfoo\.so`libfn\(x=5\) at foo\.c:7", t))],
  "lldb_cxx": [
    ("break in C++ lib", lambda t: "libcxx.so`ns::Widget::get" in t),
    ("p std::string global", lambda t: '(std::string) "widget"' in t),
    ("p *this right (v = 4)", lambda t: "(v = 4)" in t)],
  "lldb_tls": [("target variable __thread", lambda t: re.search(r"\(int\) foo_tls = 7", t))],
  "lldb_tls_cxx": [("target variable thread_local", lambda t: re.search(r"\(int\) ns::cxx_tls = 4", t))],
  "qgdb": [
    ("pending break libfn stops", lambda t: re.search(r"Breakpoint 1, libfn \(x=5\) at \S*foo\.c:7", t)),
    ("info sharedlibrary lists libfoo+libbar", lambda t: "libfoo.so" in sect(t, "==STOP1", "==STOP2") and "libbar.so" in sect(t, "==STOP1", "==STOP2")),
    ("bt exe->libfoo->libbar", lambda t: re.search(r"#0  bar_fn.*\n#1 .* in libfn \(x=5\).*foo\.c:7\n#2 .* in main \(\) at \S*host\.c:9", t)),
    ("finish returns 11", lambda t: re.search(r"Value returned is \$\d+ = 11", t)),
    ("print lib global 42", lambda t: re.search(r"\$1 = 42", t)),
    ("print lib __thread", lambda t: re.search(r"\$2 = 7", t)),
    ("pending break in dlopened lib", lambda t: re.search(r"Breakpoint 3, plugfn \(x=4\)", t)),
    ("dlopened lib listed", lambda t: "libplug.so" in sect(t, "==STOP3")),
    ("program OK", lambda t: "OK host-a" in t)],
  "qgdb_step": [("step exe->PLT->libfn", lambda t: re.search(r"==STEP\nlibfn \(x=5\) at \S*foo\.c:7", t))],
  "qgdb_cxx": [
    ("line break in C++ lib", lambda t: re.search(r"ns::Widget::get \(this=0x[0-9a-f]+\) at \S*cxx\.cpp:9", t)),
    ("print *this", lambda t: "{v = 4}" in t),
    ("print thread_local", lambda t: re.search(r"\$2 = 4", t))],
  "qgdb_cxxname": [("break by name: PLT location in lib", lambda t: "get() const@plt" in t)],
}
rows = []
for test, cs in checks.items():
    arch = "x64" if test.startswith("q") else "aa"
    for name, fn in cs:
        res = []
        for k in LK:
            p = os.path.join(A, f"{arch}.{k}.{test}.out")
            t = open(p, errors="replace").read() if os.path.exists(p) else ""
            res.append("ok" if fn(t) else "FAIL")
        rows.append((arch, test, name, res))
w = max(len(r[1]) + len(r[2]) for r in rows) + 3
print(f"{'arch':5}{'test: check':{w}}" + "".join(f"{k:>7}" for k in LK))
for arch, test, name, res in rows:
    print(f"{arch:5}{test + ': ' + name:{w}}" + "".join(f"{r:>7}" for r in res))
