//! The MCU's clock tree as plain numbers (PWR-24.9, decision 250): the core speed is a runtime state, 120 MHz
//! (the build's speed) normally and 24 MHz in Low Power Mode, so every divider the firmware programs is worked out
//! here from a [`Tree`], at either speed, and tested on the PC. The firmware's `hal::clock` holds the current tree
//! and does the register work; this module never touches a register.
//!
//! The tree is stock's (V0.27 clock setup 0x08022694): the 8 MHz crystal through the PLL, AHB /1, both APBs /2 up
//! to 120 MHz (/4 above, the APB limit), timers at twice their APB. Also here: the order of the switch
//! ([`switch_steps`]) and the time driver's bookkeeping across it ([`Elapsed`]).

/// The crystal (HEXT).
pub const HEXT_HZ: u32 = 8_000_000;
/// The embassy tick (the time driver's TMR3 rate).
pub const TICK_HZ: u32 = 2_000;
/// Stock's speed: SYSCLK at or below it keeps stock's dividers.
pub const STOCK_HZ: u32 = 120_000_000;
/// At or below this SYSCLK the APBs are undivided ([`Tree::apb_div`]).
const LOW_APB1_HZ: u32 = 24_000_000;
/// The cable link's fastest rate (`link::RATES`): every tree must reach it.
pub const CABLE_MAX_BAUD: u32 = 921_600;
/// Low Power Mode's speed (PWR-24.9): PLL x3.
pub const LOW: Tree = Tree::new(3);
/// The middle of a switch: SYSCLK from the crystal, the APBs undivided ([`Step::ApbUndivided`]). Only its UART
/// divisors are used (decision 362: the ports keep receiving while the PLL locks).
pub const CRYSTAL: Tree = Tree::new(1);

/// One clock tree: SYSCLK = HEXT x `mult` through the PLL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tree {
    pub mult: u32,
}

impl Tree {
    pub const fn new(mult: u32) -> Self {
        Tree { mult }
    }

    pub const fn sysclk(self) -> u32 {
        HEXT_HZ * self.mult
    }

    /// Whole MHz (every tree here is a multiple of 8 MHz).
    pub const fn mhz(self) -> u32 {
        self.sysclk() / 1_000_000
    }

    /// Both APBs: /2 up to 120 MHz (stock's), /4 above (the APB limit is 120 MHz, and timers at twice the APB must
    /// keep the time driver's 2 kHz prescaler within 16 bits), /1 at 24 MHz and below: at 12 MHz the cable's fastest
    /// link rate, 921600 baud, would need a divisor under 16 (at 24 MHz it is 26).
    pub const fn apb_div(self) -> u32 {
        if self.sysclk() > STOCK_HZ {
            4
        } else if self.sysclk() <= LOW_APB1_HZ {
            1
        } else {
            2
        }
    }

    pub const fn apb_hz(self) -> u32 {
        self.sysclk() / self.apb_div()
    }

    /// The timer clock on either APB: twice the bus clock when the APB divides, the bus clock when it doesn't
    /// (RM 4.1); so SYSCLK up to 120 MHz, 120 MHz above.
    pub const fn tmr_hz(self) -> u32 {
        if self.apb_div() == 1 {
            self.apb_hz()
        } else {
            2 * self.apb_hz()
        }
    }

    /// CRM_CFG's PLL fields (RM 4.3.2): PLLRC (16) from HEXT, PLLMULT (18-21 and 29-30), PLLRANGE (31) above 72 MHz.
    /// The multiplier code: x2..x15 are 0..13, x17..x64 are 16..63.
    pub const fn pll_cfg(self) -> u32 {
        let code = if self.mult <= 15 { self.mult - 2 } else { self.mult - 1 };
        let range = if self.sysclk() > 72_000_000 { 1 << 31 } else { 0 };
        range | 1 << 16 | (code & 0xF) << 18 | (code >> 4) << 29
    }

    /// CRM_CFG's APB2 (bits 11-13) and APB1 (bits 8-10) dividers: 0b000 is /1, 0b100 /2, 0b101 /4; AHB stays /1.
    pub const fn apb_cfg(self) -> u32 {
        let code = match self.apb_div() {
            1 => 0b000,
            2 => 0b100,
            _ => 0b101,
        };
        code << 11 | code << 8
    }

    /// A timer's prescaler (TMR_DIV) to count at `hz`.
    pub const fn tmr_div(self, hz: u32) -> u32 {
        self.tmr_hz() / hz - 1
    }

    /// A timer's period in counts for `hz` with no prescaler, rounded (TMR4's ADC trigger, TMR6's DAC trigger).
    pub const fn reload(self, hz: u32) -> u32 {
        (self.tmr_hz() + hz / 2) / hz
    }

    /// A USART's BAUDR for `baud` (every port is on an APB: both run at [`Tree::apb_hz`]).
    pub const fn baudr(self, baud: u32) -> u32 {
        (self.apb_hz() + baud / 2) / baud
    }

    /// Whether `baud` is reached within 2 % (the divisor is 16 at least).
    pub const fn baud_ok(self, baud: u32) -> bool {
        let div = self.baudr(baud);
        div >= 16 && (self.apb_hz() / div).abs_diff(baud) * 50 <= baud
    }

    /// SPI2's MDIV code (the clock is APB1 / 2^(code+1)): the fastest at or under 15 MHz, stock's /4 at 120 MHz.
    pub const fn spi_mdiv(self) -> u32 {
        let mut n = 0;
        while self.apb_hz() >> (n + 1) > 15_000_000 {
            n += 1;
        }
        n
    }

    /// CRM_CFG's ADCDIV code: APB2 / 6 (stock's, 10 MHz at 120), or / 2 when APB2 is 20 MHz or less so a
    /// 252-cycle conversion still fits the audio capture's 104 us sample period.
    pub const fn adc_div_code(self) -> u32 {
        if self.apb_hz() <= 20_000_000 {
            0b00
        } else {
            0b10
        }
    }

    pub const fn adc_hz(self) -> u32 {
        self.apb_hz() / if self.adc_div_code() == 0 { 2 } else { 6 }
    }

    /// The LCD strobe's nop blocks are counted at 120 MHz: once at that speed or below, more often above.
    pub const fn lcd_nops(self) -> u32 {
        self.sysclk().div_ceil(STOCK_HZ)
    }

    /// Every divider this tree needs fits: the APB limit, the 2 kHz tick, the ADC's limit, the SPI's, the
    /// cable's rates up to [`CABLE_MAX_BAUD`], and the 1 MHz microsecond counter.
    pub const fn valid(self) -> bool {
        self.mult >= 2
            && self.mult <= 64
            && self.mult != 16
            && self.apb_hz() <= 120_000_000
            && self.tmr_hz().is_multiple_of(TICK_HZ)
            && self.tmr_hz() / TICK_HZ <= 65_536
            && self.tmr_hz().is_multiple_of(1_000_000)
            && self.adc_hz() <= 28_000_000
            && self.apb_hz() >> (self.spi_mdiv() + 1) <= 15_000_000
            && self.baud_ok(115_200)
            && self.baud_ok(CABLE_MAX_BAUD)
    }
}

/// One step of the speed switch, in the order [`switch_steps`] gives (AT32F403A RM 4.2: the PLL's settings
/// change only while it is off, and it is off only while SYSCLK runs from something else). There are no flash
/// wait states to set on this part (its first 256 KB is zero-wait at any speed; stock's clock setup never
/// touches the flash controller, and the 240 MHz bench image runs without), so no step before going up or after
/// coming down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// AUTO_STEP on: stock sets it around the switch to the PLL so the core's current steps instead of jumping
    /// (the RM asks for it above 108 MHz); here it is on for the whole switch, both ways.
    AutoStepOn,
    /// SYSCLK from the crystal (8 MHz), waiting until the switch shows in SCLKSTS.
    SysclkHext,
    /// Both APBs undivided while SYSCLK is the crystal ([`CRYSTAL`]), so the UARTs run from 8 MHz whichever way
    /// the switch goes (decision 362).
    ApbUndivided,
    /// PLL off.
    PllOff,
    /// The target's APB dividers (set at 8 MHz just before SYSCLK goes back to the PLL, so no bus ever runs over
    /// its limit).
    ApbDividers,
    /// The target's PLL multiplier and range.
    PllConfig,
    /// PLL on, waiting for PLLSTBL.
    PllOnWait,
    /// SYSCLK from the PLL, waiting until it shows.
    SysclkPll,
    /// AUTO_STEP off again.
    AutoStepOff,
}

/// The register steps of a switch to any speed (either way).
pub const fn switch_steps() -> [Step; 9] {
    [
        Step::AutoStepOn,
        Step::SysclkHext,
        Step::ApbUndivided,
        Step::PllOff,
        Step::PllConfig,
        Step::PllOnWait,
        Step::ApbDividers,
        Step::SysclkPll,
        Step::AutoStepOff,
    ]
}

/// The time driver across a switch: TMR3 (the embassy tick) is stopped at a tick edge, the time the switch took is
/// counted in core cycles at each of the three speeds it runs at (the old tree, the crystal, the new tree), and the
/// counter is moved on by the whole ticks that passed. The part of a tick left over is carried to the next
/// switch, so the error never adds up: the tick stays within one tick (0.5 ms) of true time, however many switches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Elapsed {
    /// Picoseconds of a tick not yet counted, carried from the last switch.
    pub carry_ps: u64,
}

/// One tick, picoseconds.
pub const TICK_PS: u64 = 1_000_000_000_000 / TICK_HZ as u64;

impl Elapsed {
    /// Cycles `old` at `old_hz`, `hext` at the crystal's 8 MHz and `new` at `new_hz` went by while TMR3 was
    /// stopped: the whole ticks to add (the rest carried).
    pub fn ticks(&mut self, old: u32, old_hz: u32, hext: u32, new: u32, new_hz: u32) -> u32 {
        let ps = |c: u32, hz: u32| c as u64 * 1_000_000_000_000 / hz as u64;
        let total = self.carry_ps + ps(old, old_hz) + ps(hext, HEXT_HZ) + ps(new, new_hz);
        self.carry_ps = total % TICK_PS;
        (total / TICK_PS) as u32
    }
}

/// Whether TMR3's 16-bit count `cnt` is a safe place to stop it for a switch: far enough from the two half-period
/// marks (0x8000, the compare, and 0x0000, the overflow) that the ticks added after can't step over one. A switch
/// is well under 1 ms (2 ticks); 16 ticks of margin.
pub const fn tick_count_safe(cnt: u16) -> bool {
    let low = cnt & 0x7FFF;
    low >= 1 && low <= 0x7FFF - 16
}

/// PWR-24.9, PWR-24.10: in Low Power Mode, how long until the core may run at 24 MHz (0: now). It waits
/// `settle_ms` after the mode comes on, and `boost_ms` after the last key or knob while the screen is lit (a dark
/// screen drops at once). `on_for` and `since_input` are milliseconds.
pub fn lpm_low_in(on_for: u32, since_input: u32, dark: bool, settle_ms: u32, boost_ms: u32) -> u32 {
    let boost = if dark { 0 } else { boost_ms.saturating_sub(since_input) };
    settle_ms.saturating_sub(on_for).max(boost)
}

/// PWR-26 (decision 252): why the core runs at the build's speed outside Low Power Mode, one bit each. The App
/// gathers the first six (`App::clock_reasons`), the firmware adds the link and audio ones.
pub mod why {
    /// PWR-26.1: a key or the knob within [`super::INPUT_MS`], the screen lit.
    pub const INPUT: u16 = 1;
    /// PWR-26.1: APRS mode, KISS, or a decoder wanted (AFSK, POCSAG, SAME).
    pub const DECODE: u16 = 1 << 1;
    /// PWR-26.1: a scan on any slot.
    pub const SCAN: u16 = 1 << 2;
    /// PWR-26.1: an exclusive mode: band scope, tone search, counter, FM broadcast, HF mode, crossband repeater.
    pub const MODE: u16 = 1 << 3;
    /// PWR-26.1: keyed, or the Transmit (or emergency) screen.
    pub const TX: u16 = 1 << 4;
    /// PWR-26.1: the alert screen.
    pub const ALERT: u16 = 1 << 5;
    /// PWR-26.1: a live link session on the cable or Bluetooth (PWR-26.2).
    pub const LINK: u16 = 1 << 6;
    /// PWR-26.4: audio capture or playback running, or about to start (a task waiting for the speed).
    pub const AUDIO: u16 = 1 << 7;
    /// PWR-26.1, SAT-6: satellite passes being predicted.
    pub const PREDICT: u16 = 1 << 8;
    /// Their names, bit 0 first (the cable log and the readback's `why`).
    pub const NAMES: [&str; 9] = ["input", "decode", "scan", "mode", "tx", "alert", "link", "audio", "predict"];
}

/// PWR-26.1: a key or the knob keeps the build's speed this long (as Low Power Mode's boost, PWR-24.10).
pub const INPUT_MS: u32 = 3000;
/// PWR-26.3: the build's speed lasts at least this long after the last reason clears.
pub const HOLD_MS: u32 = 1000;
/// PWR-26.2: a byte from the cable or a connected Bluetooth device within this long makes the link live.
pub const LINK_MS: u32 = 5000;

/// PWR-26.3: the governor: 24 MHz (true) only with no reason now and none for [`HOLD_MS`]. `last_high_ms` is when
/// a reason was last seen (the caller sets it to `now` whenever `reasons` isn't 0).
pub fn governor(reasons: u16, last_high_ms: u32, now: u32) -> bool {
    reasons == 0 && now.wrapping_sub(last_high_ms) >= HOLD_MS
}

/// PWR-26: the speed to switch to this turn: Some(true) 24 MHz, Some(false) the build's, None stay as it is.
/// Audio running (`audio_busy`) never switches (PWR-26.4); Fixed (0 9 9 4) is the build's speed always (PWR-26.5);
/// in Low Power Mode its own timing decides (`lpm_wait`, [`lpm_low_in`]: PWR-24.9, PWR-24.10); otherwise the
/// governor.
pub fn clock_target(fixed: bool, lpm_wait: Option<u32>, reasons: u16, last_high_ms: u32, now: u32, audio_busy: bool) -> Option<bool> {
    if audio_busy {
        return None;
    }
    Some(match (fixed, lpm_wait) {
        (true, _) => false,
        (false, Some(w)) => w == 0,
        (false, None) => governor(reasons, last_high_ms, now),
    })
}

/// PWR-26.8, SAT-6: the build's speed raised to the top clock (240 MHz) for this turn: satellite passes being
/// predicted, in Dynamic (not Fixed, PWR-26.5), out of Low Power Mode (PWR-24.9) and with no audio running
/// (PWR-26.4). Only where [`clock_target`] gives the build's speed.
pub fn top_wanted(fixed: bool, lpm_wait: Option<u32>, reasons: u16, audio_busy: bool) -> bool {
    !fixed && lpm_wait.is_none() && !audio_busy && reasons & why::PREDICT != 0
}

/// PWR-26.1: the input reason: a key or the knob `since_input` ms ago, the screen lit.
pub fn input_holds(since_input: u32, dark: bool) -> bool {
    !dark && since_input < INPUT_MS
}

#[cfg(test)]
mod tests {

    /// PWR-24.10: a key or the knob holds full speed for the boost; the dark screen or the boost's end gives 24 MHz.
    #[test]
    fn lpm_boost_after_input() {
        assert_eq!(lpm_low_in(500, 10_000, false, 1000, 3000), 500, "the mode settling first");
        assert_eq!(lpm_low_in(5000, 1000, false, 1000, 3000), 2000, "a key a second ago: two seconds more");
        assert_eq!(lpm_low_in(5000, 1000, true, 1000, 3000), 0, "dark: at once");
        assert_eq!(lpm_low_in(5000, 3000, false, 1000, 3000), 0, "the boost over");
    }

    /// PWR-26.3: no reason for a second gives 24 MHz; any reason, or one cleared under a second ago, keeps 120.
    #[test]
    fn governor_hysteresis() {
        assert!(!governor(why::SCAN, 0, 10_000), "a reason now");
        assert!(!governor(0, 10_000, 10_500), "cleared half a second ago");
        assert!(governor(0, 10_000, 11_000), "cleared a second ago");
        assert!(governor(0, u32::MAX - 100, 1_000), "across the ms counter's wrap");
        assert!(!governor(0, u32::MAX - 100, 500), "across the wrap, under a second");
    }

    /// PWR-26.1: a key or the knob holds the speed for 3 s while the screen is lit; dark ends it.
    #[test]
    fn governor_input() {
        assert!(input_holds(0, false) && input_holds(2_999, false));
        assert!(!input_holds(3_000, false), "3 s on");
        assert!(!input_holds(100, true), "dark screen");
        assert_eq!(why::NAMES.len(), 9);
        assert_eq!(why::PREDICT, 1 << (why::NAMES.len() - 1));
    }

    /// PWR-26.4: audio running never switches, whatever else wants; PWR-26.5: Fixed is the build's speed always;
    /// PWR-24.9, PWR-26: Low Power Mode keeps its own timing; otherwise the governor.
    #[test]
    fn clock_target_order() {
        assert_eq!(clock_target(false, None, 0, 0, 5_000, true), None, "audio: stay");
        assert_eq!(clock_target(true, Some(0), 0, 0, 5_000, true), None, "audio, even Fixed");
        assert_eq!(clock_target(true, Some(0), 0, 0, 5_000, false), Some(false), "Fixed: the build's, in the mode too");
        assert_eq!(clock_target(true, None, 0, 0, 5_000, false), Some(false));
        assert_eq!(clock_target(false, Some(0), why::LINK, 5_000, 5_000, false), Some(true), "the mode decides alone");
        assert_eq!(clock_target(false, Some(200), 0, 0, 5_000, false), Some(false), "the mode settling or boosting");
        assert_eq!(clock_target(false, None, 0, 0, 5_000, false), Some(true), "rest: 24 MHz");
        assert_eq!(clock_target(false, None, why::DECODE, 5_000, 5_000, false), Some(false), "a reason: the build's");
        assert_eq!(clock_target(false, None, 0, 4_500, 5_000, false), Some(false), "the hold");
    }
    use super::*;

    /// PWR-26.8: predicting raises Dynamic to 240 MHz; Fixed, Low Power Mode, audio or no prediction don't.
    #[test]
    fn top_clock_only_for_prediction() {
        assert!(top_wanted(false, None, why::PREDICT | why::INPUT, false));
        assert!(!top_wanted(false, None, why::INPUT, false), "a key alone: the build's speed");
        assert!(!top_wanted(true, None, why::PREDICT, false), "Fixed");
        assert!(!top_wanted(false, Some(0), why::PREDICT, false), "Low Power Mode");
        assert!(!top_wanted(false, None, why::PREDICT, true), "audio running");
        assert!(Tree::new(30).valid() && Tree::new(30).mhz() == 240 && Tree::new(30).apb_hz() == Tree::new(15).apb_hz());
    }

    const NORMAL: Tree = Tree::new(15);
    /// The `clock-*` bench builds' trees, plus the two runtime speeds.
    const ALL: [u32; 7] = [3, 6, 12, 15, 18, 24, 30];

    /// PWR-24.9: stock's tree at 120 MHz, exactly the values the fixed-clock image used (PLL 0x80350000, APB /2,
    /// timers 120 MHz, SPI2 /4, ADC /6, 2 kHz tick from /60000, 25 kHz backlight in 4800 counts).
    #[test]
    fn stock_tree_at_120() {
        assert_eq!(NORMAL.sysclk(), 120_000_000);
        assert_eq!(NORMAL.pll_cfg(), 0x8035_0000);
        assert_eq!(NORMAL.apb_cfg(), 0b100 << 11 | 0b100 << 8);
        assert_eq!(NORMAL.tmr_hz(), 120_000_000);
        assert_eq!(NORMAL.tmr_div(TICK_HZ), 59_999);
        assert_eq!(NORMAL.tmr_div(1_000_000), 119);
        assert_eq!(NORMAL.spi_mdiv(), 1);
        assert_eq!(NORMAL.adc_div_code(), 0b10);
        assert_eq!(NORMAL.adc_hz(), 10_000_000);
        assert_eq!(NORMAL.tmr_hz() / 25_000, 4_800);
        assert_eq!(NORMAL.baudr(115_200), 521);
        assert_eq!(NORMAL.reload(48_000), 2_500);
        assert_eq!(NORMAL.lcd_nops(), 1);
    }

    /// PWR-24.9: Low Power Mode's 24 MHz tree: PLL x3 (code 1, no range bit), APBs undivided at 24 MHz, timers
    /// 24 MHz, the tick from /12000, SPI2 /2 = 12 MHz, ADC /6 = 4 MHz, 960 backlight counts, every cable rate up to
    /// 921600 within 0.2 %.
    #[test]
    fn low_power_tree_at_24() {
        assert_eq!(LOW.sysclk(), 24_000_000);
        assert_eq!(LOW.pll_cfg(), 1 << 16 | 1 << 18);
        assert_eq!(LOW.apb_hz(), 24_000_000);
        assert_eq!(LOW.apb_cfg(), 0);
        assert_eq!(LOW.tmr_hz(), 24_000_000);
        assert_eq!(LOW.tmr_div(TICK_HZ), 11_999);
        assert_eq!(LOW.tmr_div(1_000_000), 23);
        assert_eq!(LOW.spi_mdiv(), 0);
        assert_eq!(LOW.adc_div_code(), 0b10);
        assert_eq!(LOW.adc_hz(), 4_000_000);
        assert_eq!(LOW.tmr_hz() / 25_000, 960);
        assert_eq!(LOW.baudr(115_200), 208);
        assert_eq!(LOW.baudr(921_600), 26);
        for b in [9_600, 38_400, 57_600, 115_200, 230_400, 460_800, 921_600] {
            assert!(LOW.baud_ok(b), "{b}");
        }
        assert_eq!(LOW.reload(48_000), 500);
        assert_eq!(LOW.lcd_nops(), 1);
        // afsk-bk's bit timer (TERM-33): 1.2 MHz exactly.
        assert!(LOW.tmr_hz().is_multiple_of(1_200_000));
    }

    /// PWR-24.9: every bench tree (`clock-*`) and both runtime speeds are valid, and the APBs never pass 120 MHz.
    #[test]
    fn every_tree_fits() {
        for m in ALL {
            let t = Tree::new(m);
            assert!(t.valid(), "x{m}");
            assert!(t.apb_hz() <= 120_000_000);
        }
        assert_eq!(Tree::new(30).apb_cfg(), 0b101 << 11 | 0b101 << 8);
        assert_eq!(Tree::new(30).tmr_hz(), 120_000_000);
        assert_eq!(Tree::new(6).apb_hz(), 24_000_000); // 48 MHz: /2 as stock
        assert_eq!(Tree::new(6).tmr_hz(), 48_000_000);
    }

    /// PWR-24.9: the switch's order: the automatic step on around both SYSCLK changes, off the PLL before it goes
    /// off, the dividers and multiplier set while it is off and SYSCLK is the crystal, the PLL stable before SYSCLK
    /// goes back to it. PWR-26.2.1 (decision 362): the APBs undivided for the whole crystal part, the target's dividers last.
    #[test]
    fn switch_order() {
        let s = switch_steps();
        let at = |x: Step| s.iter().position(|&y| y == x).unwrap();
        assert_eq!(s[0], Step::AutoStepOn);
        assert_eq!(s[8], Step::AutoStepOff);
        assert!(at(Step::AutoStepOn) < at(Step::SysclkHext));
        assert!(at(Step::SysclkHext) < at(Step::PllOff));
        assert!(at(Step::PllOff) < at(Step::PllConfig));
        assert!(at(Step::SysclkHext) < at(Step::ApbUndivided));
        assert!(at(Step::ApbUndivided) < at(Step::PllOnWait));
        assert!(at(Step::PllOnWait) < at(Step::ApbDividers));
        assert!(at(Step::ApbDividers) < at(Step::SysclkPll));
        assert!(at(Step::PllConfig) < at(Step::PllOnWait));
        assert!(at(Step::PllOnWait) < at(Step::SysclkPll));
        assert!(at(Step::AutoStepOn) < at(Step::SysclkPll));
        assert!(at(Step::SysclkPll) < at(Step::AutoStepOff));
        assert_eq!(s.len(), 9);
    }

    /// PWR-24.9: time across switches: whole ticks added, the rest carried, so 50 round trips leave the tick less
    /// than one tick from true time.
    #[test]
    fn time_keeps_across_switches() {
        let mut e = Elapsed::default();
        // 10 us at 120 MHz, 300 us on the crystal, 7 us at 24 MHz: 317 us, no whole tick yet.
        assert_eq!(e.ticks(1_200, 120_000_000, 2_400, 168, 24_000_000), 0);
        assert_eq!(e.carry_ps, 317_000_000);
        // And again: 634 us, one tick, 134 us carried.
        assert_eq!(e.ticks(1_200, 120_000_000, 2_400, 168, 24_000_000), 1);
        assert_eq!(e.carry_ps, 134_000_000);
        let mut e = Elapsed::default();
        let (mut ticks, mut true_ps) = (0u64, 0u64);
        for i in 0..100u32 {
            let (a, b, c) = (1_000 + i * 37, 2_000 + i * 11, 500 + i * 3);
            let (f0, f1) = if i % 2 == 0 { (120_000_000, 24_000_000) } else { (24_000_000, 120_000_000) };
            ticks += e.ticks(a, f0, b, c, f1) as u64;
            true_ps += a as u64 * 1_000_000_000_000 / f0 as u64
                + b as u64 * 1_000_000_000_000 / HEXT_HZ as u64
                + c as u64 * 1_000_000_000_000 / f1 as u64;
        }
        let counted = ticks * TICK_PS;
        assert!(counted <= true_ps && true_ps - counted < TICK_PS);
    }

    /// PWR-24.9: TMR3 is stopped only away from its half-period marks.
    #[test]
    fn tick_count_margin() {
        assert!(!tick_count_safe(0x0000));
        assert!(!tick_count_safe(0x8000));
        assert!(!tick_count_safe(0x7FFF));
        assert!(!tick_count_safe(0xFFF5));
        assert!(tick_count_safe(0x0001));
        assert!(tick_count_safe(0x8001));
        assert!(tick_count_safe(0x4000));
        assert!(tick_count_safe(0x7FEF));
        assert!(!tick_count_safe(0x7FF0));
    }

    /// The trees worked out at run time (the firmware's are consts): PLLMULT's code below and above x16 (RM
    /// 4.3.2), PLLRANGE only above 72 MHz, the ADC at APB2 / 2 when APB2 is 20 MHz or less, else / 6.
    #[test]
    fn tree_fields() {
        let x2 = Tree::new(2);
        assert_eq!((x2.mhz(), x2.apb_hz(), x2.adc_div_code(), x2.adc_hz()), (16, 16_000_000, 0, 8_000_000));
        assert_eq!(x2.pll_cfg(), 1 << 16, "x2 is code 0, no range bit");
        let low = LOW;
        assert_eq!((low.mhz(), low.adc_div_code(), low.adc_hz()), (24, 0b10, 4_000_000));
        assert_eq!(low.pll_cfg(), 1 << 16 | 1 << 18);
        let full = Tree::new(15);
        assert_eq!(full.pll_cfg(), 1 << 31 | 1 << 16 | 13 << 18, "x15 is code 13");
        let top = Tree::new(30);
        assert_eq!(top.mhz(), 240);
        assert_eq!(top.pll_cfg(), 1 << 31 | 1 << 16 | (29 & 0xF) << 18 | 1 << 29, "x30 is code 29");
    }
}
