## libc++ through the Mason lane

dirs: std libcxx; shard: all; backend: hod

| mode | summary | object jobs by back end |
|---|---|---|
| mortar | 9247 tests: 8609 pass, 5 fail, 46 xfail, 2 xpass, 584 unsupported (24 driver), 0 killed, 1 timeout, 0 error; pass rate 99.4%; wall 1142.6 s; --compiler kiln | ? 1, hod 7207, llvm 2 |

### mortar

links: 7219; linked 7193; refused 26

| refusals | diagnostic |
|---|---|
| 12 | `mortar-da61bbc: error: undefined symbol: __atomic_load (undefined reference to `__atomic_load')` |
| 6 | `mortar-da61bbc: error: undefined symbol: __atomic_store (undefined reference to `__atomic_store')` |
| 4 | `mortar-da61bbc: error: undefined symbol: __atomic_compare_exchange (undefined reference to `__atomic_compare_exchange')` |
| 3 | `mortar-da61bbc: error: undefined symbol: __atomic_exchange (undefined reference to `__atomic_exchange')` |
| 1 | `mortar-da61bbc: error: undefined symbol: __atomic_is_lock_free (undefined reference to `__atomic_is_lock_free')` |

plumb on linked outputs:
- 7193: clean
