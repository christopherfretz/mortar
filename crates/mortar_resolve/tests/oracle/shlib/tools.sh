# Sourced by run.sh and usedset.sh.
# The tools for each architecture: ORACLE_PREFIX_<arch> is the drivers'
# and binutils' prefix (empty on that architecture's own host, the
# default for aarch64; `x86_64-linux-gnu-` the default for x86_64), set
# by shlib_oracle.rs from the host (mortar_testutil::target).
pfx() { local v=ORACLE_PREFIX_$1 d=; [ "$1" = x86_64 ] && d=x86_64-linux-gnu-; echo "${!v-$d}"; }
triple() { case $1 in aarch64) echo aarch64-linux-gnu;; x86_64) echo x86_64-linux-gnu;; esac; }
bfd_of() { command -v "$(pfx $1)ld.bfd"; }
