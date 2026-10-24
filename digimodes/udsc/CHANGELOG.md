# Changelog

## 0.1.0 (unreleased)

First prototype.

- VHF DSC receiver on uafsk's FSK front end (now `uafsk::fsk`, with a fourth raw-amplitude slicer).
- Framer: correlation phasing, DX/RX nearest-codeword merge, ECC check and single-symbol repair, fallback end
  detection.
- ITU-R M.493 fields: format, address (MMSI or area), category, self-ID, telecommands, working channel, distress
  alerts with acknowledgements and relays (nature, position, time), position calls.
- uafsk's 1.4-bit correlation window and 85% PLL: in noise, 3 dB now decodes 215 and 218 of 220 calls (flat,
  de-emphasised) and 6 dB all 220 (were 202 and 200 at 6 dB); 40 more bytes of RAM, about 4% more CPU.
