# aside.py DIR: (1) the U1..W2 window: exec of P while the update holds P
# open O_RDWR (IN-77a says the write-open lasts "from W2 to W4"; U1 opens it);
# (2) IN-77 W4's rename(A, P) when a non-cooperating writer (a plain link,
# IN-11: no lock) put a file at P while the output was aside: clobbered;
# (3) link(A, P) + unlink(A) (std::fs::hard_link) refuses with EEXIST.
import os, sys, subprocess, shutil
d = sys.argv[1]
P, A = os.path.join(d, 'out'), os.path.join(d, '.out.mortar-torn')
def clean():
    for f in (P, A):
        if os.path.lexists(f): os.unlink(f)
clean()
shutil.copy('/bin/true', P); os.chmod(P, 0o755)
fd = os.open(P, os.O_RDWR)
try:
    r = subprocess.run([P]); print('(1) exec P after U1, before W2:', 'ran', r.returncode)
except OSError as e:
    print('(1) exec P after U1, before W2:', e.strerror)
os.rename(P, A)                                   # W2
with open(P, 'wb') as f: f.write(b'FOREIGN')     # a plain link's rename lands at P
os.rename(A, P)                                   # W4 as written
print('(2) after W4 rename back, P holds:', open(P, 'rb').read(7), '(foreign file lost:', not os.path.exists(A), ')')
os.close(fd); clean()
shutil.copy('/bin/true', P)
os.rename(P, A)
with open(P, 'wb') as f: f.write(b'FOREIGN')
try:
    os.link(A, P); print('(3) link(A,P): succeeded (clobber!)')
except FileExistsError:
    print('(3) link(A,P): EEXIST, P keeps', open(P, 'rb').read(7))
clean()
shutil.copy('/bin/true', P); os.rename(P, A); os.link(A, P); os.unlink(A)
print('(3b) link+unlink with P free: P exists', os.path.exists(P), 'nlink', os.stat(P).st_nlink)
clean()
