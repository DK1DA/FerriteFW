//! Name the parts the test image finds. The V2 board has the MCU marking erased and the SPI flash is not
//! the W25Q16 first assumed, so the image reports what the chips say about themselves.

pub const EXPECTED_PID: u32 = 0x7005_0344; // AT32F403AVGT7, from the V1 board photos

/// AT32F403A/407 reference manual V2.07, 27.4.1 (page 519).
const MCUS: [(u32, &str); 20] = [
    (0x7005_0240, "AT32F403AVCT7"), (0x7005_0241, "AT32F403ARCT7"), (0x7005_0242, "AT32F403ACCT7"),
    (0x7005_0243, "AT32F403ACCU7"), (0x7005_0344, "AT32F403AVGT7"), (0x7005_0345, "AT32F403ARGT7"),
    (0x7005_0346, "AT32F403ACGT7"), (0x7005_0347, "AT32F403ACGU7"), (0x7005_0249, "AT32F407VCT7"),
    (0x7005_024A, "AT32F407RCT7"), (0x7005_034B, "AT32F407VGT7"), (0x7005_034C, "AT32F407RGT7"),
    (0x7005_02CD, "AT32F403AVET7"), (0x7005_02CE, "AT32F403ARET7"), (0x7005_02CF, "AT32F403ACET7"),
    (0x7005_02D0, "AT32F403ACEU7"), (0x7005_02D1, "AT32F407VET7"), (0x7005_02D2, "AT32F407RET7"),
    (0x7005_0353, "AT32F407AVGT7"), (0x7005_0254, "AT32F407AVCT7"),
];

/// AT32F403A/407 part from DEBUG_IDCODE.
pub fn mcu_name(pid: u32) -> Option<&'static str> {
    MCUS.iter().find(|m| m.0 == pid).map(|m| m.1)
}

/// "Cortex-M4 r0p1" style, from SCB CPUID: implementer [31:24], variant [23:20], part [15:4], revision [3:0].
pub fn cpu_name(cpuid: u32) -> Option<&'static str> {
    const M4: [&str; 3] = ["Cortex-M4 r0p0", "Cortex-M4 r0p1", "Cortex-M4 r0p2"];
    const M3: [&str; 5] = ["Cortex-M3 r0p0", "Cortex-M3 r1p0", "Cortex-M3 r1p1", "Cortex-M3 r2p0", "Cortex-M3 r2p1"];
    if cpuid >> 24 != 0x41 {
        return None; // not ARM
    }
    let (part, var, rev) = ((cpuid >> 4) & 0xFFF, (cpuid >> 20) & 0xF, (cpuid & 0xF) as usize);
    match (part, var) {
        (0xC24, 0) if rev < 3 => Some(M4[rev]),
        (0xC23, 0) if rev == 0 => Some(M3[0]),
        (0xC23, 1) if rev <= 1 => Some(M3[1 + rev]),
        (0xC23, 2) if rev <= 1 => Some(M3[3 + rev]),
        (0xC24, _) => Some("Cortex-M4"),
        (0xC23, _) => Some("Cortex-M3"),
        _ => None,
    }
}

/// SPI NOR capacity from the JEDEC ID's third byte (log2 of the size), 0 if implausible: 64 KB to 16 MB is
/// all the 24-bit address commands can reach.
pub fn flash_bytes(jedec: u32) -> u32 {
    let c = jedec & 0xFF;
    if (0x10..=0x18).contains(&c) { 1 << c } else { 0 }
}

pub fn flash_vendor(jedec: u32) -> &'static str {
    match (jedec >> 16) & 0xFF {
        0xEF => "Winbond",
        0xC8 => "GigaDevice",
        0x85 => "Puya",
        0x68 => "Boya",
        0x0B => "XTX",
        0x20 => "XMC",
        0x5E => "Zbit",
        0xA1 => "Fudan",
        0x1C => "EON",
        0xC2 => "Macronix",
        0x9D => "ISSI",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(mcu_name(EXPECTED_PID), Some("AT32F403AVGT7"));
        assert_eq!(mcu_name(0x7005_0353), Some("AT32F407AVGT7"));
        assert_eq!(mcu_name(0x7005_0240), Some("AT32F403AVCT7"));
        assert_eq!(mcu_name(0x1003_6414), None); // an STM32F10x high-density IDCODE
        assert_eq!(mcu_name(0), None);

        assert_eq!(cpu_name(0x410F_C241), Some("Cortex-M4 r0p1"));
        assert_eq!(cpu_name(0x412F_C231), Some("Cortex-M3 r2p1"));
        assert_eq!(cpu_name(0x410F_C249), Some("Cortex-M4")); // unknown revision still names the core
        assert_eq!(cpu_name(0), None);
        assert_eq!(cpu_name(0x410F_C270), None); // Cortex-M7: not expected here
    }

    #[test]
    fn flash() {
        assert_eq!(flash_bytes(0xEF4015), 0x20_0000); // W25Q16: 2 MB
        assert_eq!(flash_bytes(0xEF4016), 0x40_0000); // 25Q32 class: 4 MB
        assert_eq!(flash_bytes(0xC84018), 0x100_0000); // 16 MB, the 24-bit address limit
        assert_eq!(flash_bytes(0xEF4019), 0); // 32 MB needs 4-byte addresses
        assert_eq!(flash_bytes(0xFF_FFFF), 0); // nothing on the bus
        assert_eq!(flash_bytes(0), 0);
        assert_eq!(flash_vendor(0xEF4015), "Winbond");
        assert_eq!(flash_vendor(0x0B4016), "XTX");
        assert_eq!(flash_vendor(0xFF_FFFF), "?");
        let vendors: std::vec::Vec<&str> =
            [0xC8, 0x85, 0x68, 0x20, 0x5E, 0xA1, 0x1C, 0xC2, 0x9D].iter().map(|&v| flash_vendor(v << 16 | 0x4016)).collect();
        assert_eq!(vendors, ["GigaDevice", "Puya", "Boya", "XMC", "Zbit", "Fudan", "EON", "Macronix", "ISSI"]);
    }

    /// Every Cortex-M3 and -M4 revision the table names, and the ones it doesn't.
    #[test]
    fn cpu_revisions() {
        assert_eq!(cpu_name(0x410F_C240), Some("Cortex-M4 r0p0"));
        assert_eq!(cpu_name(0x410F_C242), Some("Cortex-M4 r0p2"));
        assert_eq!(cpu_name(0x410F_C243), Some("Cortex-M4"), "r0p3: not in the table");
        assert_eq!(cpu_name(0x411F_C240), Some("Cortex-M4"), "r1p0");
        assert_eq!(cpu_name(0x410F_C230), Some("Cortex-M3 r0p0"));
        assert_eq!(cpu_name(0x410F_C231), Some("Cortex-M3"), "r0p1");
        assert_eq!(cpu_name(0x411F_C230), Some("Cortex-M3 r1p0"));
        assert_eq!(cpu_name(0x411F_C231), Some("Cortex-M3 r1p1"));
        assert_eq!(cpu_name(0x411F_C232), Some("Cortex-M3"), "r1p2");
        assert_eq!(cpu_name(0x412F_C230), Some("Cortex-M3 r2p0"));
        assert_eq!(cpu_name(0x413F_C230), Some("Cortex-M3"), "r3p0");
        assert_eq!(cpu_name(0x440F_C241), None, "not ARM's");
    }
}
