#!/bin/bash
# Q10: a correct -no-pie program copying read-only data of a DT_SYMBOLIC system library.
ulimit -c 0; cd "$(dirname "$0")"; W=work.$$; mkdir $W; cd $W
L=/usr/lib/aarch64-linux-gnu/libgdk_pixbuf-2.0.so.0.4200.12
gcc -fno-pic -no-pie ../host.c $L -o host && readelf -rW host | grep -E 'COPY' ; ./host
python3 -I ../scan.py /usr/lib/aarch64-linux-gnu /usr/x86_64-linux-gnu/lib /usr/lib/x86_64-linux-gnu > scan.out 2>/dev/null; grep -E '^SYMBOLIC|^PROTECTED|^#' scan.out
