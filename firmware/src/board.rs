//! RT-950 Pro board: every pin the firmware uses, the safe state and start-up. Source: the RT-950 Pro
//! Firmware Reference and analysis/pinmap.md (stock V0.27). (L) = role is likely, not yet confirmed.
//! Carried over from the prototype (fw-rs), which mirrors fw/board.
use crate::hal::gpio::{pin, Mode, Pin, GPIOA, GPIOB, GPIOC, GPIOD, GPIOE};
use crate::hal::{clock, gpio, sys, time, tone};

// RF: all active-high path selects, plus the PA drive (DAC2 output, held at 0 V as a GPIO). Only the Radio
// task raises a receive path; no transmit path is raised in a build without the tx feature.
pub const RF_PINS: [Pin; 7] = [
    pin(GPIOB, 0),  // VHF RX
    pin(GPIOB, 1),  // VHF TX
    pin(GPIOE, 12), // UHF RX
    pin(GPIOE, 7),  // UHF TX
    pin(GPIOE, 13), // HF RX
    pin(GPIOE, 14), // HF TX
    pin(GPIOA, 5),  // PA drive
];
pub const BK_CS: [Pin; 2] = [pin(GPIOE, 8), pin(GPIOE, 15)]; // BK4829 #0 (VHF), #1 (UHF/HF)
pub const BK_SCK: Pin = pin(GPIOE, 10);
pub const BK_SDA: Pin = pin(GPIOE, 11);

// Audio
pub const AMP_EN: [Pin; 2] = [pin(GPIOB, 8), pin(GPIOE, 4)]; // (L) both

// Power
pub const POWER_HOLD: Pin = pin(GPIOA, 11); // (L) set at start-up by stock, cleared to switch off
pub const POWER_SW: Pin = pin(GPIOE, 0); // power switch sense: low = on, high = switched off (stock: release PA11, reset)

// SPI flash on SPI2 (PB13 SCK, PB14 MISO, PB15 MOSI). Writes only inside ferrite_core::layout::WRITABLE.
pub const FLASH_CS: Pin = pin(GPIOB, 12);

// Broadcast receiver SI4732: bit-banged I2C, 7-bit address 0x11
pub const SI_SCL: Pin = pin(GPIOB, 6);
pub const SI_SDA: Pin = pin(GPIOB, 7);
pub const SI4732_ADDR: u8 = 0x11;
pub const SI_RST: Pin = pin(GPIOA, 6); // SI4732 reset, active low: stock holds it low from boot (pin_audit.md)

// GPS on USART3 (PB10 TX, PB11 RX); Bluetooth module on USART1 (PA9 TX, PA10 RX)
pub const GPS_PWR_N: Pin = pin(GPIOA, 8); // low = GPS powered
pub const BT_PWR: Pin = pin(GPIOB, 9); // (L) high = module powered

// Display: ST7789V, 8080 8-bit, data on PD8-PD15
pub const LCD_WR: Pin = pin(GPIOD, 0);
pub const LCD_CS: Pin = pin(GPIOD, 1);
pub const LCD_RST: Pin = pin(GPIOD, 2);
pub const LCD_DC: Pin = pin(GPIOD, 3);
pub const LCD_DATA_PORT: u32 = GPIOD;

// Indicators
pub const LED_RED: Pin = pin(GPIOC, 13);
pub const LED_GREEN: Pin = pin(GPIOC, 14);
pub const BACKLIGHT: [Pin; 2] = [pin(GPIOC, 6), pin(GPIOB, 3)]; // (L) both
pub const LED_ACTIVE_HIGH: bool = true; // (L) flip if the LEDs turn out active-low

// Keys: matrix drive PC0-PC3, sense PD4-PD7 (active-low, pull-ups)
pub const KEY_DRIVE: [Pin; 4] = [pin(GPIOC, 0), pin(GPIOC, 1), pin(GPIOC, 2), pin(GPIOC, 3)];
pub const KEY_SENSE: [Pin; 4] = [pin(GPIOD, 7), pin(GPIOD, 6), pin(GPIOD, 5), pin(GPIOD, 4)]; // column 0 = PD7
pub const KEY_DIRECT0: Pin = pin(GPIOE, 5); // key code 0
pub const KEY_DIRECT1: Pin = pin(GPIOA, 12); // key code 1
// Stock keys the transmitter from PE3 (PTT A), PE2 (PTT B) and PE6 (PTT on the current VFO, likely the side
// connector's pin 9), and from PE5 when that key is set to PTT C. Any TX code must unkey only when all are
// released. This image never transmits; it only reports them.
pub const KEY_PTT: Pin = pin(GPIOE, 3); // PTT A
pub const KEY_SIDE1: Pin = pin(GPIOE, 2); // PTT B
pub const KEY_SIDE2: Pin = pin(GPIOE, 6); // PTT, current VFO (connector PTT, L)
pub const ENC_A: Pin = pin(GPIOB, 4);
pub const ENC_B: Pin = pin(GPIOB, 5);

const SAFE_LOW: [Pin; 9] = [
    pin(GPIOC, 4),
    pin(GPIOA, 7),
    pin(GPIOC, 5),
    pin(GPIOE, 9),
    pin(GPIOC, 12),
    pin(GPIOC, 9),
    pin(GPIOE, 1),
    pin(GPIOA, 3),
    pin(GPIOB, 2),
];

/// First code after reset and in every fault path: every RF path off, PA drive at 0 V, amps off, chips
/// deselected, power held. Touches no RAM.
pub fn safe_state() {
    gpio::clocks_on();
    // PA drive: hold the DAC in reset so its output cannot override the GPIO below.
    tone::dac_reset();
    for p in RF_PINS {
        p.output(false, Mode::OutPp);
    }
    for p in AMP_EN {
        p.output(false, Mode::OutPp);
    }
    for p in BK_CS {
        p.output(true, Mode::OutPp);
    }
    BK_SCK.output(false, Mode::OutPp);
    FLASH_CS.output(true, Mode::OutPp);
    // Pins stock leaves low that sit next to RF or TX audio: band relay PC4, broadcast-receiver input paths
    // PA7/PC5, PE9 (BK4829/RF cluster), APRS TX audio PC12, audio switches PC9/PE1, and the unused PA3/PB2.
    // Normally the bootloader already left them low; set them so a fault or a start without the bootloader
    // never leaves them floating. PA15 (high in stock RX) is left alone.
    for p in SAFE_LOW {
        p.output(false, Mode::OutPp);
    }
    POWER_HOLD.output(true, Mode::OutPp);
}

/// Called from `__pre_init`, before RAM is initialised (interrupts already masked): the safe state, then
/// the bootloader's SysTick and interrupts off.
pub fn before_ram() {
    safe_state();
    sys::quiesce();
}

/// Everything else the image needs, before any task runs.
pub fn init() {
    clock::init();
    time::init();
    gpio::jtag_off();
    LED_RED.output(!LED_ACTIVE_HIGH, Mode::OutPp);
    LED_GREEN.output(!LED_ACTIVE_HIGH, Mode::OutPp);
    for p in BACKLIGHT {
        p.output(false, Mode::OutPp);
    }
    POWER_SW.config(Mode::InPu);
    SI_RST.output(false, Mode::OutPp); // SI4732 held in reset until its driver needs it
    // GPS and Bluetooth modules start powered down; the test pages switch them on.
    GPS_PWR_N.output(true, Mode::OutPp);
    BT_PWR.output(false, Mode::OutPp);
    pin(GPIOB, 10).config(Mode::InFloat);
    pin(GPIOA, 9).config(Mode::InFloat);
    // BK4829 data line idle as an input; the SI4732 bus idle high.
    BK_SDA.config(Mode::InPu);
    SI_SCL.config(Mode::InPu);
    SI_SDA.config(Mode::InPu);
}

/// Any fault, stray interrupt or panic: interrupts off first so nothing else runs, then the safe state,
/// red LED on, stop.
pub fn fault() -> ! {
    fault_at(None)
}

/// [`fault`], with the faulting address when known (a HardFault's stacked PC): the bring-up build prints it on
/// the cable after the safe state and the LED.
pub fn fault_at(pc: Option<u32>) -> ! {
    cortex_m::interrupt::disable();
    safe_state();
    LED_RED.output(LED_ACTIVE_HIGH, Mode::OutPp);
    crate::bringup::fault(pc);
    loop {}
}
