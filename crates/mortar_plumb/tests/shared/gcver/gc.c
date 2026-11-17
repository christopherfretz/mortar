/* WP-149 (M10 with a version script): `gone` and `data_gone`, each in its
   own section (-ffunction-sections -fdata-sections), are referenced by
   nothing; loc.map makes them local, exp.map exports them. */
int keep(void) { return 1; }
int gone(void) { return 2; }
int data_gone = 7;
