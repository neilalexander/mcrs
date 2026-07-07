use embedded_hal_async::delay::DelayNs;

use super::AppContext;

const ZERO_HOP_ADVERT_INTERVAL_MS: u32 = 4 * 60 * 60 * 1000;

pub async fn run<S, D>(context: &AppContext<S>, delay: &mut D) -> !
where
    S: crate::platform::storage::Storage,
    D: DelayNs,
{
    loop {
        delay.delay_ms(ZERO_HOP_ADVERT_INTERVAL_MS).await;
        send_zero_hop_advert(context).await;
    }
}

async fn send_zero_hop_advert<S>(context: &AppContext<S>)
where
    S: crate::platform::storage::Storage,
{
    let packet = context.with_config(super::discovery::zero_hop_advert).await;

    let Some(packet) = packet else {
        crate::platform::log_fmt(format_args!("Periodic: zero-hop advert encode failed"));
        return;
    };

    let len = packet.len();
    let region = context.outbound_region_label(&packet).await;
    match context.enqueue_outbound(packet) {
        Ok(()) => match region {
            Some(region) => crate::platform::log_fmt(format_args!(
                "Periodic: queued zero-hop advert {} bytes region={}",
                len, region
            )),
            None => crate::platform::log_fmt(format_args!(
                "Periodic: queued zero-hop advert {} bytes",
                len
            )),
        },
        Err(_) => crate::platform::log_fmt(format_args!("Periodic: zero-hop advert queue full")),
    }
}
