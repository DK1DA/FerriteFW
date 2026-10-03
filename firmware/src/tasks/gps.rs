//! GPS: powers the module (PA8 low), reads NMEA from USART3, publishes the position.

use crate::board::GPS_PWR_N;
use crate::chan::POSITION;
use crate::hal::uart;
use crate::report::UART_GPS;
use embassy_time::Timer;
use ferrite_core::msg::Position;
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
            let hms = nmea.utc_hms;
            tx.send(Position {
                fix: nmea.has_fix(),
                lat: nmea.lat_e7,
                lon: nmea.lon_e7,
                sats: nmea.sats_used,
                time_s: hms / 10000 * 3600 + hms / 100 % 100 * 60 + hms % 100,
            });
        }
        Timer::after_millis(50).await; // 960 bytes/s at most: the 1 KB ring holds a second
    }
}
