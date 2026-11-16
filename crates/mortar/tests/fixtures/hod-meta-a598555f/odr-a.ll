$_Z1fv = comdat any
define linkonce_odr i32 @_Z1fv() comdat {
  ret i32 1
}
define i32 @main() {
  %r = call i32 @_Z1fv()
  ret i32 %r
}

; Mason metadata: cargo xtask meta write --into-ll
@__mason_meta = private constant [224 x i8] c"MASN\01\00\00\00\E0\00\00\00 \00\00\00\02\00\00\00\A0\00\00\00?\00\00\00\04\00\00\00\01\00\01\00@\00\00\00\10\00\00\00\03\00\00\00\01\00\00\00\06\00\00\00\00\00\00\00\00\00\00\00\0A\00\00\00\15\00\00\00\00\00\00\00\00\00\00\00\1D\00\00\00&\00\00\00\00\00\00\00\00\00\00\00\02\00\01\00@\00\00\000\00\00\00\01\00\00\00\0F\F2WQ\10\FCcD\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\00\00\00\00\01\00\01\00\00\00\00\00;\00\00\00\01\00\00\00\00\00\00\00\00lang\00c++\00odr.scheme\00model-1\00producer\00hod fixture (WP-099)\00h.h\00\00", section ".mason.meta", align 8, !exclude !{}
@llvm.compiler.used = appending global [1 x ptr] [ptr @__mason_meta], section "llvm.metadata"
