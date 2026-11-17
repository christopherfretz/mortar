# fsprobe.py DIR: (1) a positioned write after the mtime was set to the epoch
# moves the mtime back to now (so W2's epoch mtime does not survive W3);
# (2) the cost of open + flock(LOCK_EX) + fstat + stat by path + unlock + close.
import os, sys, fcntl, time, statistics
d = sys.argv[1]; p = os.path.join(d, f'wp207-probe-{os.getpid()}')
with open(p, 'wb') as f: f.write(b'\0' * 65536)
os.utime(p, (0, 0)); m0 = os.stat(p).st_mtime_ns
fd = os.open(p, os.O_RDWR); os.pwrite(fd, b'x', 4096); m1 = os.fstat(fd).st_mtime_ns; os.close(fd)
print(f'{d}: mtime after utime(0): {m0}; after one pwrite: {m1} (now {time.time_ns()}): {"BUMPED" if m1 > 0 else "kept"}')
ts = []
for i in range(200):
    t0 = time.perf_counter_ns()
    fd = os.open(p, os.O_RDWR); fcntl.flock(fd, fcntl.LOCK_EX); a = os.fstat(fd); b = os.stat(p)
    assert (a.st_dev, a.st_ino) == (b.st_dev, b.st_ino)
    fcntl.flock(fd, fcntl.LOCK_UN); os.close(fd)
    ts.append(time.perf_counter_ns() - t0)
ts.sort()
print(f'{d}: open+flock+fstat+stat+unlock+close, 200 runs: median {statistics.median(ts)/1000:.1f} us, p90 {ts[180]/1000:.1f} us')
os.unlink(p)
