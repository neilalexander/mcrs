use embedded_hal_async::delay::DelayNs;

use super::AppContext;

mod schedule;

use schedule::Schedule;

const ZERO_HOP_ADVERT_INTERVAL_HOURS: u32 = 4;

pub async fn run<S, D>(context: &AppContext<S>, delay: &mut D) -> !
where
    S: crate::platform::storage::Storage,
    D: DelayNs,
{
    let interval = context
        .with_config(|config| config.flood_advert_interval_hours())
        .await;
    let now_ms = crate::platform::now_millis();
    let mut zero_hop = Schedule::new(ZERO_HOP_ADVERT_INTERVAL_HOURS, now_ms);
    let mut flood = Schedule::new(interval, now_ms);
    loop {
        delay.delay_ms(1_000).await;
        let packet = context
            .with_config(|config| {
                if flood.due(
                    config.flood_advert_interval_hours(),
                    crate::platform::now_millis(),
                ) {
                    Some(super::discovery::flood_advert(config))
                } else {
                    None
                }
            })
            .await;
        if let Some(packet) = packet {
            send_advert(context, packet, "flood").await;
        }
        if zero_hop.due(
            ZERO_HOP_ADVERT_INTERVAL_HOURS,
            crate::platform::now_millis(),
        ) {
            let packet = context.with_config(super::discovery::zero_hop_advert).await;
            send_advert(context, packet, "zero-hop").await;
        }
    }
}

async fn send_advert<S>(context: &AppContext<S>, packet: Option<alloc::vec::Vec<u8>>, kind: &str)
where
    S: crate::platform::storage::Storage,
{
    let Some(packet) = packet else {
        crate::platform::log_fmt(format_args!("Periodic: {} advert encode failed", kind));
        return;
    };

    let len = packet.len();
    let region = context.outbound_region_label(&packet).await;
    match context.enqueue_outbound(packet) {
        Ok(()) => match region {
            Some(region) => crate::platform::log_fmt(format_args!(
                "Periodic: queued {} advert {} bytes region={}",
                kind, len, region
            )),
            None => crate::platform::log_fmt(format_args!(
                "Periodic: queued {} advert {} bytes",
                kind, len
            )),
        },
        Err(_) => crate::platform::log_fmt(format_args!("Periodic: {} advert queue full", kind)),
    }
}
