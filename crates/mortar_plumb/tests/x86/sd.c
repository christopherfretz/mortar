/* W10: GCC -gsplit-dwarf: .debug_addr holds R_386_32 against TLS. */
__thread long pad[3]; __thread int tv = 5;
int main(void){return tv+(int)pad[1]-5;}
