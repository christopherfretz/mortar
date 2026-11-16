@__mason_meta = private constant [80 x i8] c"MASN\01\00\00\00P\00\00\00 \00\00\00\01\00\00\00@\00\00\00\10\00\00\00\04\00\00\00\01\00\01\00 \00\00\00\10\00\00\00\01\00\00\00\01\00\00\00\06\00\00\00\00\00\00\00\00\00\00\00\00lang\00c++\00\00\00\00\00\00\00", section ".ldata.mason.meta", align 8, !exclude !{}
@llvm.compiler.used = appending global [1 x ptr] [ptr @__mason_meta], section "llvm.metadata"
define i32 @main() {
  ret i32 0
}
