"""WP-239: decode the rows of a format-4 state that the models need
(FILES identity and hash, SYMBOLS names, SECTIONS' file, SLOTS, INDEX).
Row layouts: crates/mortar_incr/src/format.rs (`impl Row for ...`)."""
import struct

import elfobj


class State:
    def __init__(self, path):
        self.d = d = elfobj.state_bytes(path)
        self.t = elfobj.state_tables(path)
        self.strs_at = self.t[23][2]

    def rows(self, kind):
        n, nb, at = self.t[kind]
        w = nb // n if n else 0
        return [(at + i * w) for i in range(n)]

    def s(self, at):
        off, ln = struct.unpack_from('<II', self.d, at)
        return self.d[self.strs_at + off:self.strs_at + off + ln].decode('latin1')

    def files(self):
        out = []
        for at in self.rows(3):
            path = self.s(at + 4)
            member = self.s(at + 16)
            ordinal, = struct.unpack_from('<I', self.d, at + 24)
            h = self.d[at + 44:at + 60]
            out.append(((path, member, ordinal), h))
        return out

    def symbols(self):
        out = []
        for at in self.rows(6):
            kind, file, ordinal = struct.unpack_from('<B3xII', self.d, at)
            out.append((kind, file, ordinal, self.s(at + 12)))
        return out

    def section_files(self):
        return [struct.unpack_from('<I', self.d, at)[0] for at in self.rows(7)]

    def slots(self):
        return [struct.unpack_from('<B3xI', self.d, at) for at in self.rows(12)]

    def index(self):
        return [struct.unpack_from('<IB3xI', self.d, at) for at in self.rows(34)]
