#!/bin/bash
# ld wrapper: append argv (NUL-free, one arg per line, a blank line after) to
# $LDLOG with the current directory first, then exec GNU ld.
{ echo "CWD $PWD"; printf '%s\n' "$@"; echo; } >> "$LDLOG"
exec /usr/bin/ld.bfd "$@"
