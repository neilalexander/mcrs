//! The status screen and the user button. A press wakes the screen, and
//! holding the button sends a zero-hop advert.
use alloc::string::String;
use core::{
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};

use defmt::{error, info};
use embedded_hal_async::{delay::DelayNs, digital::Wait};

use super::AppContext;
use crate::platform::{DisplayStatus, Platform, StatusDisplay};

const DISPLAY_AWAKE_MS: u32 = 5_000;
const ADVERT_HOLD_MS: u32 = 2_000;

pub(crate) async fn run<P: Platform>(
    display: Option<P::Display>,
    button: Option<P::Button>,
    context: &AppContext<P>,
) -> ! {
    let mut button = Button {
        input: button,
        delay: context.platform.delay(),
    };
    let mut delay = context.platform.delay();

    let Some(mut display) = display else {
        handle_button(&mut button, context).await
    };
    if display.init(&mut delay).await.is_err() {
        error!("display init failed");
        handle_button(&mut button, context).await
    }
    if show_status(&mut display, context, None).await.is_err() {
        error!("display write failed");
        handle_button(&mut button, context).await
    }

    let mut awake = true;
    loop {
        if awake {
            match wait_for_timeout_or_press(&mut delay, &mut button).await {
                DisplayEvent::Timeout => {
                    if display.sleep().is_ok() {
                        awake = false;
                    } else {
                        error!("display timeout");
                    }
                }
                DisplayEvent::ButtonPressed => {
                    if show_status(&mut display, context, None).await.is_err() {
                        error!("display status error after button press");
                    }
                    info!("Display wake extended");
                    let action = button.release_or_hold().await;
                    show_advert_message_if_sent(&mut display, context, action).await;
                }
            }
        } else {
            button.press().await;
            // Start tracking the release before powering the display back up.
            // A short press can otherwise finish during the display's power-on
            // and reset sequence and be misclassified as a long hold.
            let (wake_result, action) = {
                let mut action = pin!(button.release_or_hold());
                let mut wake_display = pin!(display.wake(&mut delay));
                let mut completed_action = None;
                let wake_result = poll_fn(|cx| {
                    if completed_action.is_none()
                        && let Poll::Ready(action) = action.as_mut().poll(cx)
                    {
                        completed_action = Some(action);
                    }
                    wake_display.as_mut().poll(cx)
                })
                .await;
                let action = match completed_action {
                    Some(action) => action,
                    None => action.as_mut().await,
                };
                (wake_result, action)
            };

            if wake_result.is_ok() && show_status(&mut display, context, None).await.is_ok() {
                info!("Display woken by button");
                awake = true;
                show_advert_message_if_sent(&mut display, context, action).await;
            } else {
                error!("display status or wake result error");
            }
        }
    }
}

/// Without a working display, the button only sends adverts.
async fn handle_button<P: Platform>(
    button: &mut Button<P::Button, P::Delay>,
    context: &AppContext<P>,
) -> ! {
    loop {
        if button.action().await == ButtonAction::LongHold {
            queue_advert(context).await;
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

/// The user button, if there is one, with a delay for timing holds.
struct Button<B, D> {
    input: Option<B>,
    delay: D,
}

impl<B: Wait, D: DelayNs> Button<B, D> {
    /// Waits for a press, which never comes without a button.
    async fn press(&mut self) {
        match &mut self.input {
            Some(input) => {
                let _ = input.wait_for_falling_edge().await;
            }
            None => core::future::pending().await,
        }
    }

    async fn release_or_hold(&mut self) -> ButtonAction {
        let Some(input) = &mut self.input else {
            return core::future::pending().await;
        };
        let mut hold = pin!(self.delay.delay_ms(ADVERT_HOLD_MS));
        let mut release = pin!(input.wait_for_rising_edge());

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

    async fn action(&mut self) -> ButtonAction {
        self.press().await;
        self.release_or_hold().await
    }
}

async fn wait_for_timeout_or_press<B: Wait, D: DelayNs>(
    delay: &mut D,
    button: &mut Button<B, D>,
) -> DisplayEvent {
    let mut timeout = pin!(delay.delay_ms(DISPLAY_AWAKE_MS));
    let mut press = pin!(button.press());

    poll_fn(|cx| {
        if timeout.as_mut().poll(cx).is_ready() {
            return Poll::Ready(DisplayEvent::Timeout);
        }

        if press.as_mut().poll(cx).is_ready() {
            return Poll::Ready(DisplayEvent::ButtonPressed);
        }

        Poll::Pending
    })
    .await
}

async fn queue_advert<P: Platform>(context: &AppContext<P>) -> bool {
    let now = context.platform.now_seconds();
    let packet = context
        .with_config(|config| super::discovery::zero_hop_advert(config, now))
        .await;

    let Some(packet) = packet else {
        error!("Button: advert encode failed");
        return false;
    };

    let len = packet.len();
    let region = context.outbound_region_label(&packet).await;
    match context.enqueue_outbound(packet) {
        Ok(()) => {
            match region {
                Some(region) => info!(
                    "Button: queued zero-hop advert {} bytes region={}",
                    len, region
                ),
                None => info!("Button: queued zero-hop advert {} bytes", len),
            }
            true
        }
        Err(_) => {
            error!("Button: advert queue full");
            false
        }
    }
}

async fn show_advert_message_if_sent<P: Platform>(
    display: &mut P::Display,
    context: &AppContext<P>,
    action: ButtonAction,
) {
    if action != ButtonAction::LongHold {
        return;
    }

    if !queue_advert(context).await {
        return;
    }

    if show_status(display, context, Some("Zero-hop advert sent"))
        .await
        .is_err()
    {
        error!("display write failed");
    }
}

async fn show_status<P: Platform>(
    display: &mut P::Display,
    context: &AppContext<P>,
    message: Option<&str>,
) -> Result<(), ()> {
    let status = context.status();
    let node_name = context
        .with_config(|config| String::from(config.node_name()))
        .await;
    let public_key_prefix = context.public_key_prefix::<3>().await;
    display.show(&DisplayStatus {
        node_name: &node_name,
        version: crate::VERSION,
        public_key_prefix,
        packets_sent: status.packets_sent,
        packets_received: status.packets_received,
        packet_errors: status.packet_errors,
        battery_millivolts: status.battery_millivolts,
        battery_level_percent: status.battery_level_percent,
        message,
    })
}
