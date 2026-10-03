//! Plain values the HAL reports (carried over from the prototype).
#![allow(dead_code)] // read by the System screen and the link, which come later

/// Option bytes as stored, plus the live protection flag (hal::sys).
#[derive(Clone, Copy)]
pub struct SysOpt {
    pub fap: u16,
    pub ssb: u16,
    pub eopb0: u16,
    pub fap_active: bool,
}

/// Per serial port (hal::uart).
#[derive(Clone, Copy)]
pub struct UartStats {
    pub rx_bytes: u32,
    pub tx_bytes: u32,
    pub dropped: u32,
    pub line_errors: u32,
}

pub const UART_BT: usize = 0; // USART1
pub const UART_GPS: usize = 1; // USART3
pub const UART_CPS: usize = 2; // UART4, the programming cable
pub const UART_COUNT: usize = 3;
