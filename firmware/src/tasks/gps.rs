//! GPS: powers the module (PA8 low), reads NMEA from USART3, publishes the position.

use crate::board::GPS_PWR_N;
use crate::chan::POSITION;
use crate::hal::uart;
use crate::report::UART_GPS;
use embassy_time::Timer;
use ferrite_core::msg::GpsFix;
use nmea::Nmea;

/// The ATGM336H's default rate.
const BAUD: u32 = 9600;

#[embassy_executor::task]
pub async fn task() {
    GPS_PWR_N.write(false);
    uart::init(UART_GPS, BAUD);
    let mut nmea = Nmea::default();
    let tx = POSITION.sender();
    loop {
        let mut fresh = false;
        while let Some(c) = uart::getc(UART_GPS) {
            fresh |= nmea.feed(c);
        }
        if fresh {
            tx.send(GpsFix {
                on: true,
                fix: if nmea.has_fix() { 2 } else { 0 },
                lat: nmea.lat_e7,
                lon: nmea.lon_e7,
                used: nmea.sats_used,
                ..GpsFix::default()
            });
        }
        Timer::after_millis(50).await; // 960 bytes/s at most: the 1 KB ring holds a second
    }
}
