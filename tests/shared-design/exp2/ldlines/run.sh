#!/bin/bash
# ldlines/run.sh (M-8): the -shared lines of Python's setuptools (sysconfig
# LDSHARED) and of rustc's cdylib, as this host writes them.
ulimit -c 0; cd "$(dirname "$0")"; W=work.run; mkdir -p $W; cd $W
echo "python LDSHARED: $(python3 -I -c "import sysconfig; print(sysconfig.get_config_var('LDSHARED'))")"
export RUSTUP_HOME=/Volumes/AgentWork/mortar-helper/.toolchain/rustup CARGO_HOME=/Volumes/AgentWork/mortar-helper/.toolchain/cargo
R=$CARGO_HOME/bin/rustc
echo '#[no_mangle] pub extern "C" fn rs_add(a: i32, b: i32) -> i32 { a + b }' > lib.rs
$R --version
$R --crate-type cdylib -O --print link-args lib.rs -o librs.so 2>&1 | tr ' ' '\n' | grep -E '^"?-(shared|Wl|z|nodefaultlibs|fuse)|^"?-B|version-script|relro|now|Bsym|as-needed|gc-sections|soname' | tr '\n' ' '; echo
readelf -dW librs.so | grep -E 'FLAGS|BIND_NOW|SONAME|SYMBOLIC'
readelf -VW librs.so | grep -m3 -E 'Name:'
