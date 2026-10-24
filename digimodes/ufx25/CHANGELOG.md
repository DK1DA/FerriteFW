# Changelog

## 0.1.0 (unreleased)

First prototype.

- Reed-Solomon over GF(2^8) (0x11D, roots α^1 … α^r): systematic encoder, Berlekamp-Massey / Chien / Forney
  decoder that reports the bytes corrected and rejects what it cannot correct; FX.25's shortened layout (zeros
  between data and check bytes, measured against Dire Wolf).
- The eleven FX.25 codes of the specification's Table 1; the tags checked against the specification's Gold-code
  description.
- `Encoder`: AX.25 frame to FX.25 bits (smallest block for 16, 32 or 64 check bytes). `Receiver`: streaming,
  one bit at a time, tags matched with up to 8 wrong bits (as Dire Wolf), 272 bytes of RAM.
- Interoperates with Dire Wolf both ways (`gen_packets -X` and `atest -d x`), including corrections up to t.
- Hunting only behind a flag (one of the three bytes before the 64 bits; or right after a block): 243 -> 53
  instructions a bit on noise, the same decodes on Dire Wolf's files. 8 more bytes per receiver (280 B).
- `Receiver::push_raw` (collect only), `Receiver::block`, `Receiver::collecting` and `ufx25::decode`, to correct a
  block outside the audio path.
- `Receiver` split into `Hunter` (the tag hunt, 16 bytes: the 24 bits behind the newest 64 kept in a `u32`) and
  `Collector` (the 255-byte block until cleared, `Copy`), so slicers can share one block buffer; `Receiver` is the
  two together, same API and decodes (280 B).
