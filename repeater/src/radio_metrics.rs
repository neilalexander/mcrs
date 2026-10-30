//! Signal units at the SX126x boundary: RSSI in dBm, SNR in quarter-dB.
use core::fmt;

/// Format quarter-dB as a decimal dB value without floating point or allocation.
#[derive(Clone, Copy, Debug)]
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

impl defmt::Format for SnrDb {
    fn format(&self, f: defmt::Formatter) {
        let magnitude = self.0.unsigned_abs();
        if self.0 < 0 {
            defmt::write!(f, "-");
        }
        defmt::write!(f, "{}.{:02}", magnitude / 4, (magnitude % 4) * 25)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PacketMetrics {
    pub rssi_dbm: i16,
    pub snr_quarters: i16,
}

impl From<lora_phy::mod_params::PacketStatus> for PacketMetrics {
    fn from(status: lora_phy::mod_params::PacketStatus) -> Self {
        Self {
            // Match MeshCore's integer cast: truncate toward zero, rather
            // than the driver's dbm() accessor which rounds down.
            rssi_dbm: status.rssi.quarter_dbm() / 4,
            snr_quarters: status.snr.quarter_db(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use embedded_hal_async::spi::{ErrorType, Operation, SpiDevice};

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

    struct IrqSpi(u16);
    impl ErrorType for IrqSpi {
        type Error = embedded_hal::spi::ErrorKind;
    }
    impl SpiDevice for IrqSpi {
        async fn transaction(
            &mut self,
            operations: &mut [Operation<'_, u8>],
        ) -> Result<(), Self::Error> {
            match operations {
                [
                    Operation::Write(command),
                    Operation::Read(status),
                    Operation::Read(data),
                ] => {
                    assert_eq!(*command, [0x12]); // GetIrqStatus
                    status.copy_from_slice(&[0x24]);
                    data.copy_from_slice(&self.0.to_be_bytes());
                }
                [Operation::Write(command)] => {
                    assert_eq!(*command, [0x02, 0xff, 0xff]); // ClearIrqStatus
                }
                _ => panic!("unexpected IRQ transaction"),
            }
            Ok(())
        }
    }

    #[test]
    fn pinned_driver_rejects_crc_and_header_errors_before_rx_done() {
        use lora_phy::{RxMode, mod_params::RadioMode, mod_traits::IrqState};

        for (flags, expected_error) in [
            (0x0002, None),
            (0x0042, Some(RadioError::CrcError)),
            (0x0020, Some(RadioError::HeaderError)),
            (0x0022, Some(RadioError::HeaderError)),
            (0x0062, Some(RadioError::HeaderError)),
        ] {
            let mut driver = Sx126x::new(
                IrqSpi(flags),
                MockInterface,
                Config {
                    chip: Sx1262,
                    tcxo_ctrl: None,
                    use_dcdc: true,
                    rx_boost: true,
                },
            );
            let result =
                run(driver.process_irq_event(RadioMode::Receive(RxMode::Continuous), None, true));
            match expected_error {
                Some(expected) => assert!(matches!(result, Err(error) if error == expected)),
                None => assert!(matches!(result, Ok(Some(IrqState::Done)))),
            }
        }
    }

    #[test]
    fn reads_precise_metrics_from_the_pinned_driver() {
        for (raw_snr, quarters) in [(33, 33), (223, -33), (255, -1)] {
            let mut driver = Sx126x::new(
                MockSpi {
                    raw: [187, raw_snr, 200],
                },
                MockInterface,
                Config {
                    chip: Sx1262,
                    tcxo_ctrl: None,
                    use_dcdc: true,
                    rx_boost: true,
                },
            );
            let status = run(driver.get_rx_packet_status()).unwrap();
            assert_eq!(
                PacketMetrics::from(status),
                PacketMetrics {
                    rssi_dbm: -93,
                    snr_quarters: quarters,
                }
            );
        }
    }

    fn metrics(raw: [u8; 3]) -> PacketMetrics {
        use lora_phy::mod_params::{PacketStatus, Rssi, Snr};
        PacketMetrics::from(PacketStatus {
            rssi: Rssi::from_quarter_dbm(-i16::from(raw[0]) * 2),
            snr: Snr::from_quarter_db(i16::from(raw[1] as i8)),
        })
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
                metrics([187, raw, 200]),
                PacketMetrics {
                    rssi_dbm: -93,
                    snr_quarters: expected
                }
            );
        }
        assert_eq!(metrics([186, 0, 0]).rssi_dbm, -93);
        assert_eq!(metrics([188, 0, 0]).rssi_dbm, -94);
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
