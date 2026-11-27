# Preserved source notices

The adapter in `lib.rs` and `port.rs` is a Rust port of `lib/fairplay_playfair.c` and its
declaration in `lib/fairplay.h`:

> Copyright (C) 2018 Juho Vähä-Herttua  
> Copyright (C) 2026 Omar Roth

The adapter source states that it is distributed under the GNU Lesser General Public License,
version 2.1 or (at your option) any later version. The original notice is retained here; the
PlayFair algorithm and data included by this combined unit carry the GPL-3.0 notice below, so the
crate is marked GPL-3.0-only.

The legacy algorithm and protocol tables are ported from `lib/playfair/playfair.c`,
`lib/playfair/omg_hax.c`, and `lib/playfair/omg_hax.h`. The full GNU GPL version 3 text from the
PlayFair source distribution is preserved in `licenses/PLAYFAIR-GPL-3.0.md`.

The independent helper crates remain separate dependencies with their own package license and
source attribution. The reference tree is not modified by table generation or the oracle.
