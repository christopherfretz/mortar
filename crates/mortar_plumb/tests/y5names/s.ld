/* WP-210's script for tests/y5names.rs: names assigned at the top
   level, inside SECTIONS, by PROVIDE, and one no input refers to. */
SECTIONS {
  . = 0x400000 + SIZEOF_HEADERS;
  .text : { *(.text) }
  . = ALIGN(0x1000);
  .got : { *(.got) }
  .data : { *(.data) }
  my_end = .;
  PROVIDE(my_prov = .);
}
my_abs = 0x1234;
my_free = 0x10;
