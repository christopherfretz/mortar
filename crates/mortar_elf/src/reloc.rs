//! Relocation type numbers and their names (`spec/elf.md` §2.3).
//!
//! One module per machine, each with the constants of its processor
//! supplement and a `name` function for diagnostics. A type this module
//! does not name is `None`: naming a type is not a statement of support.

use crate::consts::{EM_386, EM_AARCH64, EM_X86_64, named};

/// The name of relocation type `r_type` in an object for `e_machine`, or
/// `None` when the machine is not one of the three or the type is not
/// named.
pub fn name(e_machine: u16, r_type: u32) -> Option<&'static str> {
    match e_machine {
        EM_AARCH64 => aarch64::name(r_type),
        EM_X86_64 => x86_64::name(r_type),
        EM_386 => i386::name(r_type),
        _ => None,
    }
}

/// AAELF64 "Relocation": the LP64 types, and the `R_AARCH64_AUTH_*` types
/// of the `PAuth` ABI extension to it (0x244 to 0x255, 0x411 to 0x414).
/// The ILP32 types (`R_AARCH64_P32_*`, 1 to 188) occur only in
/// `ELFCLASS32` objects, which the readers refuse, and are not named.
pub mod aarch64 {
    use super::named;

    named! {
        /// The name of an AArch64 relocation type.
        fn name(u32);
        R_AARCH64_NONE = 0;
        /// The withdrawn second code of `R_AARCH64_NONE` (AAELF64), under
        /// the name binutils gives it; LLVM calls 256 `R_AARCH64_NONE` too.
        R_AARCH64_NULL = 256;
        R_AARCH64_ABS64 = 257;
        R_AARCH64_ABS32 = 258;
        R_AARCH64_ABS16 = 259;
        R_AARCH64_PREL64 = 260;
        R_AARCH64_PREL32 = 261;
        R_AARCH64_PREL16 = 262;
        R_AARCH64_MOVW_UABS_G0 = 263;
        R_AARCH64_MOVW_UABS_G0_NC = 264;
        R_AARCH64_MOVW_UABS_G1 = 265;
        R_AARCH64_MOVW_UABS_G1_NC = 266;
        R_AARCH64_MOVW_UABS_G2 = 267;
        R_AARCH64_MOVW_UABS_G2_NC = 268;
        R_AARCH64_MOVW_UABS_G3 = 269;
        R_AARCH64_MOVW_SABS_G0 = 270;
        R_AARCH64_MOVW_SABS_G1 = 271;
        R_AARCH64_MOVW_SABS_G2 = 272;
        R_AARCH64_LD_PREL_LO19 = 273;
        R_AARCH64_ADR_PREL_LO21 = 274;
        R_AARCH64_ADR_PREL_PG_HI21 = 275;
        R_AARCH64_ADR_PREL_PG_HI21_NC = 276;
        R_AARCH64_ADD_ABS_LO12_NC = 277;
        R_AARCH64_LDST8_ABS_LO12_NC = 278;
        R_AARCH64_TSTBR14 = 279;
        R_AARCH64_CONDBR19 = 280;
        R_AARCH64_JUMP26 = 282;
        R_AARCH64_CALL26 = 283;
        R_AARCH64_LDST16_ABS_LO12_NC = 284;
        R_AARCH64_LDST32_ABS_LO12_NC = 285;
        R_AARCH64_LDST64_ABS_LO12_NC = 286;
        R_AARCH64_MOVW_PREL_G0 = 287;
        R_AARCH64_MOVW_PREL_G0_NC = 288;
        R_AARCH64_MOVW_PREL_G1 = 289;
        R_AARCH64_MOVW_PREL_G1_NC = 290;
        R_AARCH64_MOVW_PREL_G2 = 291;
        R_AARCH64_MOVW_PREL_G2_NC = 292;
        R_AARCH64_MOVW_PREL_G3 = 293;
        R_AARCH64_LDST128_ABS_LO12_NC = 299;
        R_AARCH64_MOVW_GOTOFF_G0 = 300;
        R_AARCH64_MOVW_GOTOFF_G0_NC = 301;
        R_AARCH64_MOVW_GOTOFF_G1 = 302;
        R_AARCH64_MOVW_GOTOFF_G1_NC = 303;
        R_AARCH64_MOVW_GOTOFF_G2 = 304;
        R_AARCH64_MOVW_GOTOFF_G2_NC = 305;
        R_AARCH64_MOVW_GOTOFF_G3 = 306;
        R_AARCH64_GOTREL64 = 307;
        R_AARCH64_GOTREL32 = 308;
        R_AARCH64_GOT_LD_PREL19 = 309;
        R_AARCH64_LD64_GOTOFF_LO15 = 310;
        R_AARCH64_ADR_GOT_PAGE = 311;
        R_AARCH64_LD64_GOT_LO12_NC = 312;
        R_AARCH64_LD64_GOTPAGE_LO15 = 313;
        R_AARCH64_PLT32 = 314;
        R_AARCH64_GOTPCREL32 = 315;
        R_AARCH64_PATCHINST = 316;
        R_AARCH64_FUNCINIT64 = 317;
        R_AARCH64_TLSGD_ADR_PREL21 = 512;
        R_AARCH64_TLSGD_ADR_PAGE21 = 513;
        R_AARCH64_TLSGD_ADD_LO12_NC = 514;
        R_AARCH64_TLSGD_MOVW_G1 = 515;
        R_AARCH64_TLSGD_MOVW_G0_NC = 516;
        R_AARCH64_TLSLD_ADR_PREL21 = 517;
        R_AARCH64_TLSLD_ADR_PAGE21 = 518;
        R_AARCH64_TLSLD_ADD_LO12_NC = 519;
        R_AARCH64_TLSLD_MOVW_G1 = 520;
        R_AARCH64_TLSLD_MOVW_G0_NC = 521;
        R_AARCH64_TLSLD_LD_PREL19 = 522;
        R_AARCH64_TLSLD_MOVW_DTPREL_G2 = 523;
        R_AARCH64_TLSLD_MOVW_DTPREL_G1 = 524;
        R_AARCH64_TLSLD_MOVW_DTPREL_G1_NC = 525;
        R_AARCH64_TLSLD_MOVW_DTPREL_G0 = 526;
        R_AARCH64_TLSLD_MOVW_DTPREL_G0_NC = 527;
        R_AARCH64_TLSLD_ADD_DTPREL_HI12 = 528;
        R_AARCH64_TLSLD_ADD_DTPREL_LO12 = 529;
        R_AARCH64_TLSLD_ADD_DTPREL_LO12_NC = 530;
        R_AARCH64_TLSLD_LDST8_DTPREL_LO12 = 531;
        R_AARCH64_TLSLD_LDST8_DTPREL_LO12_NC = 532;
        R_AARCH64_TLSLD_LDST16_DTPREL_LO12 = 533;
        R_AARCH64_TLSLD_LDST16_DTPREL_LO12_NC = 534;
        R_AARCH64_TLSLD_LDST32_DTPREL_LO12 = 535;
        R_AARCH64_TLSLD_LDST32_DTPREL_LO12_NC = 536;
        R_AARCH64_TLSLD_LDST64_DTPREL_LO12 = 537;
        R_AARCH64_TLSLD_LDST64_DTPREL_LO12_NC = 538;
        R_AARCH64_TLSIE_MOVW_GOTTPREL_G1 = 539;
        R_AARCH64_TLSIE_MOVW_GOTTPREL_G0_NC = 540;
        R_AARCH64_TLSIE_ADR_GOTTPREL_PAGE21 = 541;
        R_AARCH64_TLSIE_LD64_GOTTPREL_LO12_NC = 542;
        R_AARCH64_TLSIE_LD_GOTTPREL_PREL19 = 543;
        R_AARCH64_TLSLE_MOVW_TPREL_G2 = 544;
        R_AARCH64_TLSLE_MOVW_TPREL_G1 = 545;
        R_AARCH64_TLSLE_MOVW_TPREL_G1_NC = 546;
        R_AARCH64_TLSLE_MOVW_TPREL_G0 = 547;
        R_AARCH64_TLSLE_MOVW_TPREL_G0_NC = 548;
        R_AARCH64_TLSLE_ADD_TPREL_HI12 = 549;
        R_AARCH64_TLSLE_ADD_TPREL_LO12 = 550;
        R_AARCH64_TLSLE_ADD_TPREL_LO12_NC = 551;
        R_AARCH64_TLSLE_LDST8_TPREL_LO12 = 552;
        R_AARCH64_TLSLE_LDST8_TPREL_LO12_NC = 553;
        R_AARCH64_TLSLE_LDST16_TPREL_LO12 = 554;
        R_AARCH64_TLSLE_LDST16_TPREL_LO12_NC = 555;
        R_AARCH64_TLSLE_LDST32_TPREL_LO12 = 556;
        R_AARCH64_TLSLE_LDST32_TPREL_LO12_NC = 557;
        R_AARCH64_TLSLE_LDST64_TPREL_LO12 = 558;
        R_AARCH64_TLSLE_LDST64_TPREL_LO12_NC = 559;
        R_AARCH64_TLSDESC_LD_PREL19 = 560;
        R_AARCH64_TLSDESC_ADR_PREL21 = 561;
        R_AARCH64_TLSDESC_ADR_PAGE21 = 562;
        R_AARCH64_TLSDESC_LD64_LO12 = 563;
        R_AARCH64_TLSDESC_ADD_LO12 = 564;
        R_AARCH64_TLSDESC_OFF_G1 = 565;
        R_AARCH64_TLSDESC_OFF_G0_NC = 566;
        R_AARCH64_TLSDESC_LDR = 567;
        R_AARCH64_TLSDESC_ADD = 568;
        R_AARCH64_TLSDESC_CALL = 569;
        R_AARCH64_TLSLE_LDST128_TPREL_LO12 = 570;
        R_AARCH64_TLSLE_LDST128_TPREL_LO12_NC = 571;
        R_AARCH64_TLSLD_LDST128_DTPREL_LO12 = 572;
        R_AARCH64_TLSLD_LDST128_DTPREL_LO12_NC = 573;
        R_AARCH64_AUTH_ABS64 = 580;
        R_AARCH64_AUTH_MOVW_GOTOFF_G0 = 581;
        R_AARCH64_AUTH_MOVW_GOTOFF_G0_NC = 582;
        R_AARCH64_AUTH_MOVW_GOTOFF_G1 = 583;
        R_AARCH64_AUTH_MOVW_GOTOFF_G1_NC = 584;
        R_AARCH64_AUTH_MOVW_GOTOFF_G2 = 585;
        R_AARCH64_AUTH_MOVW_GOTOFF_G2_NC = 586;
        R_AARCH64_AUTH_MOVW_GOTOFF_G3 = 587;
        R_AARCH64_AUTH_GOT_LD_PREL19 = 588;
        R_AARCH64_AUTH_LD64_GOTOFF_LO15 = 589;
        R_AARCH64_AUTH_ADR_GOT_PAGE = 590;
        R_AARCH64_AUTH_LD64_GOT_LO12_NC = 591;
        R_AARCH64_AUTH_LD64_GOTPAGE_LO15 = 592;
        R_AARCH64_AUTH_GOT_ADD_LO12_NC = 593;
        R_AARCH64_AUTH_GOT_ADR_PREL_LO21 = 594;
        R_AARCH64_AUTH_TLSDESC_ADR_PAGE21 = 595;
        R_AARCH64_AUTH_TLSDESC_LD64_LO12 = 596;
        R_AARCH64_AUTH_TLSDESC_ADD_LO12 = 597;
        R_AARCH64_COPY = 1024;
        R_AARCH64_GLOB_DAT = 1025;
        R_AARCH64_JUMP_SLOT = 1026;
        R_AARCH64_RELATIVE = 1027;
        R_AARCH64_TLS_DTPMOD64 = 1028;
        R_AARCH64_TLS_DTPREL64 = 1029;
        R_AARCH64_TLS_TPREL64 = 1030;
        R_AARCH64_TLSDESC = 1031;
        R_AARCH64_IRELATIVE = 1032;
        R_AARCH64_AUTH_RELATIVE = 1041;
        R_AARCH64_AUTH_GLOB_DAT = 1042;
        R_AARCH64_AUTH_TLSDESC = 1043;
        R_AARCH64_AUTH_IRELATIVE = 1044;
    }
}

/// x86-64 psABI "Relocation".
pub mod x86_64 {
    use super::named;

    named! {
        /// The name of an x86-64 relocation type.
        fn name(u32);
        R_X86_64_NONE = 0;
        R_X86_64_64 = 1;
        R_X86_64_PC32 = 2;
        R_X86_64_GOT32 = 3;
        R_X86_64_PLT32 = 4;
        R_X86_64_COPY = 5;
        R_X86_64_GLOB_DAT = 6;
        R_X86_64_JUMP_SLOT = 7;
        R_X86_64_RELATIVE = 8;
        R_X86_64_GOTPCREL = 9;
        R_X86_64_32 = 10;
        R_X86_64_32S = 11;
        R_X86_64_16 = 12;
        R_X86_64_PC16 = 13;
        R_X86_64_8 = 14;
        R_X86_64_PC8 = 15;
        R_X86_64_DTPMOD64 = 16;
        R_X86_64_DTPOFF64 = 17;
        R_X86_64_TPOFF64 = 18;
        R_X86_64_TLSGD = 19;
        R_X86_64_TLSLD = 20;
        R_X86_64_DTPOFF32 = 21;
        R_X86_64_GOTTPOFF = 22;
        R_X86_64_TPOFF32 = 23;
        R_X86_64_PC64 = 24;
        R_X86_64_GOTOFF64 = 25;
        R_X86_64_GOTPC32 = 26;
        R_X86_64_GOT64 = 27;
        R_X86_64_GOTPCREL64 = 28;
        R_X86_64_GOTPC64 = 29;
        R_X86_64_GOTPLT64 = 30;
        R_X86_64_PLTOFF64 = 31;
        R_X86_64_SIZE32 = 32;
        R_X86_64_SIZE64 = 33;
        R_X86_64_GOTPC32_TLSDESC = 34;
        R_X86_64_TLSDESC_CALL = 35;
        R_X86_64_TLSDESC = 36;
        R_X86_64_IRELATIVE = 37;
        R_X86_64_RELATIVE64 = 38;
        R_X86_64_PC32_BND = 39;
        R_X86_64_PLT32_BND = 40;
        R_X86_64_GOTPCRELX = 41;
        R_X86_64_REX_GOTPCRELX = 42;
        R_X86_64_CODE_4_GOTPCRELX = 43;
        R_X86_64_CODE_4_GOTTPOFF = 44;
        R_X86_64_CODE_4_GOTPC32_TLSDESC = 45;
        R_X86_64_CODE_5_GOTPCRELX = 46;
        R_X86_64_CODE_5_GOTTPOFF = 47;
        R_X86_64_CODE_5_GOTPC32_TLSDESC = 48;
        R_X86_64_CODE_6_GOTPCRELX = 49;
        R_X86_64_CODE_6_GOTTPOFF = 50;
        R_X86_64_CODE_6_GOTPC32_TLSDESC = 51;
        R_X86_64_GNU_VTINHERIT = 250;
        R_X86_64_GNU_VTENTRY = 251;
    }
}

/// i386 psABI "Relocation". The type field of an `ELFCLASS32` relocation
/// is 8 bits wide.
pub mod i386 {
    use super::named;

    named! {
        /// The name of an i386 relocation type.
        fn name(u32);
        R_386_NONE = 0;
        R_386_32 = 1;
        R_386_PC32 = 2;
        R_386_GOT32 = 3;
        R_386_PLT32 = 4;
        R_386_COPY = 5;
        R_386_GLOB_DAT = 6;
        R_386_JUMP_SLOT = 7;
        R_386_RELATIVE = 8;
        R_386_GOTOFF = 9;
        R_386_GOTPC = 10;
        R_386_32PLT = 11;
        R_386_TLS_TPOFF = 14;
        R_386_TLS_IE = 15;
        R_386_TLS_GOTIE = 16;
        R_386_TLS_LE = 17;
        R_386_TLS_GD = 18;
        R_386_TLS_LDM = 19;
        R_386_16 = 20;
        R_386_PC16 = 21;
        R_386_8 = 22;
        R_386_PC8 = 23;
        R_386_TLS_GD_32 = 24;
        R_386_TLS_GD_PUSH = 25;
        R_386_TLS_GD_CALL = 26;
        R_386_TLS_GD_POP = 27;
        R_386_TLS_LDM_32 = 28;
        R_386_TLS_LDM_PUSH = 29;
        R_386_TLS_LDM_CALL = 30;
        R_386_TLS_LDM_POP = 31;
        R_386_TLS_LDO_32 = 32;
        R_386_TLS_IE_32 = 33;
        R_386_TLS_LE_32 = 34;
        R_386_TLS_DTPMOD32 = 35;
        R_386_TLS_DTPOFF32 = 36;
        R_386_TLS_TPOFF32 = 37;
        R_386_SIZE32 = 38;
        R_386_TLS_GOTDESC = 39;
        R_386_TLS_DESC_CALL = 40;
        R_386_TLS_DESC = 41;
        R_386_IRELATIVE = 42;
        R_386_GOT32X = 43;
        R_386_GNU_VTINHERIT = 250;
        R_386_GNU_VTENTRY = 251;
    }
}
