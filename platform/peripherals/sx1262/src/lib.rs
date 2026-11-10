//! The SX1262 LoRa transceiver as a repeater [`Radio`].
#![no_std]

mod metrics;

use crate::metrics::PacketMetrics;
use defmt::{Debug2Format, debug, error};
use embedded_hal::digital::OutputPin;
use embedded_hal_async::{delay::DelayNs, digital::Wait, spi::SpiDevice};
use lora_phy::{
    LoRa, RxMode,
    iv::GenericSx126xInterfaceVariant,
    mod_params::{
        Bandwidth, CodingRate, ModulationParams, PacketParams, RadioError, SpreadingFactor,
    },
    sx126x::{self, Config, Sx126x},
};
use mcrs_repeater::{
    app::config::{self, RadioConfig},
    platform::{Radio, ReceivedPacket},
};

pub use lora_phy::sx126x::TcxoCtrlVoltage;

// MeshCore's packet format, see [`Radio`].
const ENABLE_PUBLIC_NETWORK: bool = false;
const IMPLICIT_HEADER: bool = false;
const CRC_ON: bool = true;
const IQ_INVERTED: bool = false;
/// The longest LoRa payload.
const MAX_PAYLOAD_LEN: u8 = 255;

/// How the board wires the SX1262.
pub struct BoardConfig {
    pub tcxo_ctrl: Option<TcxoCtrlVoltage>,
    pub use_dcdc: bool,
    pub rx_boost: bool,
}

impl BoardConfig {
    pub const fn heltec_v3() -> Self {
        Self {
            tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V8),
            use_dcdc: true,
            rx_boost: true,
        }
    }

    pub const fn heltec_v4() -> Self {
        Self {
            tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V8),
            use_dcdc: true,
            rx_boost: true,
        }
    }
}

type Chip<SPI, CTRL, WAIT> = Sx126x<SPI, GenericSx126xInterfaceVariant<CTRL, WAIT>, sx126x::Sx1262>;

/// An SX1262 on `spi`, with `dio1` as its interrupt. Nothing reaches the chip
/// until the repeater starts it.
pub struct Sx1262<SPI, CTRL, WAIT, DLY>
where
    SPI: SpiDevice<u8>,
    CTRL: OutputPin,
    WAIT: Wait,
    DLY: DelayNs,
{
    /// The hardware, until [`Radio::start`] hands it to the driver.
    hardware: Option<Hardware<SPI, CTRL, WAIT, DLY>>,
    started: Option<Started<SPI, CTRL, WAIT, DLY>>,
}

struct Hardware<SPI, CTRL, WAIT, DLY> {
    spi: SPI,
    reset: CTRL,
    dio1: WAIT,
    busy: WAIT,
    delay: DLY,
    board: BoardConfig,
    rx_enable: Option<CTRL>,
}

/// The driver, with the modulation and packet parameters of the repeater's
/// radio settings.
struct Started<SPI, CTRL, WAIT, DLY>
where
    SPI: SpiDevice<u8>,
    CTRL: OutputPin,
    WAIT: Wait,
    DLY: DelayNs,
{
    lora: LoRa<Chip<SPI, CTRL, WAIT>, DLY>,
    modulation_params: ModulationParams,
    rx_packet_params: PacketParams,
    tx_packet_params: PacketParams,
    transmit_power_dbm: i32,
}

impl<SPI, CTRL, WAIT, DLY> Sx1262<SPI, CTRL, WAIT, DLY>
where
    SPI: SpiDevice<u8>,
    CTRL: OutputPin,
    WAIT: Wait,
    DLY: DelayNs,
{
    pub fn new(
        spi: SPI,
        reset: CTRL,
        dio1: WAIT,
        busy: WAIT,
        rx_enable: Option<CTRL>,
        delay: DLY,
        board: BoardConfig,
    ) -> Self {
        Self {
            hardware: Some(Hardware {
                spi,
                reset,
                dio1,
                busy,
                delay,
                board,
                rx_enable,
            }),
            started: None,
        }
    }

    fn started(&mut self) -> Result<&mut Started<SPI, CTRL, WAIT, DLY>, ()> {
        self.started
            .as_mut()
            .ok_or_else(|| error!("sx1262: not started"))
    }
}

impl<SPI, CTRL, WAIT, DLY> Radio for Sx1262<SPI, CTRL, WAIT, DLY>
where
    SPI: SpiDevice<u8>,
    CTRL: OutputPin,
    WAIT: Wait,
    DLY: DelayNs,
{
    async fn start(&mut self, config: RadioConfig) -> Result<(), ()> {
        let Some(hardware) = self.hardware.take() else {
            error!("sx1262: already started");
            return Err(());
        };

        match Started::new(hardware, config).await {
            Ok(started) => {
                debug!("radio initialized");
                self.started = Some(started);
                Ok(())
            }
            Err(error) => {
                error!("failed to initialize sx1262: {:?}", Debug2Format(&error));
                Err(())
            }
        }
    }

    async fn wait_for_read(&mut self, buffer: &mut [u8]) -> Result<ReceivedPacket, ()> {
        self.started()?.wait_for_read(buffer).await
    }

    async fn channel_is_busy(&mut self) -> Result<bool, ()> {
        self.started()?.channel_is_busy().await
    }

    async fn transmit(&mut self, payload: &[u8]) -> Result<(), ()> {
        self.started()?.transmit(payload).await
    }

    async fn shutdown(&mut self) -> Result<(), ()> {
        let started = self.started()?;
        // Cancelling continuous receive leaves the chip in RX. SetSleep is
        // only valid from standby; lora-phy's sleep() does not enter it for us.
        started.lora.enter_standby().await.map_err(|error| {
            error!("sx1262 shutdown standby: {}", Debug2Format(&error));
        })?;
        started.lora.sleep(false).await.map_err(|error| {
            error!("error on shutdown sx1262: {}", Debug2Format(&error));
        })
    }
}

impl<SPI, CTRL, WAIT, DLY> Started<SPI, CTRL, WAIT, DLY>
where
    SPI: SpiDevice<u8>,
    CTRL: OutputPin,
    WAIT: Wait,
    DLY: DelayNs,
{
    async fn new(
        hardware: Hardware<SPI, CTRL, WAIT, DLY>,
        config: RadioConfig,
    ) -> Result<Self, RadioError> {
        let Hardware {
            spi,
            reset,
            dio1,
            busy,
            delay,
            board,
            rx_enable,
        } = hardware;

        let interface = GenericSx126xInterfaceVariant::new(reset, dio1, busy, rx_enable, None)?;
        let chip = Sx126x::new(
            spi,
            interface,
            Config {
                chip: sx126x::Sx1262,
                tcxo_ctrl: board.tcxo_ctrl,
                use_dcdc: board.use_dcdc,
                rx_boost: board.rx_boost,
            },
        );
        let mut lora = LoRa::new(chip, ENABLE_PUBLIC_NETWORK, delay).await?;

        let modulation_params = lora.create_modulation_params(
            spreading_factor(config.spreading_factor),
            bandwidth(config.bandwidth),
            coding_rate(config.coding_rate),
            config.receive_frequency_hz,
        )?;
        let rx_packet_params = lora.create_rx_packet_params(
            RadioConfig::PREAMBLE_LENGTH,
            IMPLICIT_HEADER,
            MAX_PAYLOAD_LEN,
            CRC_ON,
            IQ_INVERTED,
            &modulation_params,
        )?;
        let tx_packet_params = lora.create_tx_packet_params(
            RadioConfig::PREAMBLE_LENGTH,
            IMPLICIT_HEADER,
            CRC_ON,
            IQ_INVERTED,
            &modulation_params,
        )?;

        Ok(Self {
            lora,
            modulation_params,
            rx_packet_params,
            tx_packet_params,
            transmit_power_dbm: config.transmit_power_dbm,
        })
    }

    async fn wait_for_read(&mut self, buffer: &mut [u8]) -> Result<ReceivedPacket, ()> {
        self.lora
            .prepare_for_rx(
                RxMode::Continuous,
                &self.modulation_params,
                &self.rx_packet_params,
            )
            .await
            .map_err(|error| {
                error!("prepare: {}", Debug2Format(&error));
            })?;

        let (len, status) = loop {
            match self.lora.rx(&self.rx_packet_params, buffer).await {
                Ok(packet) => break packet,
                Err(error @ (RadioError::CrcError | RadioError::HeaderError)) => {
                    // Returning Err(()) triggers the app's one-second radio-failure
                    // backoff. Discard corrupt packets and immediately retry instead.
                    //
                    // todo: remove debug2format, likely caused by different defmt versions
                    error!("sx1262 receive error: {:?}", Debug2Format(&error));
                }
                Err(error) => {
                    error!("sx1262 receive error: {:?}", Debug2Format(&error));
                    return Err(());
                }
            }
        };
        let status = PacketMetrics::from(status);
        Ok(ReceivedPacket {
            len: len as usize,
            rssi: status.rssi_dbm,
            snr_quarters: status.snr_quarters,
        })
    }

    async fn channel_is_busy(&mut self) -> Result<bool, ()> {
        self.lora
            .prepare_for_cad(&self.modulation_params)
            .await
            .map_err(|error| {
                error!("CAD prepare: {}", Debug2Format(&error));
            })?;

        self.lora
            .cad(&self.modulation_params)
            .await
            .map_err(|error| {
                error!("CAD: {}", Debug2Format(&error));
            })
    }

    async fn transmit(&mut self, payload: &[u8]) -> Result<(), ()> {
        self.lora
            .prepare_for_tx(
                &self.modulation_params,
                &mut self.tx_packet_params,
                self.transmit_power_dbm,
                payload,
            )
            .await
            .map_err(|error| {
                error!("tx-prepare: {}", Debug2Format(&error));
            })?;

        self.lora.tx().await.map_err(|error| {
            error!("tx: {}", Debug2Format(&error));
        })
    }
}

fn spreading_factor(spreading_factor: config::SpreadingFactor) -> SpreadingFactor {
    match spreading_factor {
        config::SpreadingFactor::Sf5 => SpreadingFactor::_5,
        config::SpreadingFactor::Sf6 => SpreadingFactor::_6,
        config::SpreadingFactor::Sf7 => SpreadingFactor::_7,
        config::SpreadingFactor::Sf8 => SpreadingFactor::_8,
        config::SpreadingFactor::Sf9 => SpreadingFactor::_9,
        config::SpreadingFactor::Sf10 => SpreadingFactor::_10,
        config::SpreadingFactor::Sf11 => SpreadingFactor::_11,
        config::SpreadingFactor::Sf12 => SpreadingFactor::_12,
    }
}

fn bandwidth(bandwidth: config::Bandwidth) -> Bandwidth {
    match bandwidth {
        config::Bandwidth::Khz7_8 => Bandwidth::_7KHz,
        config::Bandwidth::Khz10_4 => Bandwidth::_10KHz,
        config::Bandwidth::Khz15_6 => Bandwidth::_15KHz,
        config::Bandwidth::Khz20_8 => Bandwidth::_20KHz,
        config::Bandwidth::Khz31_25 => Bandwidth::_31KHz,
        config::Bandwidth::Khz41_7 => Bandwidth::_41KHz,
        config::Bandwidth::Khz62_5 => Bandwidth::_62KHz,
        config::Bandwidth::Khz125 => Bandwidth::_125KHz,
        config::Bandwidth::Khz250 => Bandwidth::_250KHz,
        config::Bandwidth::Khz500 => Bandwidth::_500KHz,
    }
}

fn coding_rate(coding_rate: config::CodingRate) -> CodingRate {
    match coding_rate {
        config::CodingRate::Cr4_5 => CodingRate::_4_5,
        config::CodingRate::Cr4_6 => CodingRate::_4_6,
        config::CodingRate::Cr4_7 => CodingRate::_4_7,
        config::CodingRate::Cr4_8 => CodingRate::_4_8,
    }
}
