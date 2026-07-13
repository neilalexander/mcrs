use embedded_io_async::Write as _;

use super::{AppContext, cli::PostReplyAction};

const TELNET_PORT: u16 = 23;
const LINE_BUFFER_LEN: usize = 160;
const IAC: u8 = 255;

pub async fn serve<S>(stack: embassy_net::Stack<'_>, context: &AppContext<S>) -> !
where
    S: crate::platform::storage::Storage,
{
    if !context.with_config(|config| config.wifi().telnet()).await {
        core::future::pending::<()>().await;
    }

    crate::platform::log_fmt(format_args!(
        "Wi-Fi: privileged CLI listening on telnet port {}",
        TELNET_PORT
    ));
    loop {
        let mut rx_buffer = [0u8; 512];
        let mut tx_buffer = [0u8; 1024];
        let mut socket = embassy_net::tcp::TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        if socket.accept(TELNET_PORT).await.is_ok() {
            crate::platform::log_fmt(format_args!("Wi-Fi: telnet CLI client connected"));
            let _ = handle_connection(&mut socket, context).await;
        }
        socket.abort();
    }
}

async fn handle_connection<S>(
    socket: &mut embassy_net::tcp::TcpSocket<'_>,
    context: &AppContext<S>,
) -> Result<(), ()>
where
    S: crate::platform::storage::Storage,
{
    // WILL ECHO, WILL SUPPRESS-GO-AHEAD, DO SUPPRESS-GO-AHEAD.
    socket
        .write_all(&[IAC, 251, 1, IAC, 251, 3, IAC, 253, 3])
        .await
        .map_err(|_| ())?;
    socket
        .write_all(b"MCRS privileged CLI\r\n> ")
        .await
        .map_err(|_| ())?;

    let mut line = [0u8; LINE_BUFFER_LEN];
    let mut line_len = 0;
    let mut input = [0u8; 64];
    let mut telnet_bytes_left = 0u8;
    let mut last_was_cr = false;
    loop {
        let count = socket.read(&mut input).await.map_err(|_| ())?;
        if count == 0 {
            return Ok(());
        }
        for &byte in &input[..count] {
            if telnet_bytes_left > 0 {
                telnet_bytes_left -= 1;
                continue;
            }
            if byte == IAC {
                // Telnet WILL/WONT/DO/DONT commands are three bytes.
                telnet_bytes_left = 2;
                continue;
            }
            match byte {
                b'\r' | b'\n' if !(byte == b'\n' && last_was_cr) => {
                    last_was_cr = byte == b'\r';
                    socket.write_all(b"\r\n").await.map_err(|_| ())?;
                    let command = core::str::from_utf8(&line[..line_len]).unwrap_or("").trim();
                    line_len = 0;
                    if let Some(response) =
                        super::cli::handle_privileged_tcp_command(command, context).await
                    {
                        for output_line in response.text.lines() {
                            socket
                                .write_all(output_line.as_bytes())
                                .await
                                .map_err(|_| ())?;
                            socket.write_all(b"\r\n").await.map_err(|_| ())?;
                        }
                        socket.flush().await.map_err(|_| ())?;
                        if response.post_reply == Some(PostReplyAction::Reboot) {
                            crate::platform::reboot();
                        }
                    }
                    socket.write_all(b"> ").await.map_err(|_| ())?;
                }
                b'\n' => last_was_cr = false,
                0x08 | 0x7f if line_len > 0 => {
                    last_was_cr = false;
                    line_len -= 1;
                    socket.write_all(b"\x08 \x08").await.map_err(|_| ())?;
                }
                byte if !byte.is_ascii_control() && line_len < line.len() => {
                    last_was_cr = false;
                    line[line_len] = byte;
                    line_len += 1;
                    socket.write_all(&[byte]).await.map_err(|_| ())?;
                }
                _ => last_was_cr = false,
            }
        }
    }
}
