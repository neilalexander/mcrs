use super::AppContext;
use crate::platform::Platform;
use defmt::{error, info};
use embedded_hal_async::delay::DelayNs;

mod schedule;

use schedule::Schedule;

pub(crate) async fn run<P: Platform>(context: &AppContext<P>) -> ! {
    let mut delay = context.platform.delay();
    let (advert_interval, flood_interval) = context
        .with_config(|config| {
            (
                u64::from(config.advert_interval_minutes()) * 60_000,
                u64::from(config.flood_advert_interval_hours()) * 3_600_000,
            )
        })
        .await;
    let now_ms = context.platform.now_millis();
    let mut zero_hop = Schedule::new(advert_interval, now_ms);
    let mut flood = Schedule::new(flood_interval, now_ms);
    loop {
        delay.delay_ms(1_000).await;
        let packet = context
            .with_config(|config| {
                if flood.due(
                    u64::from(config.flood_advert_interval_hours()) * 3_600_000,
                    context.platform.now_millis(),
                ) {
                    Some(super::discovery::flood_advert(
                        config,
                        context.platform.now_seconds(),
                    ))
                } else {
                    None
                }
            })
            .await;
        if let Some(packet) = packet {
            send_advert(context, packet, "flood").await;
        }
        let packet = context
            .with_config(|config| {
                if zero_hop.due(
                    u64::from(config.advert_interval_minutes()) * 60_000,
                    context.platform.now_millis(),
                ) {
                    Some(super::discovery::zero_hop_advert(
                        config,
                        context.platform.now_seconds(),
                    ))
                } else {
                    None
                }
            })
            .await;
        if let Some(packet) = packet {
            send_advert(context, packet, "zero-hop").await;
        }
    }
}

async fn send_advert<P: Platform>(
    context: &AppContext<P>,
    packet: Option<alloc::vec::Vec<u8>>,
    kind: &str,
) {
    let Some(packet) = packet else {
        error!("Periodic: {} advert encode failed", kind);
        return;
    };

    let len = packet.len();
    let region = context.outbound_region_label(&packet).await;
    match context.enqueue_outbound(packet) {
        Ok(()) => match region {
            Some(region) => info!(
                "Periodic: queued {} advert {} bytes region={}",
                kind, len, region
            ),
            None => info!("Periodic: queued {} advert {} bytes", kind, len),
        },
        Err(_) => error!("Periodic: {} advert queue full", kind),
    }
}
