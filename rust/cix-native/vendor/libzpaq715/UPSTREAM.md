# libzpaq 7.15 source provenance

This directory is the source closure used by CIX's native CIXB1 ZPAQ backend.

- Upstream project: ZPAQ / libzpaq by Matt Mahoney.
- Requested tag: `7.15`.
- Pinned source revision: `9ab539f644e364f0d92e2918b90ce2534c75653f`.
- Imported from a pinned source receipt for that revision; no input data was
  acquired for this import.
- Upstream `libzpaq.cpp` SHA-256 before the CIX safety patch:
  `151eb6bd83cb6c6f5261d64b1db49358710f844ee1a2aa4b9cb63e17319df122`.
- Vendored patched `libzpaq.cpp` SHA-256:
  `11e1025a290c420359def7485422dfa4296045c7cf55f809e1e034cc0855d381`.
- `libzpaq.h` SHA-256:
  `08bd9ce17ce018468e35721e2c6a8bd13c0c5e397ce4e9c90c52aec389662f79`.
- License: public-domain libzpaq code, with the embedded libdivsufsort-lite component under the MIT notice retained in libzpaq.cpp. See ../../THIRD_PARTY_NOTICES.txt for redistribution notices.

The header's historical banner says 7.12; the pinned 7.15 program source and
the retained source receipt identify the selected release. CIX records the
runtime backend version as `7.15` and never accepts an unversioned ZPAQ CIXB1
backend identifier.

`libzpaq.cpp` carries one local, auditable safety patch: its ZPAQL interpreter
calls `cix_zpaq715_charge_instruction()` after each non-HALT instruction. The
CIX FFI resets a one-million-instruction watchdog at every HCOMP/PCOMP program
invocation (and at decoded output). This prevents malformed archive programs
from spinning forever while allowing valid long BWT/PCOMP blocks; it leaves
the encoded format and built-in level-5 encoder unchanged.
