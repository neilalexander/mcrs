//! This example runs on the RAK4631 WisBlock, which has an nRF52840 MCU and Semtech Sx126x radio.
//! Other nrf/sx126x combinations may work with appropriate pin modifications.
//! It demonstrates LORA get instant RSSI while in receive/listen mode.
#![no_std]
#![no_main]

extern crate alloc;

use defmt::*;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::{bind_interrupts, peripherals, spim};
use embassy_time::{Delay, Timer};
use embedded_alloc::LlffHeap as Heap;
use embedded_hal_bus::spi::ExclusiveDevice;
use lora_phy::LoRa;
use lora_phy::iv::GenericSx126xInterfaceVariant;
use lora_phy::sx126x::{Sx126x, Sx1262, TcxoCtrlVoltage};
use lora_phy::{mod_params::*, sx126x};
use mcrs_protocol::{GroupEncryptedPayload, Packet, Path};
use {defmt_rtt as _, panic_probe as _};

#[global_allocator]
static HEAP: Heap = Heap::empty();

// UK Narrow
const LORA_FREQUENCY_IN_HZ: u32 = 869_618_000;
const BANDWIDTH: Bandwidth = Bandwidth::_62KHz;
const RSSI_THRESHOLD: i16 = -100;

bind_interrupts!(struct Irqs {
    TWISPI1 => spim::InterruptHandler<peripherals::TWISPI1>;
});

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    unsafe {
        embedded_alloc::init!(HEAP, 1024);
    }

    let p = embassy_nrf::init(Default::default());

    let nss = Output::new(p.P0_04, Level::High, OutputDrive::Standard);
    let reset = Output::new(p.P0_28, Level::High, OutputDrive::Standard);
    let dio1 = Input::new(p.P0_03, Pull::Down);
    let busy = Input::new(p.P0_29, Pull::None);

    let mut spi_config = spim::Config::default();
    spi_config.frequency = spim::Frequency::M16;
    let spim = spim::Spim::new(p.TWISPI1, Irqs, p.P1_13, p.P1_14, p.P1_15, spi_config);
    let spi = ExclusiveDevice::new(spim, nss, Delay).unwrap();

    let config = sx126x::Config {
        chip: Sx1262,
        tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V7),
        use_dcdc: true,
        rx_boost: false,
    };
    let iv = GenericSx126xInterfaceVariant::new(reset, dio1, busy, None, None).unwrap();

    let mut lora = LoRa::with_syncword(Sx126x::new(spi, iv, config), 0x12, Delay)
        .await
        .unwrap();

    let mut packet_params = PacketParams {
        preamble_length: 8,
        implicit_header: false,
        payload_length: 255,
        crc_on: true,
        iq_inverted: false,
    };

    let modulation_params = ModulationParams {
        spreading_factor: SpreadingFactor::_8,
        bandwidth: BANDWIDTH,
        coding_rate: CodingRate::_4_8,
        low_data_rate_optimize: 0,
        frequency_in_hz: LORA_FREQUENCY_IN_HZ,
    };

    let hello_packet = Packet {
        route_type: mcrs_protocol::RouteType::Flood,
        transport_codes: None,
        path: mcrs_protocol::RoutePath::Normal(Path::empty()),
        payload: mcrs_protocol::Payload::GroupText(GroupEncryptedPayload {
            channel_hash: 17,
            mac: [67, 171],
            ciphertext: alloc::vec![
                39, 18, 148, 197, 19, 96, 163, 176, 29, 131, 128, 167, 232, 81, 91, 212, 21, 252,
                36, 200, 187, 147, 125, 56, 1, 39, 176, 31, 184, 50, 75, 149,
            ],
        }),
    };

    let hello_packet_data = hello_packet.encode().unwrap();

    lora.prepare_for_tx(
        &modulation_params,
        &mut packet_params,
        99,
        &hello_packet_data,
    )
    .await
    .unwrap();

    lora.tx().await.unwrap();

    lora.prepare_for_rx(RxMode::Continuous, &modulation_params, &packet_params)
        .await
        .unwrap();

    let mut buf = [0u8; 255];

    loop {
        match lora.rx(&packet_params, &mut buf).await {
            Ok((len, status)) => {
                info!("recv!: {}", &status);
                let packet = Packet::decode(&buf[..usize::from(len)]);
                warn!("{:?}", packet);
            }
            Err(err) => {
                info!("Radio error = {}", err);
            }
        };
    }
}
