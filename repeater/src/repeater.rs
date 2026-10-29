//! Starts the repeater on a [`Platform`] and runs its tasks.
use alloc::string::String;
use core::{
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};

use defmt::{error, info};

use crate::{
    app::{self, AppContext, battery, cli, config::AppConfig, periodic, ui},
    memory::MemoryProfile,
    platform::Platform,
};

/// The repeater for one board.
pub struct Repeater<P> {
    platform: P,
}

impl<P: Platform> Repeater<P> {
    pub fn new(platform: P) -> Self {
        Self { platform }
    }

    /// Loads the configuration, starts the board's devices and runs the
    /// repeater. It only stops when the board reboots.
    pub async fn run(self) -> ! {
        let mut platform = self.platform;
        log_memory_profile(platform.memory_profile());

        let mut storage = platform.take_storage();
        let defaults = AppConfig::generated_defaults(platform.identity_seed());
        let config = AppConfig::load_or_create(&mut storage, defaults);
        log_effective_config(&config);

        // Without a radio the CLI keeps running, so the settings can be fixed.
        let radio = match platform.start_radio(config.radio()).await {
            Ok(radio) => Some(radio),
            Err(()) => {
                error!("Radio: start failed");
                None
            }
        };
        let console = platform.take_console();
        let display = platform.take_display();
        let button = platform.take_button();
        let battery = platform.take_battery();

        let context = AppContext::new(platform, config, storage);
        let mut ui_future = pin!(ui::run(display, button, &context));
        let mut console_future = pin!(cli::serve_console(console, &context));
        let mut handler_future = pin!(app::handler_loop(&context));
        let mut battery_future = pin!(battery::run(battery, &context));
        let mut periodic_future = pin!(periodic::run(&context));
        let mut radio_future = pin!(radio_task(radio, &context));

        poll_fn(|cx| {
            match ui_future.as_mut().poll(cx) {
                Poll::Ready(never) => match never {},
                Poll::Pending => {}
            }

            match console_future.as_mut().poll(cx) {
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
}

async fn radio_task<P: Platform>(radio: Option<P::Radio>, context: &AppContext<P>) -> ! {
    let Some(mut radio) = radio else {
        loop {
            core::future::pending::<()>().await;
        }
    };
    let mut delay = context.platform.delay();
    app::radio_loop(&mut radio, context, &mut delay).await
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
