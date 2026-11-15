# sourced by run.sh; uses lk/all/allsym/log/sec, X, ARCH, CC
NA=-Wl,--no-as-needed
AN=-Wl,--as-needed
RL=-Wl,-rpath-link,middir:leafdir

e1() {
  sec "E1 DT_NEEDED names and order (gcc driver passes --as-needed by default; NA = -Wl,--no-as-needed first)"
  all 1.1-l-forms 1 -- $NA m_all.o -Ld -la -lb -lns -Ltopdir -ltop $RL
  all 1.2a-ns-relpath 1 -- $NA m_ns.o d/libns.so
  all 1.2b-ns-abspath 1 -- $NA m_ns.o $X/d/libns.so
  all 1.2c-ns-dotdot 1 -- $NA m_ns.o ./d/../d/libns.so
  all 1.2d-ns-l 1 -- $NA m_ns.o -Ld -lns
  all 1.2e-ns-lcolon 1 -- $NA m_ns.o -Ld -l:libns.so
  all 1.3-order-libs-around-obj 1 -- $NA d/libns.so m_all.o topdir/libtop.so d/libb.so d/liba.so $RL
  all 1.4-dup-spellings 1 -- $NA m_a.o -Ld -la d/liba.so $X/d/liba.so -la
  all 1.5a-same-soname-1then2 1 -- $NA m_dup.o d/libdup1.so d/libdup2.so
  all 1.5b-same-soname-2then1 1 -- $NA m_dup.o d/libdup2.so d/libdup1.so
  all 1.5c-extra-only-in-2nd 1 -- $NA m_dupx.o d/libdup1.so d/libdup2.so
  all 1.6a-group-script-NA 1 -- $NA m_a.o d/libgrp.so $RL
  all 1.6b-group-script-AN 1 -- $AN m_a.o d/libgrp.so $RL
  all 1.6c-group-script-hook 1 -- $NA m_hook.o d/libgrp.so $RL
  all 1.7a-hello-default 1 -- m_hello.o
  all 1.7b-hello-NA 1 -- $NA m_hello.o
  all 1.8a-tls_get_addr-default 1 -- m_tga.o
  all 1.8b-dl_find_object-default 1 -- m_dfo.o
  all 1.8c-tls-var-from-so 1 -- m_tls.o d/libtls.so
}

e2() {
  sec "E2 --as-needed"
  all 2.1-strong 1 -- $AN m_a.o d/liba.so
  all 2.2a-weak-only-AN 1 -- $AN m_wa.o d/liba.so
  all 2.2b-weak-only-NA 1 -- $NA m_wa.o d/liba.so
  all 2.3a-closure-listed 1 -- $AN m_top.o topdir/libtop.so middir/libmid.so leafdir/libleaf.so
  all 2.3b-closure-underlinked 1 -- $AN m_top.o topdir/libtopu.so middir/libmid.so leafdir/libleaf.so
  all 2.4-resolved-by-earlier 1 -- $AN m_a.o d/liba.so d/liba2.so
  allsym 2.5a-exe-defines-AN a_fn 1 -- $AN m_def.o d/liba.so
  allsym 2.5b-exe-defines-NA a_fn 1 -- $NA m_def.o d/liba.so
  all 2.6a-gc-ref-AN-gc 1 -- $AN -Wl,--gc-sections m_gc.o d/liba.so
  all 2.6b-gc-ref-AN-nogc 1 -- $AN m_gc.o d/liba.so
  all 2.7a-versioned-only-v1 1 -- $AN m_v1.o d/libver.so
  all 2.7b-versioned-default 1 -- $AN m_v.o d/libver.so
  all 2.8a-AN-lib-before-obj 1 -- $AN d/liba.so m_a.o
  all 2.8b-AN-after-obj 1 -- m_a.o $AN d/liba.so
  all 2.8c-AN-then-NA-same-lib 1 -- $AN m_hello.o d/liba.so $NA d/liba.so
  all 2.9-AN-lib-only-weak-def 1 -- $AN m_wddef.o d/libweak.so
  all 2.10a-closure-symbol-default 1 -- $NA m_mid.o -Ltopdir -ltop $RL
  all 2.10b-copy-dt-needed 1 -- $NA m_mid.o -Wl,--copy-dt-needed-entries -Ltopdir -ltop $RL
  all 2.10c-copy-dt-needed-AN 1 -- $AN m_mid.o -Wl,--copy-dt-needed-entries -Ltopdir -ltop $RL
}

e3() {
  sec "E3 undefined symbols in shared objects"
  all 3.1a-libx-default 1 -- $NA m_x.o d/libx.so
  all 3.1b-libx-znow 1 -- $NA -Wl,-z,now m_x.o d/libx.so
  all 3.2a-libx-allow 1 -- $NA -Wl,--allow-shlib-undefined m_x.o d/libx.so
  all 3.2b-libx-noallow 1 -- $NA -Wl,--no-allow-shlib-undefined m_x.o d/libx.so
  all 3.3-libxw-weak-undef 1 -- $NA m_xw.o d/libxw.so
  all 3.4a-closure-undef-rpathlink 1 -- $NA m_umid.o middir/libumid.so -Wl,-rpath-link,leafdir
  all 3.4b-closure-undef-both-on-line 1 -- $NA m_umid.o middir/libumid.so leafdir/libuleaf.so
  all 3.5-closure-undef-not-found 1 -- $NA m_umid.o middir/libumid.so
  all 3.6-AN-dropped-libx 1 -- $AN m_a.o d/liba.so d/libx.so
  all 3.7-libx-unused-NA-AN 1 -- $AN m_a.o d/liba.so $NA d/libx.so
  allsym 3.8a-exe-defines-hook-norexport hook 1 -- $AN m_hook.o d/libhook.so
  allsym 3.8b-exe-defines-hook-NA hook 1 -- $NA m_hook.o d/libhook.so
  allsym 3.8c-exe-defines-hook-AN-lib-dropped hook 1 -- $AN m_hook.o d/liba.so d/libhook.so -Wl,--defsym,unused_x=0
}

e4() {
  sec "E4 the DT_NEEDED closure, -rpath-link"
  all 4.1-mid-not-found 1 -- $NA m_top.o -Ltopdir -ltop
  all 4.1b-mid-found-leaf-not 1 -- $NA m_top.o -Ltopdir -ltop -Wl,-rpath-link,middir
  allsym 4.2a-hook-in-closure hook 1 -- $NA m_htop.o -Ltopdir -lhtop -Wl,-rpath-link,middir
  allsym 4.2b-hook-closure-not-found hook 1 -- $NA m_htop.o -Ltopdir -lhtop
  allsym 4.2c-hook-both-on-line hook 1 -- $NA m_htop.o -Ltopdir -lhtop middir/libhmid.so
  all 4.3a-closure-def-for-exe-ref 1 -- $NA m_mid.o -Ltopdir -ltop $RL
  all 4.3b-closure-def-not-found 1 -- $NA m_mid.o -Ltopdir -ltop
  all 4.3c-closure-def-in-L-dir 1 -- $NA m_mid.o -Ltopdir -Lmiddir -Lleafdir -ltop

  sec "E4.4 GNU ld search order for DT_NEEDED of libhtop (which libhmid.so: --verbose 'found' line)"
  local s
  for s in s_rl s_rp s_run s_ldlp s_L s_dtr s_org; do mkdir -p $X/$s; cp $X/middir/libhmid.so $X/$s/; done
  mkdir -p $X/htd $X/hto/sub
  so $X/htd/libhtop.so libhtop.so src/libhtop.c -Wl,--enable-new-dtags -Wl,-rpath,$X/s_dtr -Lmiddir -lhmid
  so $X/hto/sub/libhtop.so libhtop.so src/libhtop.c -Wl,--enable-new-dtags '-Wl,-rpath,$ORIGIN/../../s_org' -Lmiddir -lhmid
  mkdir -p $X/htr; so $X/htr/libhtop.so libhtop.so src/libhtop.c -Wl,--disable-new-dtags -Wl,-rpath,$X/s_dtr -Lmiddir -lhmid
  log "htr/libhtop.so: $(readelf -dW $X/htr/libhtop.so | grep -E 'RUNPATH|RPATH')"
  mkdir -p $X/htown; so $X/htown/libhtop.so libhtop.so src/libhtop.c -Lmiddir -lhmid; cp $X/middir/libhmid.so $X/htown/
  log "htd/libhtop.so: $(readelf -dW $X/htd/libhtop.so | grep -E 'RUNPATH|RPATH')"
  log "hto/sub/libhtop.so: $(readelf -dW $X/hto/sub/libhtop.so | grep -E 'RUNPATH|RPATH')"
  srch() { # id, then env assignments as 'K=V' words up to --, then args
    local id=$1; shift; local envs=()
    while [ "$1" != -- ]; do envs+=("$1"); shift; done; shift
    local l
    for l in $LINKERS; do
      local out=$X/out/$id.$l; rm -f $out
      ( cd $X && env "${envs[@]}" $CC -B$W/bin/$ARCH-$l/ -o $out $NA m_htop.o "$@" -Wl,--verbose ) > $out.err 2>&1
      local rc=$?
      log "[$ARCH $id $l] env=${envs[*]} link rc=$rc found: $(grep -E 'found libhmid|libhmid.so, needed' $out.err | sed "s#$X/##g" | paste -sd';' -) hook-exported=$(readelf --dyn-syms -W $out 2>/dev/null | grep -cw hook)"
      grep -E 'error|undefined' $out.err | grep -v '^ *attempt' | sed "s#$X/##g" | head -3 | sed 's/^/    msg: /' >> $LOG
    done
  }
  local E=X_UNUSED=1
  srch 4.4a-rpathlink-vs-rpath $E -- -Ltopdir -lhtop -Wl,-rpath-link,s_rl -Wl,-rpath,$X/s_rp
  srch 4.4b-rpath-vs-LD_RUN_PATH LD_RUN_PATH=$X/s_run -- -Ltopdir -lhtop -Wl,-rpath,$X/s_rp
  srch 4.4c-LD_RUN_PATH-only LD_RUN_PATH=$X/s_run -- -Ltopdir -lhtop
  srch 4.4d-LD_RUN_PATH-vs-DT_RUNPATH LD_RUN_PATH=$X/s_run -- htd/libhtop.so
  srch 4.4e-DT_RUNPATH-vs-LD_LIBRARY_PATH LD_LIBRARY_PATH=$X/s_ldlp -- htd/libhtop.so
  srch 4.4f-LD_RUN_PATH-vs-LD_LIBRARY_PATH LD_RUN_PATH=$X/s_run LD_LIBRARY_PATH=$X/s_ldlp -- -Ltopdir -lhtop
  srch 4.4g-LD_LIBRARY_PATH-only LD_LIBRARY_PATH=$X/s_ldlp -- -Ltopdir -lhtop
  srch 4.4h-L-dir-only $E -- -Ltopdir -Ls_L -lhtop
  srch 4.4i-needing-libs-own-dir $E -- htown/libhtop.so
  srch 4.4j-ORIGIN-runpath $E -- hto/sub/libhtop.so
  srch 4.4k-rpath-only $E -- -Ltopdir -lhtop -Wl,-rpath,$X/s_rp
  srch 4.4l-rpathlink-colon-list $E -- -Ltopdir -lhtop -Wl,-rpath-link,/nonexist:s_rl
  srch 4.4m-DT_RUNPATH-vs-rpath $E -- htd/libhtop.so -Wl,-rpath,$X/s_rp
  srch 4.4n-LD_RUN_PATH-ignored-with-rpath LD_RUN_PATH=$X/s_run -- -Ltopdir -lhtop -Wl,-rpath,$X/s_none
  srch 4.4o-DT_RUNPATH-only $E -- htd/libhtop.so
  srch 4.4p-DT_RPATH-vs-LD_LIBRARY_PATH LD_LIBRARY_PATH=$X/s_ldlp -- htr/libhtop.so
  srch 4.4q-rpathlink-vs-LD_LIBRARY_PATH LD_LIBRARY_PATH=$X/s_ldlp -- -Ltopdir -lhtop -Wl,-rpath-link,s_rl
}

e5() {
  sec "E5 symbol resolution with shared objects"
  allsym 5.1-obj-and-so-define a_fn 1 -- $NA m_def2.o d/libref.so
  all 5.2a-two-so-liba-first 1 -- $NA m_a.o d/liba.so d/liba2.so
  all 5.2b-two-so-liba2-first 1 -- $NA m_a.o d/liba2.so d/liba.so
  allsym 5.3-weak-obj-def-vs-so a_fn 1 -- $NA m_wdef2.o d/libref.so
  allsym 5.4a-common-vs-so cv 1 -- $NA m_cv.o d/libcom.so
  allsym 5.4b-common-vs-so-AN cv 1 -- $AN m_cv.o d/libcom.so
  all 5.5a-so-then-archive 1 -- $NA m_a.o d/liba.so arc/libarc.a
  all 5.5b-archive-then-so 1 -- $NA m_a.o arc/libarc.a d/liba.so
  all 5.5c-archive-then-so-AN 1 -- $AN m_a.o arc/libarc.a d/liba.so
  allsym 5.6a-protected-data-pie pv 1 -- $NA m_pv.o d/libprot.so
  allsym 5.6b-protected-data-nopie pv 1 -- $NA -no-pie m_pv_np.o d/libprot.so
  allsym 5.6c-protected-func-pie pf 1 -- $NA m_pf.o d/libprot.so
  allsym 5.6d-protected-func-nopie pf 1 -- $NA -no-pie m_pf_np.o d/libprot.so
  all 5.7-hidden-in-dynsym 1 -- $NA m_vf.o d/libvis.so
  allsym 5.8a-unversioned-ref foo 1 -- $NA m_v.o d/libver.so
  allsym 5.8b-symver-ref-V1 foo 1 -- $NA m_v1.o d/libver.so
  all 5.8f-symver-V1-lib-local-star 1 -- $NA m_v1.o d/libver_loc.so
  all 5.8c-symver-ref-V3 1 -- $NA m_v3.o d/libver.so
  all 5.8d-ref-hidden-only-version 1 -- $NA m_hid.o d/libver.so
  allsym 5.8e-exe-def-preempts-versioned foo 1 -- $NA m_vdef.o d/libver.so
  allsym 5.9-gnu-unique uq 1 -- $NA m_uq.o d/libuniq.so
  allsym 5.10a-ifunc-from-so-pie ifn 1 -- $NA m_if.o d/libifunc.so
  allsym 5.10b-ifunc-from-so-nopie ifn 1 -- $NA -no-pie m_if_np.o d/libifunc.so
  all 5.11a-weak-def-in-so 1 -- $NA m_wd.o d/libweak.so
  allsym 5.11b-strong-exe-vs-weak-so wd 1 -- $NA m_wddef.o d/libweak.so
}

e6() {
  sec "E6 -Bstatic/-Bdynamic, search, odd inputs"
  all 6.1a-l-so-and-a-same-dir 1 -- $NA m_q.o -Lq -lq
  all 6.1b-Bstatic 1 -- $NA m_q.o -Lq -Wl,-Bstatic -lq -Wl,-Bdynamic
  all 6.1c-a-in-first-dir 1 -- $NA m_q.o -Lqa -Lq -lq
  all 6.1d-so-in-first-dir 1 -- $NA m_q.o -Lq -Lqa -lq
  all 6.1e-static 1 -- m_q.o -static -Lq -lq
  all 6.1f-Bstatic-so-by-path 1 -- $NA m_q.o -Wl,-Bstatic q/libq.so -Wl,-Bdynamic
  all 6.2-soname-differs-from-filename 1 -- $NA m_all.o d/libb.so d/liba.so d/libns.so topdir/libtop.so $RL
  all 6.3a-INPUT-script 1 -- $NA m_ns.o -Ld d/libinp.so
  all 6.3b-INPUT-script-l 1 -- $NA m_ns.o -Ld -linp
  all 6.4a-dangling-symlink-path 1 -- $NA m_a.o d/libdang.so d/liba.so
  all 6.4b-dangling-symlink-l 1 -- $NA m_a.o -Ld -ldang -la
  all 6.5a-empty-path 1 -- $NA m_a.o d/libempty.so d/liba.so
  all 6.5b-empty-l 1 -- $NA m_a.o -Ld -lempty -la
  all 6.6a-text-junk 1 -- $NA m_a.o d/libjunk.so d/liba.so
  all 6.6b-bad-elf 1 -- $NA m_a.o d/libbad.so d/liba.so
}

experiments() {
  for m in pv pf if; do obj $X/m_${m}_np.o src/m_$m.c -fno-pie; done
  e1; e2; e3; e4; e5; e6
}
