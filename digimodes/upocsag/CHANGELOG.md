# Changelog

## 0.1.0 (unreleased)

First prototype.

- `Receiver`: discriminator audio to messages at 512, 1200 and 2400 baud at once, either polarity; integer only.
- `Decoder`: codewords to messages (address, function, numeric and alphanumeric text), for hardware codeword sources.
- BCH(31,21) with parity: corrects two errors per codeword.
- `encode`: messages to codewords.
- Matches multimon-ng on 6 of 7 real recordings in 564 B of RAM and 2.0 MIPS.
- Review fixes: `Decoder` keeps an address's frame in range when `sync` is missed; uncorrectable codewords at the
  end of a message (a fade) are dropped; `encode` rejects addresses over 21 bits and functions over 3.
