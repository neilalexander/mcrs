use esp_hal::{
    Async, Blocking,
    analog::adc::{Adc, AdcCalCurve, AdcPin},
    gpio::{Flex, Input, Level, Output},
    peripherals::{ADC1, GPIO1, WIFI},
    rng::Rng,
    spi::master::Spi,
    timer::timg::Timer as TimgTimer,
    uart::Uart,
};

use alloc::{format, string::String};
use core::{
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};
use embedded_hal_async::delay::DelayNs as _;
use esp_wifi::wifi::{
    AccessPointConfiguration, AuthMethod, ClientConfiguration, Configuration, WifiEvent,
};
use static_cell::StaticCell;

pub type RadioSpi = Spi<'static, Blocking>;
pub type RadioOutput = Output<'static>;
pub type RadioInput = Input<'static>;
pub type CliUart = Uart<'static, Async>;
pub type ButtonInput = Input<'static>;
pub type WifiTimer = TimgTimer<'static>;
pub type BatteryAdc = Adc<'static, ADC1<'static>, Blocking>;
pub type BatterySensePin = AdcPin<GPIO1<'static>, ADC1<'static>, AdcCalCurve<ADC1<'static>>>;

const FIRMWARE_VERSION: &str = env!("MESHCORE_FIRMWARE_VERSION");
const OTA_AP_IP: embassy_net::Ipv4Address = embassy_net::Ipv4Address::new(192, 168, 4, 1);
const OTA_AP_PREFIX: u8 = 24;
const OTA_HTTP_PORT: u16 = 80;
const NTP_SERVER: &str = "pool.ntp.org";
const NTP_PORT: u16 = 123;
const NTP_UNIX_EPOCH_OFFSET: u32 = 2_208_988_800;
const NTP_RETRY_SECONDS: u64 = 60;
const NTP_REFRESH_SECONDS: u64 = 6 * 60 * 60;
const OTA_DHCP_SERVER_PORT: u16 = 67;
const OTA_DHCP_CLIENT_PORT: u16 = 68;
const OTA_CLIENT_IP: embassy_net::Ipv4Address = embassy_net::Ipv4Address::new(192, 168, 4, 2);
const DHCP_PACKET_LEN: usize = 300;
const DHCP_COOKIE: [u8; 4] = [99, 130, 83, 99];
const DISPLAY_AWAKE_MS: u32 = 5_000;
const PRG_ADVERT_HOLD_MS: u32 = 2_000;
const BATTERY_SAMPLE_INTERVAL_MS: u32 = 60_000;
const BATTERY_STABILIZE_MS: u32 = 10;
const BATTERY_ADC_DISCARD_SAMPLES: u32 = 8;
const BATTERY_ADC_SAMPLES: u32 = 8;
// Heltec v3/v4 upstream firmware uses ADC_MULTIPLIER 5.42. The esp-hal
// calibrated result here matches the raw 10-bit ADC voltage term used by
// upstream, so the same board multiplier is still required.
const BATTERY_DIVIDER_SCALE_MILLI: u32 = 5420;
const BATTERY_PERCENT_MIN_MV: u16 = 3300;
const BATTERY_PERCENT_MAX_MV: u16 = 4100;
pub(crate) const MEMORY_PROFILE: crate::memory::MemoryProfile =
    crate::memory::MemoryProfile::new(160 * 1024, 8, 16, 64, 256);
pub(crate) const STORAGE_LAYOUT: crate::platform::storage::Layout =
    crate::platform::storage::Layout {
        partition_label: "meshcore",
        partition_size: 256 * 1024,
        max_file_size: 4096,
    };

pub struct RadioResources<F> {
    pub spi: RadioSpi,
    pub cs: RadioOutput,
    pub reset: RadioOutput,
    pub dio1: RadioInput,
    pub busy: RadioInput,
    pub board_config: crate::modules::sx1262::BoardConfig,
    pub frontend: F,
}

pub struct WifiResources {
    pub timer: WifiTimer,
    pub rng: Rng,
    pub wifi: WIFI<'static>,
}

pub struct BatteryMonitor {
    pub adc: BatteryAdc,
    pub sense: BatterySensePin,
    pub ctrl: Flex<'static>,
    pub ctrl_active_level: Level,
    pub ctrl_inactive_level: Level,
}

pub trait RadioFrontend {
    fn set_rx_mode(&mut self);
    fn set_tx_mode(&mut self);
}

#[allow(dead_code)]
pub struct NoRadioFrontend;

impl RadioFrontend for NoRadioFrontend {
    fn set_rx_mode(&mut self) {}
    fn set_tx_mode(&mut self) {}
}

pub fn generate_identity_seed(rng: &mut Rng) -> [u8; 32] {
    let mut seed = [0u8; 32];
    rng.read(&mut seed);

    while seed == [0u8; 32] {
        rng.read(&mut seed);
    }

    seed
}

async fn display_task<I2C, RESET, POWER>(
    mut display: Option<crate::modules::ssd1306::Display<I2C, RESET, POWER>>,
    mut button: ButtonInput,
    context: &crate::app::AppContext<crate::platform::EspStorage>,
) -> !
where
    I2C: embedded_hal::i2c::I2c + embedded_hal::i2c::ErrorType,
    RESET: embedded_hal::digital::OutputPin,
    POWER: embedded_hal::digital::OutputPin,
{
    let mut display_delay = crate::platform::radio_delay();
    let mut button_delay = crate::platform::radio_delay();

    let Some(display) = &mut display else {
        loop {
            if wait_for_prg_button_action(&mut button_delay, &mut button).await
                == ButtonAction::LongHold
            {
                queue_prg_button_advert(context).await;
            }
        }
    };

    if display.init(&mut display_delay).await.is_err() {
        crate::platform::log_display_init_failed();
        loop {
            if wait_for_prg_button_action(&mut button_delay, &mut button).await
                == ButtonAction::LongHold
            {
                queue_prg_button_advert(context).await;
            }
        }
    }

    if write_display_status(display, context, None).await.is_err() {
        crate::platform::log_display_write_failed();
        loop {
            if wait_for_prg_button_action(&mut button_delay, &mut button).await
                == ButtonAction::LongHold
            {
                queue_prg_button_advert(context).await;
            }
        }
    }

    let mut awake = true;
    loop {
        if awake {
            match wait_for_display_timeout_or_button(&mut display_delay, &mut button).await {
                DisplayEvent::Timeout => {
                    if display.sleep().is_ok() {
                        awake = false;
                    } else {
                        crate::platform::log_display_write_failed();
                    }
                }
                DisplayEvent::ButtonPressed => {
                    if write_display_status(display, context, None).await.is_err() {
                        crate::platform::log_display_write_failed();
                    }
                    crate::platform::log_fmt(format_args!("OLED display wake extended"));
                    let action =
                        wait_for_prg_button_release_or_hold(&mut button_delay, &mut button).await;
                    show_prg_advert_message_if_sent(display, context, action).await;
                }
            }
        } else {
            wait_for_prg_button_press(&mut button).await;
            if display.wake().is_ok() && write_display_status(display, context, None).await.is_ok()
            {
                crate::platform::log_fmt(format_args!("OLED display woken by PRG button"));
                awake = true;
                let action =
                    wait_for_prg_button_release_or_hold(&mut button_delay, &mut button).await;
                show_prg_advert_message_if_sent(display, context, action).await;
            } else {
                crate::platform::log_display_write_failed();
                let _ = wait_for_prg_button_release_or_hold(&mut button_delay, &mut button).await;
            }
        }
    }
}

enum DisplayEvent {
    Timeout,
    ButtonPressed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonAction {
    ShortPress,
    LongHold,
}

async fn wait_for_display_timeout_or_button(
    display_delay: &mut crate::platform::RadioDelay,
    button: &mut ButtonInput,
) -> DisplayEvent {
    let mut timeout = pin!(display_delay.delay_ms(DISPLAY_AWAKE_MS));
    let mut button_press = pin!(wait_for_prg_button_press(button));

    poll_fn(|cx| {
        if timeout.as_mut().poll(cx).is_ready() {
            return Poll::Ready(DisplayEvent::Timeout);
        }

        if button_press.as_mut().poll(cx).is_ready() {
            return Poll::Ready(DisplayEvent::ButtonPressed);
        }

        Poll::Pending
    })
    .await
}

async fn wait_for_prg_button_press(button: &mut ButtonInput) {
    let _ = embedded_hal_async::digital::Wait::wait_for_falling_edge(button).await;
}

async fn wait_for_prg_button_action(
    delay: &mut crate::platform::RadioDelay,
    button: &mut ButtonInput,
) -> ButtonAction {
    wait_for_prg_button_press(button).await;
    wait_for_prg_button_release_or_hold(delay, button).await
}

async fn wait_for_prg_button_release_or_hold(
    delay: &mut crate::platform::RadioDelay,
    button: &mut ButtonInput,
) -> ButtonAction {
    let mut hold = pin!(delay.delay_ms(PRG_ADVERT_HOLD_MS));
    let mut release = pin!(embedded_hal_async::digital::Wait::wait_for_rising_edge(
        button
    ));

    poll_fn(|cx| {
        if release.as_mut().poll(cx).is_ready() {
            return Poll::Ready(ButtonAction::ShortPress);
        }

        if hold.as_mut().poll(cx).is_ready() {
            return Poll::Ready(ButtonAction::LongHold);
        }

        Poll::Pending
    })
    .await
}

async fn queue_prg_button_advert(
    context: &crate::app::AppContext<crate::platform::EspStorage>,
) -> bool {
    let packet = context
        .with_config(crate::app::discovery::zero_hop_advert)
        .await;

    let Some(packet) = packet else {
        crate::platform::log_fmt(format_args!("PRG button: advert encode failed"));
        return false;
    };

    let len = packet.len();
    let region = context.outbound_region_label(&packet).await;
    match context.enqueue_outbound(packet) {
        Ok(()) => {
            match region {
                Some(region) => crate::platform::log_fmt(format_args!(
                    "PRG button: queued zero-hop advert {} bytes region={}",
                    len, region
                )),
                None => crate::platform::log_fmt(format_args!(
                    "PRG button: queued zero-hop advert {} bytes",
                    len
                )),
            }
            true
        }
        Err(_) => {
            crate::platform::log_fmt(format_args!("PRG button: advert queue full"));
            false
        }
    }
}

async fn show_prg_advert_message_if_sent<I2C, RESET, POWER>(
    display: &mut crate::modules::ssd1306::Display<I2C, RESET, POWER>,
    context: &crate::app::AppContext<crate::platform::EspStorage>,
    action: ButtonAction,
) where
    I2C: embedded_hal::i2c::I2c + embedded_hal::i2c::ErrorType,
    RESET: embedded_hal::digital::OutputPin,
    POWER: embedded_hal::digital::OutputPin,
{
    if action != ButtonAction::LongHold {
        return;
    }

    if !queue_prg_button_advert(context).await {
        return;
    }

    if write_display_status(display, context, Some("Zero-hop advert sent"))
        .await
        .is_err()
    {
        crate::platform::log_display_write_failed();
    }
}

async fn write_display_status<I2C, RESET, POWER>(
    display: &mut crate::modules::ssd1306::Display<I2C, RESET, POWER>,
    context: &crate::app::AppContext<crate::platform::EspStorage>,
    message: Option<&str>,
) -> Result<(), ()>
where
    I2C: embedded_hal::i2c::I2c + embedded_hal::i2c::ErrorType,
    RESET: embedded_hal::digital::OutputPin,
    POWER: embedded_hal::digital::OutputPin,
{
    let status = context.status();
    let node_name = context
        .with_config(|config| alloc::string::String::from(config.node_name()))
        .await;
    display.write_status(
        &node_name,
        FIRMWARE_VERSION,
        context.public_key_prefix::<3>().await,
        status.packets_sent,
        status.packets_received,
        status.packet_errors,
        status.battery_millivolts,
        status.battery_level_percent,
        message,
    )
}

pub async fn run_board_tasks<I2C, RESET, POWER, FRONTEND>(
    display: Option<crate::modules::ssd1306::Display<I2C, RESET, POWER>>,
    prg_button: ButtonInput,
    battery: BatteryMonitor,
    radio: RadioResources<FRONTEND>,
    mut cli_serial: Option<CliUart>,
    wifi: WifiResources,
    context: crate::app::AppContext<crate::platform::EspStorage>,
) -> !
where
    I2C: embedded_hal::i2c::I2c + embedded_hal::i2c::ErrorType,
    RESET: embedded_hal::digital::OutputPin,
    POWER: embedded_hal::digital::OutputPin,
    FRONTEND: RadioFrontend,
{
    let mut cli = crate::app::cli::Cli::new();
    let mut periodic_delay = crate::platform::radio_delay();
    let mut display_future = pin!(display_task(display, prg_button, &context));
    let mut radio_future = pin!(radio_task(radio, &context));
    let mut handler_future = pin!(crate::app::handler_loop(&context));
    let mut cli_future = pin!(cli_task(&mut cli_serial, &mut cli, &context));
    let mut battery_future = pin!(battery_task(battery, &context));
    let mut ota_future = pin!(wifi_task(wifi, &context));
    let mut periodic_future = pin!(crate::app::periodic::run(&context, &mut periodic_delay));

    poll_fn(|cx| {
        match display_future.as_mut().poll(cx) {
            Poll::Ready(never) => match never {},
            Poll::Pending => {}
        }

        match cli_future.as_mut().poll(cx) {
            Poll::Ready(never) => match never {},
            Poll::Pending => {}
        }

        match handler_future.as_mut().poll(cx) {
            Poll::Ready(never) => match never {},
            Poll::Pending => {}
        }

        match battery_future.as_mut().poll(cx) {
            Poll::Ready(never) => match never {},
            Poll::Pending => {}
        }

        match ota_future.as_mut().poll(cx) {
            Poll::Ready(never) => match never {},
            Poll::Pending => {}
        }

        match periodic_future.as_mut().poll(cx) {
            Poll::Ready(never) => match never {},
            Poll::Pending => {}
        }

        match radio_future.as_mut().poll(cx) {
            Poll::Ready(never) => match never {},
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

async fn wifi_task(
    wifi: WifiResources,
    context: &crate::app::AppContext<crate::platform::EspStorage>,
) -> ! {
    let init = match esp_wifi::init(wifi.timer, wifi.rng) {
        Ok(init) => init,
        Err(error) => {
            crate::platform::log_fmt(format_args!("Wi-Fi: init failed: {:?}", error));
            core::future::pending::<()>().await;
            unreachable!()
        }
    };
    let (controller, interfaces) = match esp_wifi::wifi::new(&init, wifi.wifi) {
        Ok(wifi) => wifi,
        Err(error) => {
            crate::platform::log_fmt(format_args!("Wi-Fi: device init failed: {:?}", error));
            core::future::pending::<()>().await;
            unreachable!()
        }
    };
    if context
        .with_config(|config| config.wifi().ssid().is_empty())
        .await
    {
        run_ota_ap_mode(controller, interfaces.ap, context).await
    } else {
        run_ota_station_mode(controller, interfaces.sta, context).await
    }
}

async fn run_ota_ap_mode<'a>(
    mut controller: esp_wifi::wifi::WifiController<'a>,
    device: esp_wifi::wifi::WifiDevice<'a>,
    context: &crate::app::AppContext<crate::platform::EspStorage>,
) -> ! {
    static STACK_RESOURCES: StaticCell<embassy_net::StackResources<4>> = StaticCell::new();
    let stack_resources = STACK_RESOURCES.init(embassy_net::StackResources::new());
    let net_config = embassy_net::Config::ipv4_static(embassy_net::StaticConfigV4 {
        address: embassy_net::Ipv4Cidr::new(OTA_AP_IP, OTA_AP_PREFIX),
        gateway: None,
        dns_servers: Default::default(),
    });
    let (stack, mut runner) = embassy_net::new(
        device,
        net_config,
        stack_resources,
        crate::platform::now_millis(),
    );
    let mut runner = pin!(runner.run());
    let mut dhcp_rx_meta = [embassy_net::udp::PacketMetadata::EMPTY; 1];
    let mut dhcp_tx_meta = [embassy_net::udp::PacketMetadata::EMPTY; 1];
    let mut dhcp_rx_buffer = [0u8; 576];
    let mut dhcp_tx_buffer = [0u8; DHCP_PACKET_LEN];
    let mut dhcp_socket = embassy_net::udp::UdpSocket::new(
        stack,
        &mut dhcp_rx_meta,
        &mut dhcp_rx_buffer,
        &mut dhcp_tx_meta,
        &mut dhcp_tx_buffer,
    );
    if dhcp_socket.bind(OTA_DHCP_SERVER_PORT).is_err() {
        crate::platform::log_fmt(format_args!("OTA: DHCP bind failed"));
        core::future::pending::<()>().await;
    }
    let mut dhcp = pin!(dhcp_server(&dhcp_socket));
    let ssid = ota_ssid(context).await;
    let mut worker = pin!(async {
        loop {
            wait_for_ota_requested_idle(context, true).await;
            if start_ota_ap(&mut controller, &ssid).await.is_err() {
                context.request_ota_stop();
                continue;
            }
            crate::platform::log_fmt(format_args!(
                "OTA: AP started ssid={} url=http://{}/",
                ssid, OTA_AP_IP
            ));
            serve_ota_session(stack, context).await;
            let _ = stop_ota_ap(&mut controller).await;
        }
    });
    let never: core::convert::Infallible = poll_fn(|cx| {
        let _ = runner.as_mut().poll(cx);
        let _ = dhcp.as_mut().poll(cx);
        match worker.as_mut().poll(cx) {
            Poll::Ready(never) => match never {},
            Poll::Pending => Poll::Pending,
        }
    })
    .await;
    match never {}
}

async fn run_ota_station_mode<'a>(
    mut controller: esp_wifi::wifi::WifiController<'a>,
    device: esp_wifi::wifi::WifiDevice<'a>,
    context: &crate::app::AppContext<crate::platform::EspStorage>,
) -> ! {
    static STACK_RESOURCES: StaticCell<embassy_net::StackResources<4>> = StaticCell::new();
    let stack_resources = STACK_RESOURCES.init(embassy_net::StackResources::new());
    let (stack, mut runner) = embassy_net::new(
        device,
        embassy_net::Config::dhcpv4(Default::default()),
        stack_resources,
        crate::platform::now_millis(),
    );
    let mut runner = pin!(runner.run());
    let mut worker = pin!(async {
        loop {
            let wifi = context.with_config(|config| config.wifi().clone()).await;
            let ssid = String::from(wifi.ssid());
            let password = String::from(wifi.password());
            if ssid.is_empty() {
                crate::platform::log_fmt(format_args!("Wi-Fi: configure wifi.ssid"));
                embassy_time::Timer::after_secs(30).await;
                continue;
            }
            let config = ClientConfiguration {
                ssid,
                password: password.clone(),
                auth_method: if password.is_empty() {
                    AuthMethod::None
                } else {
                    AuthMethod::WPA2Personal
                },
                ..Default::default()
            };
            if matches!(controller.is_started(), Ok(true)) {
                let _ = controller.stop_async().await;
            }
            if controller
                .set_configuration(&Configuration::Client(config))
                .is_err()
                || controller.start_async().await.is_err()
                || controller.connect_async().await.is_err()
            {
                crate::platform::log_fmt(format_args!("Wi-Fi: station connection failed"));
                embassy_time::Timer::after_secs(10).await;
                continue;
            }
            let configured = {
                let mut wait_config = pin!(stack.wait_config_up());
                let mut disconnected = pin!(controller.wait_for_event(WifiEvent::StaDisconnected));
                poll_fn(|cx| {
                    if wait_config.as_mut().poll(cx).is_ready() {
                        Poll::Ready(true)
                    } else if disconnected.as_mut().poll(cx).is_ready() {
                        Poll::Ready(false)
                    } else {
                        Poll::Pending
                    }
                })
                .await
            };
            if !configured {
                crate::platform::log_fmt(format_args!(
                    "Wi-Fi: disconnected before DHCP completed; reconnecting"
                ));
                let _ = controller.stop_async().await;
                embassy_time::Timer::after_secs(5).await;
                continue;
            }
            if let Some(config) = stack.config_v4() {
                crate::platform::log_fmt(format_args!(
                    "Wi-Fi: station connected address={} ota=http://{}:{}/",
                    config.address,
                    config.address.address(),
                    OTA_HTTP_PORT
                ));
            } else {
                crate::platform::log_fmt(format_args!("Wi-Fi: station connected"));
            }
            {
                let mut server = pin!(serve_ota(stack, context));
                let mut ntp = pin!(ntp_loop(stack));
                let mut disconnected = pin!(controller.wait_for_event(WifiEvent::StaDisconnected));
                poll_fn(|cx| {
                    if let Poll::Ready(never) = server.as_mut().poll(cx) {
                        match never {}
                    }
                    if let Poll::Ready(never) = ntp.as_mut().poll(cx) {
                        match never {}
                    }
                    if disconnected.as_mut().poll(cx).is_ready() {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                })
                .await;
            }
            crate::platform::log_fmt(format_args!("Wi-Fi: station disconnected; reconnecting"));
            let _ = controller.stop_async().await;
            embassy_time::Timer::after_secs(5).await;
        }
    });
    poll_fn(|cx| {
        let _ = runner.as_mut().poll(cx);
        match worker.as_mut().poll(cx) {
            Poll::Ready(never) => match never {},
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

async fn ntp_loop(stack: embassy_net::Stack<'_>) -> ! {
    loop {
        let delay_seconds = match sync_ntp(stack).await {
            Some(unix_seconds) => {
                if crate::platform::set_wall_clock_if_forward(unix_seconds) {
                    crate::platform::log_fmt(format_args!(
                        "NTP: synchronized unix={}",
                        unix_seconds
                    ));
                }
                NTP_REFRESH_SECONDS
            }
            None => {
                crate::platform::log_fmt(format_args!("NTP: sync failed; retrying"));
                NTP_RETRY_SECONDS
            }
        };
        embassy_time::Timer::after_secs(delay_seconds).await;
    }
}

async fn sync_ntp(stack: embassy_net::Stack<'_>) -> Option<u32> {
    let addresses = stack
        .dns_query(NTP_SERVER, embassy_net::dns::DnsQueryType::A)
        .await
        .ok()?;
    let address = addresses.first().copied()?;
    let mut rx_meta = [embassy_net::udp::PacketMetadata::EMPTY; 1];
    let mut tx_meta = [embassy_net::udp::PacketMetadata::EMPTY; 1];
    let mut rx_buffer = [0u8; 48];
    let mut tx_buffer = [0u8; 48];
    let mut socket = embassy_net::udp::UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );
    socket.bind(0).ok()?;
    let mut request = [0u8; 48];
    request[0] = 0x23;
    request[40..48].copy_from_slice(&crate::platform::now_millis().to_be_bytes());
    socket.send_to(&request, (address, NTP_PORT)).await.ok()?;

    let mut response = [0u8; 48];
    let mut timeout = pin!(embassy_time::Timer::after_secs(10));
    let len = poll_fn(|cx| match socket.poll_recv_from(&mut response, cx) {
        Poll::Ready(Ok((len, _))) => Poll::Ready(Some(len)),
        Poll::Ready(Err(_)) => Poll::Ready(None),
        Poll::Pending if timeout.as_mut().poll(cx).is_ready() => Poll::Ready(None),
        Poll::Pending => Poll::Pending,
    })
    .await?;
    if len < response.len() {
        return None;
    }
    let leap = response[0] >> 6;
    let mode = response[0] & 0x07;
    let stratum = response[1];
    if leap == 3
        || !matches!(mode, 4 | 5)
        || !(1..=15).contains(&stratum)
        || response[24..32] != request[40..48]
    {
        return None;
    }
    let ntp_seconds = u32::from_be_bytes(response[40..44].try_into().ok()?);
    ntp_seconds.checked_sub(NTP_UNIX_EPOCH_OFFSET)
}

async fn serve_ota(
    stack: embassy_net::Stack<'_>,
    context: &crate::app::AppContext<crate::platform::EspStorage>,
) -> ! {
    loop {
        wait_for_ota_requested_idle(context, true).await;
        serve_ota_session(stack, context).await;
    }
}

async fn serve_ota_session(
    stack: embassy_net::Stack<'_>,
    context: &crate::app::AppContext<crate::platform::EspStorage>,
) {
    while context.ota_requested() {
        let mut rx_buffer = [0u8; 2048];
        let mut tx_buffer = [0u8; 2048];
        let mut socket = embassy_net::tcp::TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        if accept_ota_connection(context, &mut socket).await {
            crate::platform::log_fmt(format_args!("OTA: client connected"));
            match handle_ota_connection(context, &mut socket).await {
                OtaConnectionResult::Complete(Ok(())) => {}
                OtaConnectionResult::Complete(Err(error)) => {
                    crate::platform::log_fmt(format_args!("OTA: request failed: {:?}", error));
                }
                OtaConnectionResult::Stopped => {
                    crate::platform::log_fmt(format_args!("OTA: request aborted"));
                }
            }
        }
        socket.abort();
    }
}

async fn accept_ota_connection<'a>(
    context: &crate::app::AppContext<crate::platform::EspStorage>,
    socket: &mut embassy_net::tcp::TcpSocket<'a>,
) -> bool {
    let generation = context.ota_generation();
    let mut accept = pin!(socket.accept(OTA_HTTP_PORT));
    poll_fn(|cx| {
        if !context.ota_requested() || context.ota_generation() != generation {
            return Poll::Ready(false);
        }
        context.register_ota_waker(cx.waker());
        match accept.as_mut().poll(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(true),
            Poll::Ready(Err(_)) => Poll::Ready(false),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

enum OtaConnectionResult {
    Complete(Result<(), crate::app::ota::HttpOtaError>),
    Stopped,
}

async fn handle_ota_connection<'a>(
    context: &crate::app::AppContext<crate::platform::EspStorage>,
    socket: &mut embassy_net::tcp::TcpSocket<'a>,
) -> OtaConnectionResult {
    let generation = context.ota_generation();
    let mut connection = pin!(crate::app::ota::handle_connection(socket));
    poll_fn(|cx| {
        if !context.ota_requested() || context.ota_generation() != generation {
            return Poll::Ready(OtaConnectionResult::Stopped);
        }
        context.register_ota_waker(cx.waker());
        match connection.as_mut().poll(cx) {
            Poll::Ready(result) => Poll::Ready(OtaConnectionResult::Complete(result)),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

async fn start_ota_ap(
    controller: &mut esp_wifi::wifi::WifiController<'_>,
    ssid: &str,
) -> Result<(), esp_wifi::wifi::WifiError> {
    controller.set_configuration(&Configuration::AccessPoint(AccessPointConfiguration {
        ssid: String::from(ssid),
        ssid_hidden: false,
        channel: 1,
        auth_method: AuthMethod::None,
        max_connections: 1,
        ..Default::default()
    }))?;
    controller.start_async().await
}

async fn stop_ota_ap(
    controller: &mut esp_wifi::wifi::WifiController<'_>,
) -> Result<(), esp_wifi::wifi::WifiError> {
    let stop_result = controller.stop_async().await;
    let disable_result = controller.set_configuration(&Configuration::None);

    match (stop_result, disable_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), _) | (Ok(()), Err(error)) => Err(error),
    }
}

async fn ota_ssid<S>(context: &crate::app::AppContext<S>) -> String
where
    S: crate::platform::storage::Storage,
{
    let public_key = context.public_key().await;
    format!(
        "meshcore-ota-{:02x}{:02x}{:02x}",
        public_key[0], public_key[1], public_key[2]
    )
}

async fn dhcp_server(socket: &embassy_net::udp::UdpSocket<'_>) -> ! {
    let mut request = [0u8; 576];
    let mut response = [0u8; DHCP_PACKET_LEN];
    loop {
        let Ok((len, _)) = socket.recv_from(&mut request).await else {
            continue;
        };
        let Some(response_len) = build_dhcp_response(&request[..len], &mut response) else {
            continue;
        };
        let destination = embassy_net::IpEndpoint::new(
            embassy_net::IpAddress::Ipv4(embassy_net::Ipv4Address::BROADCAST),
            OTA_DHCP_CLIENT_PORT,
        );
        let _ = socket.send_to(&response[..response_len], destination).await;
    }
}

fn build_dhcp_response(request: &[u8], response: &mut [u8; DHCP_PACKET_LEN]) -> Option<usize> {
    if request.len() < 240
        || request[0] != 1
        || request[1] != 1
        || request[2] != 6
        || request[236..240] != DHCP_COOKIE
    {
        return None;
    }

    let message_type = dhcp_message_type(&request[240..])?;
    let reply_type = match message_type {
        1 => 2, // DISCOVER -> OFFER
        3 => 5, // REQUEST -> ACK
        _ => return None,
    };

    response.fill(0);
    response[0] = 2;
    response[1..4].copy_from_slice(&request[1..4]);
    response[4..8].copy_from_slice(&request[4..8]);
    response[10..12].copy_from_slice(&request[10..12]);
    response[16..20].copy_from_slice(&OTA_CLIENT_IP.octets());
    response[20..24].copy_from_slice(&OTA_AP_IP.octets());
    response[24..28].copy_from_slice(&request[24..28]);
    response[28..44].copy_from_slice(&request[28..44]);
    response[236..240].copy_from_slice(&DHCP_COOKIE);

    let mut offset = 240;
    offset = append_dhcp_option(response, offset, 53, &[reply_type])?;
    offset = append_dhcp_option(response, offset, 54, &OTA_AP_IP.octets())?;
    offset = append_dhcp_option(response, offset, 51, &3600u32.to_be_bytes())?;
    offset = append_dhcp_option(response, offset, 1, &[255, 255, 255, 0])?;
    offset = append_dhcp_option(response, offset, 3, &OTA_AP_IP.octets())?;
    response[offset] = 255;
    Some(DHCP_PACKET_LEN)
}

fn dhcp_message_type(mut options: &[u8]) -> Option<u8> {
    while let Some((&code, rest)) = options.split_first() {
        options = rest;
        match code {
            0 => continue,
            255 => return None,
            _ => {
                let (&len, rest) = options.split_first()?;
                let len = len as usize;
                if rest.len() < len {
                    return None;
                }
                if code == 53 && len == 1 {
                    return Some(rest[0]);
                }
                options = &rest[len..];
            }
        }
    }
    None
}

fn append_dhcp_option(response: &mut [u8], offset: usize, code: u8, value: &[u8]) -> Option<usize> {
    let end = offset.checked_add(2)?.checked_add(value.len())?;
    if end >= response.len() || value.len() > u8::MAX as usize {
        return None;
    }
    response[offset] = code;
    response[offset + 1] = value.len() as u8;
    response[offset + 2..end].copy_from_slice(value);
    Some(end)
}

async fn wait_for_ota_requested_idle(
    context: &crate::app::AppContext<crate::platform::EspStorage>,
    requested: bool,
) {
    loop {
        if context.ota_requested() == requested {
            return;
        }

        let generation = context.ota_generation();
        poll_fn(|cx| {
            if context.ota_requested() == requested || context.ota_generation() != generation {
                return Poll::Ready(());
            }

            context.register_ota_waker(cx.waker());
            if context.ota_requested() == requested || context.ota_generation() != generation {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }
}

async fn battery_task(
    mut battery: BatteryMonitor,
    context: &crate::app::AppContext<crate::platform::EspStorage>,
) -> ! {
    let mut delay = crate::platform::radio_delay();

    loop {
        publish_battery_sample_with_delay(&mut battery, context, &mut delay).await;

        delay.delay_ms(BATTERY_SAMPLE_INTERVAL_MS).await;
    }
}

pub async fn publish_battery_sample(
    battery: &mut BatteryMonitor,
    context: &crate::app::AppContext<crate::platform::EspStorage>,
) {
    let mut delay = crate::platform::radio_delay();
    publish_battery_sample_with_delay(battery, context, &mut delay).await;
}

async fn publish_battery_sample_with_delay(
    battery: &mut BatteryMonitor,
    context: &crate::app::AppContext<crate::platform::EspStorage>,
    delay: &mut crate::platform::RadioDelay,
) {
    let Some(sample) = battery.sample(delay).await else {
        context.set_battery_millivolts(None);
        context.set_battery_level_percent(None);
        return;
    };

    let percent = battery_level_percent(sample.millivolts);
    crate::platform::log_fmt(format_args!(
        "Battery: adc={}mV battery={}mV {}%",
        sample.adc_millivolts, sample.millivolts, percent
    ));
    context.set_battery_millivolts(Some(sample.millivolts));
    context.set_battery_level_percent(Some(percent));
}

struct BatterySample {
    adc_millivolts: u32,
    millivolts: u16,
}

enum BatterySampleError {
    AdcRead,
    RawZero,
    Arithmetic,
}

impl BatterySampleError {
    fn log(self) {
        match self {
            Self::AdcRead => crate::platform::log_fmt(format_args!("Battery: ADC read failed")),
            Self::RawZero => {
                crate::platform::log_fmt(format_args!("Battery: ADC raw sample is zero"))
            }
            Self::Arithmetic => {
                crate::platform::log_fmt(format_args!("Battery: sample conversion failed"))
            }
        }
    }
}

async fn radio_task<FRONTEND, S>(
    radio: RadioResources<FRONTEND>,
    context: &crate::app::AppContext<S>,
) -> !
where
    FRONTEND: RadioFrontend,
    S: crate::platform::storage::Storage,
{
    let RadioResources {
        spi,
        cs,
        reset,
        dio1,
        busy,
        board_config,
        frontend,
    } = radio;
    let spi = crate::modules::spi_device::BlockingExclusiveSpiDevice::new(
        spi,
        cs,
        crate::platform::spi_delay(),
    );
    let receive_config = context
        .with_config(|config| config.radio().receive_config())
        .await;

    let receiver = match crate::modules::sx1262::init_receiver(
        spi,
        reset,
        dio1,
        busy,
        crate::platform::radio_delay(),
        board_config,
        receive_config,
    )
    .await
    {
        Ok(receiver) => receiver,
        Err(_) => {
            crate::platform::log_radio_init_failed();
            crate::platform::idle_loop();
        }
    };

    crate::platform::log_radio_initialized();
    let mut app_delay = crate::platform::radio_delay();
    let mut receiver = BoardRadio { receiver, frontend };
    crate::app::radio_loop(&mut receiver, context, &mut app_delay).await;
}

struct BoardRadio<R, FRONTEND> {
    receiver: R,
    frontend: FRONTEND,
}

impl<R, FRONTEND> crate::modules::Receiver for BoardRadio<R, FRONTEND>
where
    R: crate::modules::Receiver,
    FRONTEND: RadioFrontend,
{
    async fn wait_for_read(
        &mut self,
        buffer: &mut [u8],
    ) -> Result<crate::modules::ReceivedPacket, ()> {
        self.frontend.set_rx_mode();
        self.receiver.wait_for_read(buffer).await
    }

    async fn channel_is_busy(&mut self) -> Result<bool, ()> {
        self.frontend.set_rx_mode();
        self.receiver.channel_is_busy().await
    }

    async fn transmit(&mut self, payload: &[u8]) -> Result<(), ()> {
        self.frontend.set_tx_mode();
        let result = self.receiver.transmit(payload).await;
        self.frontend.set_rx_mode();
        result
    }
}

async fn cli_task(
    serial: &mut Option<CliUart>,
    cli: &mut crate::app::cli::Cli,
    context: &crate::app::AppContext<crate::platform::EspStorage>,
) -> ! {
    loop {
        let Some(serial) = serial else {
            core::future::pending::<()>().await;
            continue;
        };

        let mut byte = [0];
        match serial.read_async(&mut byte).await {
            Ok(count) if count > 0 => {
                let echo = cli.echo_for_byte(byte[0]);
                match echo {
                    crate::app::cli::SerialEcho::None => {}
                    crate::app::cli::SerialEcho::Byte => {
                        let _ = serial.write_async(&byte).await;
                    }
                    crate::app::cli::SerialEcho::Bytes(bytes) => {
                        let _ = serial.write_async(bytes).await;
                    }
                }
                cli.accept_byte(byte[0], context).await;
            }
            Ok(_) | Err(_) => {}
        }
    }
}

impl BatteryMonitor {
    async fn sample(&mut self, delay: &mut crate::platform::RadioDelay) -> Option<BatterySample> {
        self.ctrl.set_level(self.ctrl_active_level);
        delay.delay_ms(BATTERY_STABILIZE_MS).await;

        let raw = self.read_adc_average();
        self.ctrl.set_level(self.ctrl_inactive_level);

        let adc_millivolts = match raw {
            Ok(adc_millivolts) => adc_millivolts,
            Err(error) => {
                error.log();
                return None;
            }
        };
        if adc_millivolts == 0 {
            BatterySampleError::RawZero.log();
            return None;
        }

        let adc_millivolts = u64::from(adc_millivolts);
        let Some(millivolts) = adc_millivolts
            .checked_mul(u64::from(BATTERY_DIVIDER_SCALE_MILLI))
            .and_then(|value| value.checked_div(1000))
        else {
            BatterySampleError::Arithmetic.log();
            return None;
        };

        Some(BatterySample {
            adc_millivolts: adc_millivolts.min(u64::from(u32::MAX)) as u32,
            millivolts: millivolts.min(u64::from(u16::MAX)) as u16,
        })
    }

    fn read_adc_average(&mut self) -> Result<u32, BatterySampleError> {
        for _ in 0..BATTERY_ADC_DISCARD_SAMPLES {
            let _ = self.read_adc_sample()?;
        }

        let mut total = 0u32;
        for _ in 0..BATTERY_ADC_SAMPLES {
            total = total
                .checked_add(u32::from(self.read_adc_sample()?))
                .ok_or(BatterySampleError::Arithmetic)?;
        }

        Ok(total / BATTERY_ADC_SAMPLES)
    }

    fn read_adc_sample(&mut self) -> Result<u16, BatterySampleError> {
        loop {
            match self.adc.read_oneshot(&mut self.sense) {
                Ok(value) => return Ok(value),
                Err(nb::Error::WouldBlock) => {}
                Err(nb::Error::Other(())) => return Err(BatterySampleError::AdcRead),
            }
        }
    }
}

fn battery_level_percent(millivolts: u16) -> u8 {
    if millivolts <= BATTERY_PERCENT_MIN_MV {
        return 0;
    }

    if millivolts >= BATTERY_PERCENT_MAX_MV {
        return 100;
    }

    let range = u32::from(BATTERY_PERCENT_MAX_MV - BATTERY_PERCENT_MIN_MV);
    let offset = u32::from(millivolts - BATTERY_PERCENT_MIN_MV);
    ((offset * 100) / range) as u8
}

pub fn opposite_level(level: Level) -> Level {
    match level {
        Level::High => Level::Low,
        Level::Low => Level::High,
    }
}

pub async fn log_effective_config<S>(context: &crate::app::AppContext<S>)
where
    S: crate::platform::storage::Storage,
{
    let mut output = alloc::string::String::new();
    context
        .with_config(|config| config.write_effective_config(&mut output))
        .await;
    crate::platform::log_fmt(format_args!("Effective config:"));
    for line in output.lines() {
        crate::platform::log_fmt(format_args!("{}", line));
    }
}

pub fn log_memory_profile(memory: crate::memory::MemoryProfile) {
    crate::platform::log_fmt(format_args!(
        "Memory profile: heap={} inbound_queue={} outbound_queue={} neighbours={} seen_packets={}",
        memory.heap_size,
        memory.inbound_queue_len,
        memory.outbound_queue_len,
        memory.max_neighbours,
        memory.seen_packet_cache_len
    ));
}
