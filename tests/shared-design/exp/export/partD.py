# Part D (and E): relocation classes in a -shared output from non-PIC code. One assembly object per
# (relocation form, target kind); the library is dlopen'ed by a host that checks the probe's result.
import sys, os, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *

work = init(sys.argv[1], 'D')
S = open(os.path.join(work, 'summary.txt'), 'w')
def out(s=''):
    S.write(s + '\n'); S.flush()

write('d_defs.c', r'''
long hid_d __attribute__((visibility("hidden"))) = 0x22;
long pre_d = 0x33;
__attribute__((visibility("hidden"))) long hid_fn(void){return 0x22;}
long pre_fn(void){return 0x33;}
__thread long tv = 0x55;
void *E_hid_d(void){return &hid_d;}
void *E_hid_fn(void){return (void*)hid_fn;}
''')
write('d_dep.c', 'long imp_d = 0x44; long imp_fn(void){return 0x44;}\n')
write('d_host.c', r'''
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
#include <string.h>
#ifdef INTERPOSE
long pre_d = 0x99; long pre_fn(void){return 0x99;}
#endif
typedef long (*fn)(void);
int main(int argc, char **argv){
  const char *mode = argv[2], *kind = argv[3];
  void *h = dlopen(argv[1], RTLD_NOW);
  if(!h){ printf("DLOPEN-FAIL %s\n", dlerror()); return 2; }
  fn probe = (fn)dlsym(h, "probe");
  Dl_info di; dladdr((void*)probe, &di);
  long exp = 0;
  int isfn = !strcmp(mode, "call");
  if(!strcmp(mode, "addr")){
    if(!strcmp(kind,"loc")) exp = (long)((void*(*)(void))dlsym(h, argv[4] && !strcmp(argv[4],"fn") ? "E_loc_fn" : "E_loc_d"))();
    else if(!strcmp(kind,"hid")) exp = (long)((void*(*)(void))dlsym(h, argv[4] && !strcmp(argv[4],"fn") ? "E_hid_fn" : "E_hid_d"))();
    else if(!strcmp(kind,"pre")) {
#ifdef INTERPOSE
      exp = (argv[4] && !strcmp(argv[4],"fn")) ? (long)pre_fn : (long)&pre_d;
#else
      exp = (long)dlsym(h, (argv[4] && !strcmp(argv[4],"fn")) ? "pre_fn" : "pre_d");
#endif
    }
    else if(!strcmp(kind,"imp")) exp = (long)dlsym(h, (argv[4] && !strcmp(argv[4],"fn")) ? "imp_fn" : "imp_d");
    else exp = 0;
  } else {
    if(!strcmp(kind,"loc")) exp = 0x11; else if(!strcmp(kind,"hid")) exp = 0x22;
    else if(!strcmp(kind,"pre")) {
#ifdef INTERPOSE
      exp = 0x99;
#else
      exp = 0x33;
#endif
    } else if(!strcmp(kind,"imp")) exp = 0x44; else if(!strcmp(kind,"tls")) exp = 0x55; else if(!strcmp(kind,"stls")) exp = 0x66;
  }
  long got = probe();
  printf("%s got=%#lx exp=%#lx base=%#lx\n", got == exp ? "OK" : "FAIL", got, exp, (long)di.dli_fbase);
  return got == exp ? 0 : 1;
}
''')

SYM = {('loc', 'd'): 'loc_d', ('loc', 'fn'): 'loc_fn', ('hid', 'd'): 'hid_d', ('hid', 'fn'): 'hid_fn',
       ('pre', 'd'): 'pre_d', ('pre', 'fn'): 'pre_fn', ('imp', 'd'): 'imp_d', ('imp', 'fn'): 'imp_fn',
       ('wk', 'd'): 'wk_d', ('wk', 'fn'): 'wk_fn'}
KINDS = ['loc', 'hid', 'pre', 'imp', 'wk']

# --- per-arch assembly: (form name, mode, target class, asm-flags, body); body uses SYM.
A64_COMMON = '''
  .data
  .p2align 3
loc_d: .xword 0x11
  .text
  .p2align 2
loc_fn: mov x0, #0x11
  ret
  .globl E_loc_d
  .type E_loc_d,%function
E_loc_d: adrp x0, loc_d
  add x0, x0, :lo12:loc_d
  ret
  .globl E_loc_fn
  .type E_loc_fn,%function
E_loc_fn: adrp x0, loc_fn
  add x0, x0, :lo12:loc_fn
  ret
  .globl probe
  .type probe,%function
'''
def a64_word(sec, flags, directive, load):
    return ('''
  .section %s,"%s"
  .p2align 3
w: %s
  .text
probe: adrp x1, w
  add x1, x1, :lo12:w
  %s
  ret
''' % (sec, flags, directive, load))
A64 = [
    ('ADR_PREL_PG_HI21+ADD_ABS_LO12_NC', 'addr', 'd', [], 'probe: adrp x0, SYM\n add x0, x0, :lo12:SYM\n ret\n'),
    ('ADR_PREL_PG_HI21+LDST64_ABS_LO12_NC', 'val', 'd', [], 'probe: adrp x0, SYM\n ldr x0, [x0, :lo12:SYM]\n ret\n'),
    ('ADR_PREL_LO21', 'addr', 'd', [], 'probe: adr x0, SYM\n ret\n'),
    ('LD_PREL_LO19', 'val', 'd', [], 'probe: ldr x0, SYM\n ret\n'),
    ('MOVW_UABS_G0..G3', 'addr', 'd', [], 'probe: movz x0, #:abs_g3:SYM\n movk x0, #:abs_g2_nc:SYM\n movk x0, #:abs_g1_nc:SYM\n movk x0, #:abs_g0_nc:SYM\n ret\n'),
    ('ADR_GOT_PAGE+LD64_GOT_LO12_NC(control)', 'addr', 'd', [], 'probe: adrp x0, :got:SYM\n ldr x0, [x0, :got_lo12:SYM]\n ret\n'),
    ('CALL26', 'call', 'fn', [], 'probe: stp x29, x30, [sp, -16]!\n bl SYM\n ldp x29, x30, [sp], 16\n ret\n'),
    ('JUMP26', 'call', 'fn', [], 'probe: b SYM\n'),
    ('ABS64@.data', 'addr', 'd', [], a64_word('.data', 'aw', '.xword SYM', 'ldr x0, [x1]')),
    ('ABS64@.data.rel.ro', 'addr', 'd', [], a64_word('.data.rel.ro', 'aw', '.xword SYM', 'ldr x0, [x1]')),
    ('ABS64@.rodata', 'addr', 'd', [], a64_word('.rodata', 'a', '.xword SYM', 'ldr x0, [x1]')),
    ('ABS32@.data', 'addr', 'd', [], a64_word('.data', 'aw', '.word SYM', 'ldr w0, [x1]')),
    ('PREL32@.data', 'addr', 'd', [], a64_word('.data', 'aw', '.word SYM - .', 'ldrsw x0, [x1]\n add x0, x0, x1')),
    ('PREL32@.rodata', 'addr', 'd', [], a64_word('.rodata', 'a', '.word SYM - .', 'ldrsw x0, [x1]\n add x0, x0, x1')),
]
X64_COMMON = '''
  .data
  .p2align 3
loc_d: .quad 0x11
  .text
loc_fn: movl $0x11, %eax
  ret
  .globl E_loc_d
  .type E_loc_d,@function
E_loc_d: leaq loc_d(%rip), %rax
  ret
  .globl E_loc_fn
  .type E_loc_fn,@function
E_loc_fn: leaq loc_fn(%rip), %rax
  ret
  .globl probe
  .type probe,@function
'''
def x64_word(sec, flags, directive, load):
    return '''
  .section %s,"%s"
  .p2align 3
w: %s
  .text
probe: leaq w(%%rip), %%rcx
  %s
  ret
''' % (sec, flags, directive, load)
X64 = [
    ('PC32(lea)', 'addr', 'd', [], 'probe: leaq SYM(%rip), %rax\n ret\n'),
    ('PC32(mov load)', 'val', 'd', [], 'probe: movq SYM(%rip), %rax\n ret\n'),
    ('32(movl $)', 'addr', 'd', [], 'probe: movl $SYM, %eax\n ret\n'),
    ('32S(movq $)', 'addr', 'd', [], 'probe: movq $SYM, %rax\n ret\n'),
    ('64@.text(movabs)', 'addr', 'd', [], 'probe: movabsq $SYM, %rax\n ret\n'),
    ('REX_GOTPCRELX(control)', 'addr', 'd', [], 'probe: movq SYM@GOTPCREL(%rip), %rax\n ret\n'),
    ('GOTPCREL(norelax)', 'addr', 'd', ['-Wa,-mrelax-relocations=no'], 'probe: movq SYM@GOTPCREL(%rip), %rax\n ret\n'),
    ('PLT32(jmp)', 'call', 'fn', [], 'probe: jmp SYM@PLT\n'),
    ('PC32(jmp e9)', 'call', 'fn', [], 'probe: .byte 0xe9\n .long SYM - . - 4\n'),
    ('64@.data', 'addr', 'd', [], x64_word('.data', 'aw', '.quad SYM', 'movq (%rcx), %rax')),
    ('64@.data.rel.ro', 'addr', 'd', [], x64_word('.data.rel.ro', 'aw', '.quad SYM', 'movq (%rcx), %rax')),
    ('64@.rodata', 'addr', 'd', [], x64_word('.rodata', 'a', '.quad SYM', 'movq (%rcx), %rax')),
    ('32@.data', 'addr', 'd', [], x64_word('.data', 'aw', '.long SYM', 'movl (%rcx), %eax')),
    ('PC32@.data', 'addr', 'd', [], x64_word('.data', 'aw', '.long SYM - .', 'movslq (%rcx), %rax\n addq %rcx, %rax')),
    ('PC32@.rodata', 'addr', 'd', [], x64_word('.rodata', 'a', '.long SYM - .', 'movslq (%rcx), %rax\n addq %rcx, %rax')),
]
I386_COMMON = '''
  .data
  .p2align 2
loc_d: .long 0x11
  .text
thunk: movl (%esp), %ebx
  ret
loc_fn: movl $0x11, %eax
  ret
  .globl E_loc_d
  .type E_loc_d,@function
E_loc_d: push %ebx
  call thunk
  addl $_GLOBAL_OFFSET_TABLE_, %ebx
  leal loc_d@GOTOFF(%ebx), %eax
  pop %ebx
  ret
  .globl E_loc_fn
  .type E_loc_fn,@function
E_loc_fn: push %ebx
  call thunk
  addl $_GLOBAL_OFFSET_TABLE_, %ebx
  leal loc_fn@GOTOFF(%ebx), %eax
  pop %ebx
  ret
  .globl probe
  .type probe,@function
'''
EBX = 'probe: push %ebx\n sub $8, %esp\n call thunk\n addl $_GLOBAL_OFFSET_TABLE_, %ebx\n'
EBX_END = ' add $8, %esp\n pop %ebx\n ret\n'
def i386_word(sec, flags, directive, load):
    return '''
  .section %s,"%s"
  .p2align 2
w: %s
  .text
''' % (sec, flags, directive) + EBX + ' leal w@GOTOFF(%ebx), %ecx\n ' + load + '\n' + EBX_END
I386 = [
    ('32@.text(movl $)', 'addr', 'd', [], 'probe: movl $SYM, %eax\n ret\n'),
    ('32@.text(movl load)', 'val', 'd', [], 'probe: movl SYM, %eax\n ret\n'),
    ('PC32(call, no ebx)', 'call', 'fn', [], 'probe: sub $12, %esp\n call SYM\n add $12, %esp\n ret\n'),
    ('PLT32(call, ebx=GOT)', 'call', 'fn', [], EBX + ' call SYM@PLT\n' + EBX_END),
    ('GOT32X(ebx)', 'addr', 'd', [], EBX + ' movl SYM@GOT(%ebx), %eax\n' + EBX_END),
    ('GOT32(ebx,norelax)', 'addr', 'd', ['-Wa,-mrelax-relocations=no'], EBX + ' movl SYM@GOT(%ebx), %eax\n' + EBX_END),
    ('GOT32X(no base)', 'addr', 'd', [], 'probe: movl SYM@GOT, %eax\n ret\n'),
    ('GOTOFF(ebx)', 'addr', 'd', [], EBX + ' leal SYM@GOTOFF(%ebx), %eax\n' + EBX_END),
    ('32@.data', 'addr', 'd', [], i386_word('.data', 'aw', '.long SYM', 'movl (%ecx), %eax')),
    ('32@.data.rel.ro', 'addr', 'd', [], i386_word('.data.rel.ro', 'aw', '.long SYM', 'movl (%ecx), %eax')),
    ('32@.rodata', 'addr', 'd', [], i386_word('.rodata', 'a', '.long SYM', 'movl (%ecx), %eax')),
    ('PC32@.data', 'addr', 'd', [], i386_word('.data', 'aw', '.long SYM - .', 'movl (%ecx), %eax\n addl %ecx, %eax')),
]
FORMS = {'a64': (A64_COMMON, A64), 'x64': (X64_COMMON, X64), 'i386': (I386_COMMON, I386)}

def short(t):
    for p in ('R_AARCH64_', 'R_X86_64_', 'R_386_'):
        t = t.replace(p, '')
    return t

def judge(a, L, d, lib, mode, kind, tcls, out_rows, label):
    e, tags = dyn_summary(os.path.join(work, lib))
    tr = 'TEXTREL' in tags or ('FLAGS' in tags and tags['FLAGS'][0] & 4)
    rl = relocs(os.path.join(work, lib))
    # records naming the target, and RELATIVE records whose place is in the probe's word / text
    syms = set(SYM.values()) | {'tv'}
    recs = sorted(set('%s:%s' % (short(t), s.split('@')[0]) for sec, off, t, s in rl if s.split('@')[0] in syms))
    res = []
    if not (kind == 'wk' and mode != 'addr'):
        for host in ('host', 'host_int'):
            rc, t = run(a, '%s/%s' % (a, host), ['%s/dep' % a], args=[os.path.join(work, lib), mode, kind, tcls])
            t = t.strip().splitlines()
            line = t[-1] if t else ''
            if rc not in (0, 1, 2):
                line = 'CRASH rc=%d %s' % (rc, line[:80])
            res.append(('plain' if host == 'host' else 'interp') + ':' + (line.split(' base=')[0]))
    else:
        res.append('not run (weak undefined %s)' % mode)
    return '%s%s recs=[%s] %s' % ('TEXTREL ' if tr else '', 'ok', ','.join(recs), ' '.join(res))

for a in ARCHES:
    os.makedirs(os.path.join(work, a, 'dep'), exist_ok=True)
    cc(a, 'gcc', 'd_defs.c', a + '/defs.o', ['-fPIC', '-O2'])
    cc(a, 'gcc', 'd_dep.c', a + '/dep.o', ['-fPIC', '-O2'])
    link(a, 'bfd', [a + '/dep.o'], a + '/dep/libdep.so', ['-Wl,-soname,libdep.so'])
    sh([GCC[a], 'd_host.c', '-o', a + '/host'])
    sh([GCC[a], '-DINTERPOSE', '-rdynamic', 'd_host.c', '-o', a + '/host_int'])
    common, forms = FORMS[a]
    out('=== D %s: assembly forms (cell: link verdict; TEXTREL if DT_TEXTREL/DF_TEXTREL; recs = dynamic records '
        'naming the target; run plain / interposed host (defines pre_d, pre_fn))' % a)
    for fi, (fname, mode, tcls, aflags, body) in enumerate(forms):
        out('--- %s %s (%s)' % (a, fname, mode))
        for kind in KINDS:
            sym = SYM[(kind, tcls)]
            src = common
            if kind == 'hid':
                src += '  .hidden %s\n' % sym
            if kind == 'wk':
                src += '  .weak %s\n' % sym
            src += body.replace('SYM', sym)
            sname = '%s/f%02d_%s.s' % (a, fi, kind)
            write(sname, src)
            o = sname[:-2] + '.o'
            rc, t = cc(a, 'gcc', sname, o, aflags)
            if rc:
                out('  %-4s ASSEMBLY FAILED: %s' % (kind, first_diag(t, 2)))
                continue
            orel = ','.join(sorted(set(short(t) for sec, off, t, s in relocs(os.path.join(work, o))
                                       if s == sym and sec.endswith(('.text', '.data', '.rodata', '.data.rel.ro')))))
            for L in LINKERS:
                lib = '%s/f%02d_%s.%s.so' % (a, fi, kind, L)
                rc, t = link(a, L, [o, a + '/defs.o', '-L' + a + '/dep', '-ldep'], lib, ['-Wl,-soname,libt.so'])
                warn = first_diag(t, 2)
                if rc:
                    out('  %-4s %-5s [%s] REFUSED: %s' % (kind, L, orel, warn))
                    continue
                r = judge(a, L, a, lib, mode, kind, tcls, None, fname)
                out('  %-4s %-5s [%s] %s%s' % (kind, L, orel, r, (' WARN: ' + warn) if warn else ''))

# --- compiler-generated non-PIC code in a shared object
C_CASES = [
    ('callext', 'call', 'imp', 'fn', 'extern long imp_fn(void); long probe(void){return imp_fn()+0;}'),
    ('addrext', 'addr', 'imp', 'fn', 'extern long imp_fn(void); long probe(void){return (long)imp_fn;}'),
    ('dataext', 'val', 'imp', 'd', 'extern long imp_d; long probe(void){return imp_d;}'),
    ('static', 'val', 'loc', 'd', 'static long sd = 0x11; long probe(void){ long *volatile p = &sd; return *p; }'),
    ('hidden', 'val', 'hid', 'd', 'extern long hid_d __attribute__((visibility("hidden"))); long probe(void){return hid_d;}'),
    ('defsame', 'val', 'pre', 'd', 'extern long pre_d; long probe(void){return pre_d;}'),
    ('callsame', 'call', 'pre', 'fn', 'extern long pre_fn(void); long probe(void){return pre_fn()+0;}'),
    ('tls', 'val', 'tls', 'd', 'extern __thread long tv; long probe(void){return tv;}'),
    ('stls', 'val', 'stls', 'd', 'static __thread long stv = 0x66; long probe(void){return stv;}'),
    ('ptr_data', 'val', 'pre', 'd', 'extern long pre_d; long *p = &pre_d; long probe(void){return *p;}'),
    ('ptr_ro', 'val', 'pre', 'd', 'extern long pre_d; long *const p = &pre_d; long probe(void){return **(long *const volatile *)&p;}'),
    ('ptr_ro_imp', 'val', 'imp', 'd', 'extern long imp_d; long *const p = &imp_d; long probe(void){return **(long *const volatile *)&p;}'),
    ('fnptr_ro_imp', 'call', 'imp', 'fn', 'extern long imp_fn(void); long (*const fp)(void) = imp_fn; long probe(void){return (*(long (*const volatile *)(void))&fp)();}'),
    ('ptr_ro_static', 'val', 'loc', 'd', 'static long sd = 0x11; long *const p = &sd; long probe(void){return **(long *const volatile *)&p;}'),
]
for a in ARCHES:
    out('=== D %s: compiler-generated objects without -fPIC, linked -shared' % a)
    for cname, mode, kind, tcls, src in C_CASES:
        write('c_%s.c' % cname, src + '\n')
        for comp in ('gcc', 'clang22'):
            for pic in ('-fno-pic', '-fPIE'):
                o = '%s/c_%s.%s%s.o' % (a, cname, comp, pic)
                cc(a, comp, 'c_%s.c' % cname, o, [pic, '-O2'])
                orel = ','.join('%s:%s@%s' % (short(t), s, sec.replace('.rela', '').replace('.rel', ''))
                                for sec, off, t, s in relocs(os.path.join(work, o)) if s and not s.startswith('.L'))
                for L in LINKERS:
                    if L == 'lld19' and comp != 'gcc':
                        continue
                    lib = '%s/c_%s.%s%s.%s.so' % (a, cname, comp, pic, L)
                    rc, t = link(a, L, [o, a + '/defs.o', '-L' + a + '/dep', '-ldep'], lib, ['-Wl,-soname,libt.so'])
                    warn = first_diag(t, 2)
                    if rc:
                        out('  %-13s %-7s %-8s %-5s [%s] REFUSED: %s' % (cname, comp, pic, L, orel, warn))
                        continue
                    r = judge(a, L, a, lib, mode, kind, tcls, None, cname)
                    out('  %-13s %-7s %-8s %-5s [%s] %s%s' % (cname, comp, pic, L, orel, r, (' WARN: ' + warn) if warn else ''))
S.close()
