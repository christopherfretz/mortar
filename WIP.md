# WP-032e WIP

- done: timers (5130a38); fix 1 second placement reuses fixed point + sites + symtab plan.
- measured fix 1 (protoc, 9 interleaved): layout 62.0->40.0 ms (1T), 46.1->31.1 (8T); CPU 8T 862->806; RSS 8T 235->229.
- next: CPU growth with threads (layout 100 ms CPU at 8T for 31 wall), sites (12 ms), erratum code(), validate.
