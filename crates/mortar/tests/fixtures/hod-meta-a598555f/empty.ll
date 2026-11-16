@__mason_meta = private constant [0 x i8] zeroinitializer, section ".mason.meta", align 8, !exclude !{}
@llvm.compiler.used = appending global [1 x ptr] [ptr @__mason_meta], section "llvm.metadata"
define i32 @main() {
  ret i32 0
}
