//! Starts the repeater on a board's devices and runs its tasks.
use crate::{
    app::{self, AppContext, battery, cli, config::AppConfig, periodic, ui},
    memory::MemoryProfile,
    platform::{Battery, Platform, Radio, StatusDisplay, Storage, led::Led, sensors::Sensors},
};
use alloc::{boxed::Box, string::String};
use core::future::Future;
use defmt::{error, info};
use embassy_futures::{
    join::{join, join5},
    select::{Either3, select3},
};
use embedded_hal_async::digital::Wait;
use embedded_io_async::{Read, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SleepRequest {
    LowVoltage,
    Hibernate(core::time::Duration),
}

/// The repeater for one board, with the devices it drives.
pub struct Repeater<'a, PLATFORM, STORAGE, RADIO, CONSOLE, DISPLAY, BUTTON, BATTERY, SENSORS, LED>
where
    PLATFORM: 'static,
    STORAGE: 'static,
{
    context: &'static AppContext<PLATFORM, STORAGE>,
    radio: &'a mut RADIO,
    console: &'a mut CONSOLE,
    display: &'a mut DISPLAY,
    button: &'a mut BUTTON,
    battery: &'a mut BATTERY,
    sensors: &'a mut SENSORS,
    led: &'a mut LED,
}

impl<'a, PLATFORM, STORAGE, RADIO, CONSOLE, DISPLAY, BUTTON, BATTERY, SENSORS, LED>
    Repeater<'a, PLATFORM, STORAGE, RADIO, CONSOLE, DISPLAY, BUTTON, BATTERY, SENSORS, LED>
where
    PLATFORM: Platform + 'static,
    STORAGE: Storage + 'static,
    RADIO: Radio,
    CONSOLE: Read + Write,
    DISPLAY: StatusDisplay,
    BUTTON: Wait,
    BATTERY: Battery,
    SENSORS: Sensors,
    LED: Led,
{
    /// Loads the configuration from `storage`, or creates it on first boot.
    /// `console` serves the local CLI, and a press of `button` pulls its input
    /// low. For a device the board doesn't have, pass the matching `No*` type
    /// from [`crate::platform`].
    pub fn new(
        platform: PLATFORM,
        mut storage: STORAGE,
        radio: &'a mut RADIO,
        console: &'a mut CONSOLE,
        display: &'a mut DISPLAY,
        button: &'a mut BUTTON,
        battery: &'a mut BATTERY,
        sensors: &'a mut SENSORS,
        led: &'a mut LED,
    ) -> Self {
        log_memory_profile(platform.memory_profile());

        let defaults = AppConfig::generated_defaults(platform.identity_seed());
        let config = AppConfig::load_or_create(&mut storage, defaults);
        log_effective_config(&config);

        // The repeater runs until the board reboots, so the context is never
        // freed. Leaking it lets the board's own tasks share it.
        let context = Box::leak(Box::new(AppContext::new(platform, config, storage)));

        Self {
            context,
            radio,
            console,
            display,
            button,
            battery,
            sensors,
            led,
        }
    }

    /// The repeater's context, for the board's own tasks, such as Wi-Fi, that
    /// run alongside [`Self::run`].
    pub fn context(&self) -> &'static AppContext<PLATFORM, STORAGE> {
        self.context
    }

    /// Runs alongside the board task and returns the reason for sleeping.
    /// `lowvoltage_supported` indicates whether the board supports recovery.
    /// Returning drops all internal task futures and releases device borrows.
    pub async fn run(self, lowvoltage_supported: bool, board_task: impl Future) -> SleepRequest {
        let Self {
            context,
            radio,
            console,
            display,
            button,
            battery,
            sensors,
            led,
        } = self;

        let repeater_tasks = join5(
            ui::run(display, button, led, context),
            cli::serve_console(console, context),
            app::handler_loop(context),
            periodic::run(context, sensors),
            radio_task(radio, context),
        );
        let all_tasks = join(repeater_tasks, board_task);

        match select3(
            context.wait_for_sleep(),
            battery::run(battery, context, lowvoltage_supported),
            all_tasks,
        )
        .await
        {
            Either3::First(request) => request,
            _ => SleepRequest::LowVoltage,
        }
    }
}

async fn radio_task<P, S, R>(radio: &mut R, context: &AppContext<P, S>)
where
    P: Platform,
    S: Storage,
    R: Radio,
{
    let config = context.with_config(|config| config.radio()).await;
    if radio.start(config).await.is_err() {
        error!("Radio: start failed");
        // Keep the CLI running so the settings can be fixed before rebooting.
        loop {
            core::future::pending::<()>().await;
        }
    }
    let mut delay = context.platform.delay();
    join(
        app::radio_loop(radio, context, config, &mut delay),
        periodic::startup_advert(context),
    )
    .await;
}

fn log_memory_profile(memory: MemoryProfile) {
    info!(
        "Memory profile: heap={} inbound_queue={} outbound_queue={} neighbours={} seen_packets={}",
        memory.heap_size,
        memory.inbound_queue_len,
        memory.outbound_queue_len,
        memory.max_neighbours,
        memory.seen_packet_cache_len
    );
}

fn log_effective_config(config: &AppConfig) {
    let mut output = String::new();
    config.write_effective_config(&mut output);
    info!("Effective config:");
    for line in output.lines() {
        info!("{}", line);
    }
}
