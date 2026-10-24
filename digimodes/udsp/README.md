# udsp

Integer DSP building blocks for microcontrollers, shared by the decoders in
[udigimodes](../../README.md). `no_std`, no allocation, no floating point at run time.

| Module | What | Cost |
| --- | --- | --- |
| `trig` | One quarter-wave sine table (1,025 values) serving every FFT twiddle, window and oscillator (interpolated for any frequency); `const` sin/cos for building other tables at compile time | 2 KB flash |
| `fft` | Radix-2 complex FFT on 16-bit values with block floating point (halves only when a value could overflow, and counts it); spectrum of a real frame | in place, no extra RAM |
| `log` | log2 from count-leading-zeros and a 33-entry table, for decibel levels | 66 B flash |
| `mix` | Mixer and decimating FIR filter: shift a band to 0 Hz and lower the rate; `lowpass` designs the filter at compile time | delay line: `L` complex samples |
| `fir` | Decimating FIR filter for real signals | delay line: `L` samples |
| `goertzel` | Power at one frequency over a block, as a share of the block's energy (tone detection) | 12 B per frequency |
| `arm` | The Cortex-M4 DSP instructions behind the packed kernels (SMUSD, SMUADX, SMLAD), and `cycles()`, a DWT cycle counter for timing a kernel on the target | none |

## On a Cortex-M4/M7

Built for an ARMv7E-M target (`thumbv7em-*`), the FFT and the mixer's filter use the M4's dual 16-bit multiplies,
picked by the build script (stable Rust doesn't expose `target_feature = "dsp"` on ARM, so the target name
decides). Everywhere else they are plain Rust. **The results are bit-identical**: the packed kernels compute the
same 32-bit products and sums in a different order, and the halving test is the same comparison.

- **FFT** (`fft::cfft`): each (re, im) pair is one 32-bit load and store, the complex multiply is SMUSD + SMUADX,
  twiddles are packed once per stage, the halving test reads two values per load and stops at the first large one,
  and the inner loop has no bounds checks. 1.6-2.1x fewer instructions (128 to 1,024 points; the low end is a
  quiet signal, where every halving test reads the whole block).
- **Mixer filter** (`mix::FrontEnd`): two taps per step, the pair's real and imaginary parts regrouped (PKHBT,
  PKHTB) for SMLAD.
- **Left plain, on purpose:** the real-signal FIR (`fir::Decimator`), which LLVM's ParallelDSP pass already turns
  into unrolled SMLAD (a hand-written loop measured slower), and Goertzel, whose recurrence is one 32 x 32 -> 64
  multiply per sample with no 16-bit pairs.

Whole decoders on a Cortex-M4 (instructions counted in an emulator by each crate's `tools/cpu-bench`, same decodes
before and after):

| Decoder | Before | After |
| --- | --- | --- |
| uft48, FT8 capture (mixer, filter, FFT), per 80 ms hop | 150.8 k | 113.6 k (-25 %) |
| uwspr, capture, per 12 kHz sample | 162 | 131 (-19 %) |
| uwspr, decode of one slot | 255 M | 250 M (-2 %) |

Checks: `tests/packed.rs` runs the packed kernels (instructions emulated, `--features packed`) against the plain
ones on random and edge-case inputs; `tools/check-m4` does the same with the real instructions in the Unicorn
emulator (`cargo build --release && python3 run.py`, needs `unicorn` and `pyelftools`) and counts each kernel's
instructions; with `--features udsp/packed`, every decoder's recording-based report gives the same numbers as with
the plain kernels. Features: `plain` keeps the plain kernels on the target (to compare on hardware), `packed`
runs the packed kernels anywhere with emulated instructions.

Tests check each against floating point: the FFT against a direct DFT (within 0.2 dB), the log within 0.01, the
oscillator within 1e-4, the mixer's output frequency, both filters' stopbands (beyond 40 dB) and the Goertzel
filter's share for on- and off-frequency tones.

[PolyForm Noncommercial 1.0.0](../../LICENSE.md).
