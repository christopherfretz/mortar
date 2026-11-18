# syncprobe.py DIR [MB] [ROUNDS]: the fixed write-protocol cost of ONE in-place
# update as IN-77 (WP-207) writes it, on the file system of DIR, against the
# go/no-go budget. A 50 MB "output" P and a 9 MB "state" S are created once;
# each round runs W1..W6 with 4 dirty 1 MiB windows and a 24 KB state patch:
#   W1 flag+fdatasync(S); W2 rename(P,A), stat, fsync(dir), zero e_ident,
#   fdatasync(P); W3 windows+fdatasync(P); W4 header, fsync(P), rename(A,P),
#   fsync(dir); W5 patch+fdatasync(S); W6 flag+fdatasync(S)
# plus the lock round trip (open S, flock, fstat, stat). Prints per-step and
# total medians and p90 in ms. Removes its files at the end (literal names).
import fcntl, os, statistics, sys, time

d = sys.argv[1]
mb = int(sys.argv[2]) if len(sys.argv) > 2 else 50
rounds = int(sys.argv[3]) if len(sys.argv) > 3 else 15
P = os.path.join(d, 'out.bin')
A = os.path.join(d, '.out.bin.mortar-torn')
S = os.path.join(d, 'out.bin.mortar-state')
blk = os.urandom(1 << 20)
with open(P, 'wb') as f:
    for _ in range(mb):
        f.write(blk)
with open(S, 'wb') as f:
    for _ in range(9):
        f.write(blk)
dfd = os.open(d, os.O_RDONLY)
steps = {k: [] for k in ('lock', 'W1', 'W2', 'W3', 'W4', 'W5', 'W6', 'total')}
for r in range(rounds):
    t0 = time.perf_counter()
    s = os.open(S, os.O_RDWR)
    fcntl.flock(s, fcntl.LOCK_EX)
    os.fstat(s); os.stat(S)
    p = os.open(P, os.O_RDWR)
    t1 = time.perf_counter(); steps['lock'].append(t1 - t0)
    os.pwrite(s, b'\x01\0\0\0', 16); os.fdatasync(s)
    t2 = time.perf_counter(); steps['W1'].append(t2 - t1)
    os.rename(P, A); os.stat(A); os.fsync(dfd); os.pwrite(p, b'\0' * 16, 0); os.fdatasync(p)
    t3 = time.perf_counter(); steps['W2'].append(t3 - t2)
    for i in range(4):
        os.pwrite(p, blk, (5 + 11 * i) << 20)
    os.fdatasync(p)
    t4 = time.perf_counter(); steps['W3'].append(t4 - t3)
    os.pwrite(p, b'\x7fELF' + b'\2' * 12, 0); os.fsync(p); os.rename(A, P); os.fsync(dfd)
    t5 = time.perf_counter(); steps['W4'].append(t5 - t4)
    for i in range(6):
        os.pwrite(s, blk[:4096], (1 + 37 * i) * 4096)
    os.fdatasync(s)
    t6 = time.perf_counter(); steps['W5'].append(t6 - t5)
    os.pwrite(s, b'\0\0\0\0', 16); os.fdatasync(s)
    t7 = time.perf_counter(); steps['W6'].append(t7 - t6)
    fcntl.flock(s, fcntl.LOCK_UN); os.close(s); os.close(p)
    steps['total'].append(t7 - t0)
os.close(dfd)
os.unlink(P); os.unlink(S)
for k, v in steps.items():
    v = sorted(x * 1000 for x in v)
    print(f'{k:6s} median {statistics.median(v):8.2f} ms  p90 {v[int(0.9 * (len(v) - 1))]:8.2f} ms  max {v[-1]:8.2f}')
print(f'load {open("/proc/loadavg").read().split()[0]}  dir {d}  output {mb} MB  rounds {rounds}')
