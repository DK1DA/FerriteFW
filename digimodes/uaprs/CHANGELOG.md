# Changelog

## 0.1.0 (unreleased)

First prototype.

- AX.25 UI frames: addresses, paths, parsing and building, TNC2 text.
- APRS parsing, zero copy: plain, compressed, Mic-E and NMEA positions; DAO; messages, acks, rejections and
  reply-acks; objects and items; status; weather (with and without position, Ultimeter); capabilities;
  telemetry; third-party traffic.
- Building position reports (plain and compressed), messages, acks and rejections. (The telemetry report and
  definition builders were removed Oct 27, 2026: Ferrite puts its values in the position comment.)
- KISS encoder and decoder; smart beaconing; distance and bearing.
- Agrees with Ham::APRS::FAP field by field on 926 of 934 packets.
- Review fixes: no integer overflow on hostile packets (Mic-E altitude, Ultimeter, long NMEA numbers); NMEA
  positions and timestamps range-checked; DAO keeps the hemisphere at 0°; compressed courses of 358-359° no longer
  sent as a range; builders keep data-like comments from being read as data; KISS drops frames cut mid-escape;
  smart beaconing safe with any settings; SSID masked when written.
- Smaller: `Frame::emit_tnc2` without `core::fmt`; hex without `from_str_radix`; `nmea` and `ultimeter` features.
- KISS TNC support: `kiss::Params` (host settings, flags for uafsk) and `kiss::Access` (p-persistence).
