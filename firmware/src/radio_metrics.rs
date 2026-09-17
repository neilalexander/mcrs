//! Signal units at the SX126x boundary: RSSI in dBm, SNR in quarter-dB.
use alloc::rc::Rc;
use core::{cell::Cell, fmt};
use embedded_hal_async::spi::{ErrorType, Operation, SpiDevice};

/// Format quarter-dB as a decimal dB value without floating point or allocation.
pub struct SnrDb(pub i16);
impl fmt::Display for SnrDb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let magnitude = self.0.unsigned_abs();
        if self.0 < 0 {
            f.write_str("-")?;
        }
        write!(f, "{}.{:02}", magnitude / 4, (magnitude % 4) * 25)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PacketMetrics {
    pub rssi_dbm: i16,
    pub snr_quarters: i16,
}
impl PacketMetrics {
    fn from_status(status: &[u8]) -> Self {
        Self {
            // Match RadioLib's -raw/2.0 followed by MeshCore's integer cast:
            // truncate toward zero rather than rounding negative values down.
            rssi_dbm: -(i16::from(status[0])) / 2,
            snr_quarters: i16::from(status[1] as i8),
        }
    }
}

#[derive(Clone, Default)]
pub struct PacketStatus(Rc<Cell<Option<PacketMetrics>>>);
impl PacketStatus {
    pub fn take(&self) -> Option<PacketMetrics> {
        self.0.take()
    }
}

/// Preserve GetPacketStatus's raw values before lora-phy 3.0.1 rounds them to
/// whole dB. This observes the existing read, without another SPI transaction
/// (which could pick up a different packet in continuous RX mode), and leaves
/// the driver's response untouched. The transaction shape is covered by tests.
pub struct StatusSpi<SPI> {
    inner: SPI,
    status: PacketStatus,
}
impl<SPI> StatusSpi<SPI> {
    pub fn new(inner: SPI, status: PacketStatus) -> Self {
        Self { inner, status }
    }
}
impl<SPI: ErrorType> ErrorType for StatusSpi<SPI> {
    type Error = SPI::Error;
}
impl<SPI: SpiDevice<u8>> SpiDevice<u8> for StatusSpi<SPI> {
    async fn transaction(
        &mut self,
        operations: &mut [Operation<'_, u8>],
    ) -> Result<(), Self::Error> {
        let packet_status =
            matches!(operations.first(), Some(Operation::Write(command)) if *command == [0x14]);
        if packet_status {
            self.status.take();
        }
        self.inner.transaction(operations).await?;
        if packet_status {
            if let [
                Operation::Write(_),
                Operation::Read(status),
                Operation::Read(data),
            ] = operations
            {
                if status.len() == 1 && data.len() == 3 {
                    self.status.0.set(Some(PacketMetrics::from_status(data)));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    use lora_phy::{
        mod_params::RadioError,
        mod_traits::{InterfaceVariant, RadioKind},
        sx126x::{Config, Sx126x, Sx1262},
    };

    fn run<T>(future: impl Future<Output = T>) -> T {
        let mut future = pin!(future);
        let mut context = Context::from_waker(Waker::noop());
        for _ in 0..100 {
            if let Poll::Ready(result) = future.as_mut().poll(&mut context) {
                return result;
            }
        }
        panic!("SPI operation did not finish");
    }
    struct MockSpi {
        raw: [u8; 3],
        fail: bool,
    }
    impl ErrorType for MockSpi {
        type Error = embedded_hal::spi::ErrorKind;
    }
    impl SpiDevice for MockSpi {
        async fn transaction(
            &mut self,
            operations: &mut [Operation<'_, u8>],
        ) -> Result<(), Self::Error> {
            let mut pending = true;
            core::future::poll_fn(|cx| {
                if pending {
                    pending = false;
                    cx.waker().wake_by_ref();
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            })
            .await;
            if self.fail {
                return Err(embedded_hal::spi::ErrorKind::Other);
            }
            let [
                Operation::Write(command),
                Operation::Read(status),
                Operation::Read(data),
            ] = operations
            else {
                panic!("unexpected driver transaction");
            };
            assert_eq!(*command, [0x14]);
            status.copy_from_slice(&[0x24]);
            data.copy_from_slice(&self.raw);
            Ok(())
        }
    }
    struct MockInterface;
    impl InterfaceVariant for MockInterface {
        async fn reset(
            &mut self,
            _: &mut impl embedded_hal_async::delay::DelayNs,
        ) -> Result<(), RadioError> {
            Ok(())
        }
        async fn wait_on_busy(&mut self) -> Result<(), RadioError> {
            Ok(())
        }
        async fn await_irq(&mut self) -> Result<(), RadioError> {
            Ok(())
        }
        async fn enable_rf_switch_rx(&mut self) -> Result<(), RadioError> {
            Ok(())
        }
        async fn enable_rf_switch_tx(&mut self) -> Result<(), RadioError> {
            Ok(())
        }
        async fn disable_rf_switch(&mut self) -> Result<(), RadioError> {
            Ok(())
        }
    }

    #[test]
    fn captures_the_actual_lora_phy_status_read_before_rounding() {
        for (raw_snr, quarters, rounded_snr) in [(33, 33, 8), (223, -33, -8), (255, -1, 0)] {
            let captured = PacketStatus::default();
            let spi = StatusSpi::new(
                MockSpi {
                    raw: [187, raw_snr, 200],
                    fail: false,
                },
                captured.clone(),
            );
            let mut driver = Sx126x::new(
                spi,
                MockInterface,
                Config {
                    chip: Sx1262,
                    tcxo_ctrl: None,
                    use_dcdc: true,
                    rx_boost: true,
                },
            );
            let rounded = run(driver.get_rx_packet_status()).unwrap();
            // lora-phy sees exactly the original response; our metrics retain
            // the quarter-dB precision it discards and use MeshCore RSSI rounding.
            assert_eq!(rounded.snr, rounded_snr);
            assert_eq!(rounded.rssi, -94);
            assert_eq!(
                captured.take(),
                Some(PacketMetrics {
                    rssi_dbm: -93,
                    snr_quarters: quarters
                })
            );
            assert_eq!(captured.take(), None);
        }
    }

    #[test]
    fn failed_status_read_does_not_reuse_previous_metrics() {
        let captured = PacketStatus::default();
        captured.0.set(Some(PacketMetrics {
            rssi_dbm: -90,
            snr_quarters: 32,
        }));
        let mut spi = StatusSpi::new(
            MockSpi {
                raw: [187, 33, 200],
                fail: true,
            },
            captured.clone(),
        );
        let mut status = [0];
        let mut data = [0; 3];
        assert!(
            run(spi.transaction(&mut [
                Operation::Write(&[0x14]),
                Operation::Read(&mut status),
                Operation::Read(&mut data)
            ]))
            .is_err()
        );
        assert_eq!(captured.take(), None);
    }

    #[test]
    fn retains_signed_quarter_db_precision_and_matches_meshcore_rssi() {
        for (raw, expected) in [
            (32, 32),
            (33, 33),
            (1, 1),
            (0, 0),
            (255, -1),
            (223, -33),
            (128, -128),
            (127, 127),
        ] {
            assert_eq!(
                PacketMetrics::from_status(&[187, raw, 200]),
                PacketMetrics {
                    rssi_dbm: -93,
                    snr_quarters: expected
                }
            );
        }
        assert_eq!(PacketMetrics::from_status(&[186, 0, 0]).rssi_dbm, -93);
        assert_eq!(PacketMetrics::from_status(&[188, 0, 0]).rssi_dbm, -94);
    }

    #[test]
    fn displays_db_including_negative_fractional_values() {
        for (quarters, expected) in [
            (32, "8.00"),
            (33, "8.25"),
            (34, "8.50"),
            (35, "8.75"),
            (-1, "-0.25"),
            (-33, "-8.25"),
            (0, "0.00"),
            (i16::MIN, "-8192.00"),
        ] {
            assert_eq!(format!("{}", SnrDb(quarters)), expected);
        }
    }
}
