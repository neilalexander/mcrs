use embedded_hal::digital::OutputPin;
use embedded_hal_async::{delay::DelayNs, digital::Wait, spi::SpiDevice};
use lora_phy::{
    LoRa, RxMode,
    iv::GenericSx126xInterfaceVariant,
    mod_params::{
        Bandwidth, CodingRate, ModulationParams, PacketParams, RadioError, SpreadingFactor,
    },
    sx126x::{Config, Sx126x, Sx1262, TcxoCtrlVoltage},
};

pub const ENABLE_PUBLIC_NETWORK: bool = false;
pub const RECEIVE_PREAMBLE_LENGTH: u16 = 8;
pub const RECEIVE_MAX_PAYLOAD_LEN: u8 = 255;

pub struct BoardConfig {
    pub tcxo_ctrl: Option<TcxoCtrlVoltage>,
    pub use_dcdc: bool,
    pub rx_boost: bool,
}

impl BoardConfig {
    #[allow(dead_code)]
    pub const fn heltec_v3() -> Self {
        Self {
            tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V8),
            use_dcdc: false,
            rx_boost: true,
        }
    }

    #[allow(dead_code)]
    pub const fn heltec_v4() -> Self {
        Self {
            tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V8),
            use_dcdc: false,
            rx_boost: true,
        }
    }
}

pub type Interface<CTRL, WAIT> = GenericSx126xInterfaceVariant<CTRL, WAIT>;
pub type Driver<SPI, CTRL, WAIT> = Sx126x<SPI, Interface<CTRL, WAIT>, Sx1262>;
pub type Radio<SPI, CTRL, WAIT, DLY> = LoRa<Driver<SPI, CTRL, WAIT>, DLY>;

pub struct ReceiveConfig {
    pub frequency_hz: u32,
    pub spreading_factor: SpreadingFactor,
    pub bandwidth: Bandwidth,
    pub coding_rate: CodingRate,
    pub preamble_length: u16,
    pub max_payload_len: u8,
    pub crc_on: bool,
    pub iq_inverted: bool,
    pub transmit_output_power_dbm: i32,
}

pub struct Receiver<SPI, CTRL, WAIT, DLY>
where
    SPI: SpiDevice<u8>,
    CTRL: OutputPin,
    WAIT: Wait,
    DLY: DelayNs,
{
    radio: Radio<SPI, CTRL, WAIT, DLY>,
    modulation_params: ModulationParams,
    rx_packet_params: PacketParams,
    tx_packet_params: PacketParams,
    transmit_output_power_dbm: i32,
}

pub async fn init<SPI, CTRL, WAIT, DLY>(
    spi: SPI,
    reset: CTRL,
    dio1: WAIT,
    busy: WAIT,
    delay: DLY,
    board: BoardConfig,
) -> Result<Radio<SPI, CTRL, WAIT, DLY>, RadioError>
where
    SPI: SpiDevice<u8>,
    CTRL: OutputPin,
    WAIT: Wait,
    DLY: DelayNs,
{
    let interface = Interface::new(reset, dio1, busy, None, None)?;
    let driver = Driver::new(
        spi,
        interface,
        Config {
            chip: Sx1262,
            tcxo_ctrl: board.tcxo_ctrl,
            use_dcdc: board.use_dcdc,
            rx_boost: board.rx_boost,
        },
    );

    LoRa::new(driver, ENABLE_PUBLIC_NETWORK, delay).await
}

pub async fn init_receiver<SPI, CTRL, WAIT, DLY>(
    spi: SPI,
    reset: CTRL,
    dio1: WAIT,
    busy: WAIT,
    delay: DLY,
    board: BoardConfig,
    receive: ReceiveConfig,
) -> Result<Receiver<SPI, CTRL, WAIT, DLY>, RadioError>
where
    SPI: SpiDevice<u8>,
    CTRL: OutputPin,
    WAIT: Wait,
    DLY: DelayNs,
{
    let mut radio = init(spi, reset, dio1, busy, delay, board).await?;
    let modulation_params = radio.create_modulation_params(
        receive.spreading_factor,
        receive.bandwidth,
        receive.coding_rate,
        receive.frequency_hz,
    )?;
    let rx_packet_params = radio.create_rx_packet_params(
        receive.preamble_length,
        false,
        receive.max_payload_len,
        receive.crc_on,
        receive.iq_inverted,
        &modulation_params,
    )?;
    let tx_packet_params = radio.create_tx_packet_params(
        receive.preamble_length,
        false,
        receive.crc_on,
        receive.iq_inverted,
        &modulation_params,
    )?;

    Ok(Receiver {
        radio,
        modulation_params,
        rx_packet_params,
        tx_packet_params,
        transmit_output_power_dbm: receive.transmit_output_power_dbm,
    })
}

impl<SPI, CTRL, WAIT, DLY> crate::modules::Receiver for Receiver<SPI, CTRL, WAIT, DLY>
where
    SPI: SpiDevice<u8>,
    CTRL: OutputPin,
    WAIT: Wait,
    DLY: DelayNs,
{
    async fn wait_for_read(
        &mut self,
        buffer: &mut [u8],
    ) -> Result<crate::modules::ReceivedPacket, ()> {
        self.radio
            .prepare_for_rx(
                RxMode::Continuous,
                &self.modulation_params,
                &self.rx_packet_params,
            )
            .await
            .map_err(|error| {
                crate::platform::log_radio_receive_error("prepare", radio_error_name(&error));
            })?;

        let (len, status) = self
            .radio
            .rx(&self.rx_packet_params, buffer)
            .await
            .map_err(|error| {
                crate::platform::log_radio_receive_error("rx", radio_error_name(&error));
            })?;

        Ok(crate::modules::ReceivedPacket {
            len: len as usize,
            rssi: status.rssi,
            snr: status.snr,
        })
    }

    async fn channel_is_busy(&mut self) -> Result<bool, ()> {
        self.radio
            .prepare_for_cad(&self.modulation_params)
            .await
            .map_err(|error| {
                crate::platform::log_radio_receive_error("CAD prepare", radio_error_name(&error));
            })?;

        self.radio
            .cad(&self.modulation_params)
            .await
            .map_err(|error| {
                crate::platform::log_radio_receive_error("CAD", radio_error_name(&error));
            })
    }

    async fn transmit(&mut self, payload: &[u8]) -> Result<(), ()> {
        self.radio
            .prepare_for_tx(
                &self.modulation_params,
                &mut self.tx_packet_params,
                self.transmit_output_power_dbm,
                payload,
            )
            .await
            .map_err(|error| {
                crate::platform::log_radio_receive_error("tx-prepare", radio_error_name(&error));
            })?;

        self.radio.tx().await.map_err(|error| {
            crate::platform::log_radio_receive_error("tx", radio_error_name(&error));
        })
    }
}

fn radio_error_name(error: &RadioError) -> &'static str {
    match error {
        RadioError::SPI => "spi",
        RadioError::Reset => "reset",
        RadioError::RfSwitchRx => "rf-switch-rx",
        RadioError::RfSwitchTx => "rf-switch-tx",
        RadioError::Busy => "busy",
        RadioError::Irq => "irq",
        RadioError::DIO1 => "dio1",
        RadioError::InvalidConfiguration => "invalid-configuration",
        RadioError::InvalidRadioMode => "invalid-radio-mode",
        RadioError::OpError(_) => "op-error",
        RadioError::InvalidBaseAddress(_, _) => "invalid-base-address",
        RadioError::PayloadSizeUnexpected(_) => "payload-size-unexpected",
        RadioError::PayloadSizeMismatch(_, _) => "payload-size-mismatch",
        RadioError::UnavailableSpreadingFactor => "unavailable-spreading-factor",
        RadioError::UnavailableBandwidth => "unavailable-bandwidth",
        RadioError::InvalidBandwidthForFrequency => "invalid-bandwidth-for-frequency",
        RadioError::InvalidSF6ExplicitHeaderRequest => "invalid-sf6-explicit-header-request",
        RadioError::InvalidOutputPowerForFrequency => "invalid-output-power-for-frequency",
        RadioError::TransmitTimeout => "transmit-timeout",
        RadioError::ReceiveTimeout => "receive-timeout",
        RadioError::DutyCycleUnsupported => "duty-cycle-unsupported",
        RadioError::RngUnsupported => "rng-unsupported",
    }
}
