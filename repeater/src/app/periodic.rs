use embedded_hal_async::delay::DelayNs;

use super::AppContext;

mod schedule;

use schedule::Schedule;

pub async fn run<S, D>(context: &AppContext<S>, delay: &mut D) -> !
where
    S: crate::platform::storage::Storage,
    D: DelayNs,
{
    let (advert_interval, flood_interval) = context
        .with_config(|config| {
            (
                u64::from(config.advert_interval_minutes()) * 60_000,
                u64::from(config.flood_advert_interval_hours()) * 3_600_000,
            )
        })
        .await;
    let now_ms = crate::platform::now_millis();
    let mut zero_hop = Schedule::new(advert_interval, now_ms);
    let mut flood = Schedule::new(flood_interval, now_ms);
    loop {
        delay.delay_ms(1_000).await;
        let packet = context
            .with_config(|config| {
                if flood.due(
                    u64::from(config.flood_advert_interval_hours()) * 3_600_000,
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
        let packet = context
            .with_config(|config| {
                if zero_hop.due(
                    u64::from(config.advert_interval_minutes()) * 60_000,
                    crate::platform::now_millis(),
                ) {
                    Some(super::discovery::zero_hop_advert(config))
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
