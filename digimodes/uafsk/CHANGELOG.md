# Changelog

## 0.1.0 (unreleased)

First prototype.

- Bell 202 demodulator, integer only, modelled on Dire Wolf: 1200 Hz high-pass, one-bit I/Q correlators, per-tone
  AGC, up to three slicers and two chains (`Demodulator<S, C>`), digital PLL, NRZI.
- HDLC deframer (unstuffing, CRC-16/X.25 check, 330-byte frames) and framer.
- Phase-continuous modulator as a sample iterator.
- Decodes 94-99% of Dire Wolf's frames on the WA8LMF test CD in 1,236 B of RAM and 3.4 MIPS.
- Review fixes: a steady tone no longer overflows the deframer's ones counter; the modulator always sends at least
  one flag before and after a frame.
- Faster and smaller with identical decodes: 32-bit divides, a ring of samples instead of products, the frame's own
  FCS as the duplicate key (`Deframer::push_fcs`): 5.4 -> 3.4 MIPS, 1,468 -> 1,236 B.
- Carrier detect, `Demodulator::busy`, for channel access (12 bytes of RAM).
- `fsk` module: the FSK front end on its own (any tone pair; used by udsc); a fourth slicer on raw amplitudes (`Demodulator<4, 1>`: 949/964/947/958 on WA8LMF, against 938/956/943/958 with 3).
- Sensitivity: the correlators sum over 1.4 bits instead of one (`Config::window`), and the PLL pulls to 85%
  instead of 74%. Off-air recordings at 12 kS/s: 205 -> 360 and 332 -> 379 frames (Dire Wolf: 353 and 367); WA8LMF
  938/956/943/958 -> 986/988/978/990. Same CPU, 40 more bytes of RAM (1,288 B). Carrier detect also needs both
  tones to come and go, so a steady tone between them no longer reads as busy.
- Repair (`Config::repair`, 3 by default): each tone decision carries its margin (`Fsk::push_soft`,
  `Deframer::push_soft`); a frame with a bad FCS gets its 3 least confident decisions flipped (7 combinations),
  kept only if the stuffing is unchanged and the AX.25 addresses are sane. Off-air recordings at 12 kS/s: 360 -> 393
  and 379 -> 386 frames, 319 -> 333 and 362 -> 363 of Dire Wolf's 353 and 367; at 16 kS/s 368 -> 406 and 376 -> 389;
  WA8LMF 986/988/978/990 -> 997/1007/998/1009. 0 wrong FCS among the repaired frames (independent CRC). 60 more bytes
  of RAM (1,348 B), 345 instructions a sample instead of 335. Tapered correlation windows and deciding from the
  margin summed over the middle of the bit were tried and not kept (docs/measurements.md).
- FX.25 receive (`Config::fx25`, on by default): an FX.25 receiver per slicer on the same bits; the block is
  collected, not corrected, in `push`, and handed out once (`take_fx25`, a 260-byte `Fx25Block` to correct
  elsewhere with `Fx25Block::decode`, or `decode_fx25` in place); the plain copy's FCS goes with it, so a frame
  heard both ways is marked `duplicate`. Dire Wolf's noise files: 41/44/46 frames against 36/37/38 plain. RAM
  1,348 -> 2,460 B, 345 -> 380 instructions a sample; off-air and WA8LMF counts unchanged.
- FX.25 RAM: one block buffer shared by the slicers instead of a receiver each and a block for the caller. A
  16-byte tag hunter per slicer (`ufx25::Hunter`); the first slicer to find a tag claims the buffer (a lower
  slicer finding the same tag within 16 bits takes over), and the complete block stays there until released
  (`pending_fx25`, `release_fx25`; `take_fx25` copies it out and releases; `fx25_skipped` counts tags that came
  while it was busy). `Fx25Block` wraps a `ufx25::Collector` (264 bytes; `mode()`, `tag_errors()`, `block()`).
  RAM 2,460 -> 1,668 B, 372 -> 367 instructions a sample; Dire Wolf's noise files (41/44/46, no tag skipped),
  off-air and WA8LMF counts unchanged.
- `Modulator` is generic over its bits: `Modulator::from_bits` sends any bit sequence (an FX.25 transmission);
  `Modulator::new` as before.
