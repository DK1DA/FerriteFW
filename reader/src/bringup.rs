//! The fault path's report (firmware/src/board.rs calls it after the safe state): straight to the cable, polled.
pub fn fault(pc: Option<u32>) {
    let uart = crate::report::UART_CPS;
    crate::hal::uart::write(uart, b"\r\n*** FAULT: safe state, red LED, stopped");
    if let Some(pc) = pc {
        let mut h = *b"; pc 0x00000000";
        for i in 0..8 {
            h[7 + i] = b"0123456789ABCDEF"[(pc >> ((7 - i) * 4)) as usize & 15];
        }
        crate::hal::uart::write(uart, &h);
    }
    crate::hal::uart::write(uart, b"\r\n");
}
