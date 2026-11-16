- **Q12. Closed (the review's M-7 ruling).** A `.symver` definition
  `foo@V1` is hidden exactly when node V1's own `local:` matches `foo`
  (GNU ld and lld; mold keeps it, §17.8), with the warning
  `versioned-definition-hidden`; a `local:` of another node leaves it
  exported (all three, `exp2/vscript2`). resolve §9.7 rule 12.
