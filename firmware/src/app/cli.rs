extern crate alloc;

use super::AppContext;
use alloc::{format, string::String, vec, vec::Vec};
use core::fmt::Write;
use mcrs_protocol::{
    PERM_ACL_ADMIN, PERM_ACL_GUEST, Packet, Payload, RepeaterResponsePlaintext, RequestPlaintext,
    TextMessagePlaintext, TextType,
};

const LINE_BUFFER_LEN: usize = 160;
const REMOTE_CLI_REPLY_MAX_LEN: usize = 160;
const REMOTE_CLI_PREFIX: &str = "cli ";
const REMOTE_LOGIN_PREFIX: &str = "login ";
const ANON_REQ_TYPE_REGIONS: u8 = 0x01;
const ANON_REQ_TYPE_OWNER: u8 = 0x02;
const ANON_REQ_TYPE_BASIC: u8 = 0x03;
const REQ_TYPE_GET_STATUS: u8 = 0x01;
const REQ_TYPE_GET_TELEMETRY_DATA: u8 = 0x03;
const REQ_TYPE_GET_NEIGHBOURS: u8 = 0x06;
const REQ_TYPE_GET_OWNER_INFO: u8 = 0x07;
const LPP_CHANNEL_SELF: u8 = 1;
const LPP_TYPE_VOLTAGE: u8 = 116;
const REPEATER_STATS_LEN: usize = 56;
const FIRMWARE_VERSION: &str = env!("MESHCORE_FIRMWARE_VERSION");

pub struct Cli {
    line: [u8; LINE_BUFFER_LEN],
    len: usize,
    last_was_carriage_return: bool,
}

pub enum SerialEcho {
    None,
    Byte,
    Bytes(&'static [u8]),
}

pub struct CliResponse {
    pub text: String,
    pub post_reply: Option<PostReplyAction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostReplyAction {
    Reboot,
}

impl Cli {
    pub const fn new() -> Self {
        Self {
            line: [0; LINE_BUFFER_LEN],
            len: 0,
            last_was_carriage_return: false,
        }
    }

    pub fn echo_for_byte(&mut self, byte: u8) -> SerialEcho {
        match byte {
            b'\r' => {
                self.last_was_carriage_return = true;
                SerialEcho::Bytes(b"\r\n")
            }
            b'\n' if self.last_was_carriage_return => {
                self.last_was_carriage_return = false;
                SerialEcho::None
            }
            b'\n' => SerialEcho::Bytes(b"\r\n"),
            0x08 | 0x7f if self.len > 0 => {
                self.last_was_carriage_return = false;
                SerialEcho::Bytes(b"\x08 \x08")
            }
            byte if !byte.is_ascii_control() && self.len < self.line.len() => {
                self.last_was_carriage_return = false;
                SerialEcho::Byte
            }
            _ => {
                self.last_was_carriage_return = false;
                SerialEcho::None
            }
        }
    }

    pub async fn accept_byte<Store>(&mut self, byte: u8, context: &AppContext<Store>)
    where
        Store: crate::platform::storage::Storage,
    {
        match byte {
            b'\r' | b'\n' => self.submit(context).await,
            0x08 | 0x7f => {
                self.len = self.len.saturating_sub(1);
            }
            byte if byte.is_ascii_control() => {}
            byte if self.len < self.line.len() => {
                self.line[self.len] = byte;
                self.len += 1;
            }
            _ => {
                crate::platform::log_fmt(format_args!("CLI: line too long"));
                self.len = 0;
            }
        }
    }

    async fn submit<Store>(&mut self, context: &AppContext<Store>)
    where
        Store: crate::platform::storage::Storage,
    {
        if self.len == 0 {
            return;
        }

        let command = String::from(core::str::from_utf8(&self.line[..self.len]).unwrap_or(""));
        self.len = 0;

        if let Some(response) = handle_command(command.trim(), context, CliRequest::serial()).await
            && response.post_reply == Some(PostReplyAction::Reboot)
        {
            crate::platform::reboot();
        }
    }
}

pub async fn handle_privileged_tcp_command<Store>(
    command: &str,
    context: &AppContext<Store>,
) -> Option<CliResponse>
where
    Store: crate::platform::storage::Storage,
{
    handle_command(command.trim(), context, CliRequest::tcp()).await
}

pub async fn handle_remote_packet<Store>(
    packet: &Packet,
    context: &AppContext<Store>,
) -> Option<alloc::vec::Vec<u8>>
where
    Store: crate::platform::storage::Storage,
{
    if let Payload::AnonymousRequest(payload) = &packet.payload {
        let reply_path = if packet.route_type.is_flood() {
            packet.normal_path()?.reversed()
        } else if packet.route_type.is_direct()
            && packet_targets_this_node(packet, &context.node_hash().await)
        {
            mcrs_protocol::Path::empty()
        } else {
            return None;
        };
        return handle_anonymous_request(payload, reply_path, context).await;
    }

    if !packet.route_type.is_direct()
        || !packet_targets_this_node(packet, &context.node_hash().await)
    {
        return None;
    }

    match &packet.payload {
        Payload::Request(payload) => handle_authenticated_request(payload, context).await,
        Payload::TextMessage(payload) => handle_authenticated_text_message(payload, context).await,
        _ => None,
    }
}

async fn handle_anonymous_request(
    payload: &mcrs_protocol::AnonymousRequestPayload,
    reply_path: mcrs_protocol::Path,
    context: &AppContext<impl crate::platform::storage::Storage>,
) -> Option<alloc::vec::Vec<u8>> {
    let (decrypted, responder_public_key) = context
        .with_identity(|identity| {
            let decrypted = super::crypto::decrypt_anonymous_request(payload, identity)?;
            Some((decrypted, *identity.public_key()))
        })
        .await?;

    let timestamp = plaintext_timestamp(&decrypted.plaintext)?;
    let body = plaintext_body(&decrypted.plaintext)?;

    if let Some(response) = handle_anonymous_subrequest(body, timestamp, context).await {
        return super::crypto::encode_response_plaintext(
            &decrypted.shared_secret,
            &decrypted.sender_pubkey,
            &responder_public_key,
            &response,
            reply_path,
        );
    }

    let Ok(body) = core::str::from_utf8(body) else {
        return None;
    };
    let body = body.trim();
    let now_ms = crate::platform::now_millis();

    let privilege = context
        .with_config(|config| login_privilege(body, config.remote_cli_password()))
        .await;
    if let Some(privilege) = privilege {
        context
            .authenticate_remote_login(
                &payload.sender_pubkey,
                &decrypted.shared_secret,
                privilege,
                timestamp,
                now_ms,
                &reply_path,
            )
            .await;
        let mut sender = String::new();
        append_hex(&mut sender, &payload.sender_pubkey);
        crate::platform::log_fmt(format_args!(
            "Remote login accepted: privilege={} pubkey={}",
            remote_privilege_name(privilege),
            sender
        ));
        return super::crypto::encode_login_response(
            &decrypted.shared_secret,
            &decrypted.sender_pubkey,
            &responder_public_key,
            acl_permissions_for(privilege),
            reply_path,
        );
    }

    let command = body.strip_prefix(REMOTE_CLI_PREFIX).map(str::trim)?;
    if command.is_empty() {
        return None;
    }

    let privilege = context
        .remote_privilege_for(&payload.sender_pubkey, now_ms)
        .await;
    let request = CliRequest::remote(cli_privilege_for_remote(privilege), timestamp);

    let response = handle_command(command, context, request).await?;
    if response.post_reply == Some(PostReplyAction::Reboot) {
        context.request_reboot_after_next_remote_reply();
    }
    encode_remote_cli_reply(
        &decrypted.shared_secret,
        &decrypted.sender_pubkey,
        &responder_public_key,
        timestamp,
        response.text,
        reply_path,
    )
}

async fn handle_authenticated_text_message(
    payload: &mcrs_protocol::DirectEncryptedPayload,
    context: &AppContext<impl crate::platform::storage::Storage>,
) -> Option<alloc::vec::Vec<u8>> {
    let now_ms = crate::platform::now_millis();
    let sessions = context
        .remote_sessions_matching_source_hash(payload.source_hash, now_ms)
        .await;
    let (decrypted, responder_public_key) = context
        .with_identity(|identity| {
            let decrypted =
                super::crypto::decrypt_authenticated_direct_payload(payload, identity, &sessions)?;
            Some((decrypted, *identity.public_key()))
        })
        .await?;
    let plaintext = TextMessagePlaintext::decode(&decrypted.plaintext).ok()?;

    if plaintext.text_type != TextType::CliData {
        return None;
    }
    if !context
        .accept_newer_remote_timestamp(
            &decrypted.sender_pubkey,
            decrypted.privilege,
            plaintext.timestamp,
            now_ms,
        )
        .await
    {
        return None;
    }

    let command = core::str::from_utf8(&plaintext.message).ok()?.trim();
    if command.is_empty() {
        return None;
    }

    let request = CliRequest::remote(
        cli_privilege_for_remote(Some(decrypted.privilege)),
        plaintext.timestamp,
    );
    let response = handle_command(command, context, request).await?;
    if response.post_reply == Some(PostReplyAction::Reboot) {
        context.request_reboot_after_next_remote_reply();
    }
    encode_remote_cli_reply(
        &decrypted.shared_secret,
        &decrypted.sender_pubkey,
        &responder_public_key,
        plaintext.timestamp,
        response.text,
        decrypted.reply_path,
    )
}

async fn handle_authenticated_request(
    payload: &mcrs_protocol::DirectEncryptedPayload,
    context: &AppContext<impl crate::platform::storage::Storage>,
) -> Option<Vec<u8>> {
    let now_ms = crate::platform::now_millis();
    let decrypted = decrypt_authenticated_payload(payload, context, now_ms).await?;
    let plaintext = RequestPlaintext::decode(&decrypted.plaintext).ok()?;
    let responder_public_key = context.public_key().await;

    if !context
        .accept_newer_remote_timestamp(
            &decrypted.sender_pubkey,
            decrypted.privilege,
            plaintext.timestamp,
            now_ms,
        )
        .await
    {
        return None;
    }

    let response_body =
        handle_binary_request(&plaintext, context, now_ms, decrypted.privilege).await?;
    super::crypto::encode_response_plaintext(
        &decrypted.shared_secret,
        &decrypted.sender_pubkey,
        &responder_public_key,
        &response_body,
        decrypted.reply_path,
    )
}

async fn decrypt_authenticated_payload(
    payload: &mcrs_protocol::DirectEncryptedPayload,
    context: &AppContext<impl crate::platform::storage::Storage>,
    now_ms: u64,
) -> Option<super::crypto::AuthenticatedPlaintext> {
    let sessions = context
        .remote_sessions_matching_source_hash(payload.source_hash, now_ms)
        .await;
    context
        .with_identity(|identity| {
            super::crypto::decrypt_authenticated_direct_payload(payload, identity, &sessions)
        })
        .await
}

async fn handle_binary_request(
    request: &RequestPlaintext,
    context: &AppContext<impl crate::platform::storage::Storage>,
    now_ms: u64,
    privilege: super::remote::RemotePrivilege,
) -> Option<Vec<u8>> {
    match request.request_data.first().copied()? {
        REQ_TYPE_GET_STATUS => {
            let mut response = Vec::new();
            response.extend_from_slice(&request.timestamp.to_le_bytes());
            encode_status_binary_response(context.status(), &mut response);
            Some(response)
        }
        REQ_TYPE_GET_TELEMETRY_DATA => {
            let mut response = Vec::new();
            response.extend_from_slice(&request.timestamp.to_le_bytes());
            if let Some(millivolts) = context.status().battery_millivolts {
                response.push(LPP_CHANNEL_SELF);
                response.push(LPP_TYPE_VOLTAGE);
                let centivolts = millivolts.saturating_add(5) / 10;
                response.extend_from_slice(&centivolts.to_be_bytes());
            }
            Some(response)
        }
        REQ_TYPE_GET_NEIGHBOURS => {
            let mut response = Vec::new();
            response.extend_from_slice(&request.timestamp.to_le_bytes());
            if !context
                .encode_neighbours_binary_response(&request.request_data, now_ms, &mut response)
                .await
            {
                return None;
            }
            Some(response)
        }
        REQ_TYPE_GET_OWNER_INFO => {
            let node_name = context
                .with_config(|config| String::from(config.node_name()))
                .await;
            let mut response = Vec::new();
            response.extend_from_slice(&request.timestamp.to_le_bytes());
            response.extend_from_slice(FIRMWARE_VERSION.as_bytes());
            response.push(b'\n');
            response.extend_from_slice(node_name.as_bytes());
            response.push(b'\n');
            Some(response)
        }
        request_type if privilege != super::remote::RemotePrivilege::Admin => {
            crate::platform::log_fmt(format_args!(
                "Remote request denied: type=0x{:02x} privilege={}",
                request_type,
                remote_privilege_name(privilege)
            ));
            None
        }
        request_type => {
            crate::platform::log_fmt(format_args!(
                "Remote request unsupported: type=0x{:02x}",
                request_type
            ));
            None
        }
    }
}

async fn handle_anonymous_subrequest(
    body: &[u8],
    request_timestamp: u32,
    context: &AppContext<impl crate::platform::storage::Storage>,
) -> Option<Vec<u8>> {
    match body.first().copied()? {
        ANON_REQ_TYPE_BASIC => Some(anonymous_basic_response(request_timestamp)),
        ANON_REQ_TYPE_REGIONS => {
            let regions = context
                .with_config(|config| config.regions().allowed_names())
                .await;
            let response = RepeaterResponsePlaintext {
                reflected_tag: request_timestamp,
                responder_time: crate::platform::now_seconds(),
                body: regions.into_bytes(),
            };
            Some(response.encode())
        }
        ANON_REQ_TYPE_OWNER => {
            let node_name = context
                .with_config(|config| String::from(config.node_name()))
                .await;
            Some(anonymous_owner_response(request_timestamp, &node_name))
        }
        request_type if request_type < b' ' => {
            crate::platform::log_fmt(format_args!(
                "Anonymous request unsupported: type=0x{:02x}",
                request_type
            ));
            None
        }
        _ => None,
    }
}

fn anonymous_basic_response(request_timestamp: u32) -> Vec<u8> {
    let response = RepeaterResponsePlaintext {
        reflected_tag: request_timestamp,
        responder_time: crate::platform::now_seconds(),
        body: vec![repeater_features()],
    };
    response.encode()
}

fn anonymous_owner_response(request_timestamp: u32, node_name: &str) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(node_name.as_bytes());
    body.push(b'\n');
    let response = RepeaterResponsePlaintext {
        reflected_tag: request_timestamp,
        responder_time: crate::platform::now_seconds(),
        body,
    };
    response.encode()
}

fn repeater_features() -> u8 {
    0
}

fn encode_status_binary_response(status: super::Status, out: &mut Vec<u8>) {
    let response_start = out.len();
    out.resize(response_start + REPEATER_STATS_LEN, 0);
    let stats = &mut out[response_start..response_start + REPEATER_STATS_LEN];
    let mut offset = 0;

    write_u16(stats, &mut offset, status.battery_millivolts.unwrap_or(0));
    write_u16(stats, &mut offset, status.outbound_queue_len);
    write_i16(stats, &mut offset, 0);
    write_i16(
        stats,
        &mut offset,
        status.last_rssi.clamp(i16::MIN, i16::MAX),
    );
    write_u32(stats, &mut offset, status.packets_received);
    write_u32(stats, &mut offset, status.packets_sent);
    write_u32(stats, &mut offset, status.tx_airtime_ms / 1_000);
    write_u32(
        stats,
        &mut offset,
        status.uptime_seconds.min(u32::MAX as u64) as u32,
    );
    write_u32(stats, &mut offset, 0);
    write_u32(stats, &mut offset, 0);
    write_u32(stats, &mut offset, 0);
    write_u32(stats, &mut offset, 0);
    write_u16(
        stats,
        &mut offset,
        status.packet_errors.min(u16::MAX as u32) as u16,
    );
    write_i16(
        stats,
        &mut offset,
        status.last_snr.clamp(i16::MIN, i16::MAX),
    );
    write_u16(stats, &mut offset, 0);
    write_u16(stats, &mut offset, 0);
    write_u32(stats, &mut offset, 0);
    write_u32(stats, &mut offset, status.packet_errors);
}

async fn handle_command(
    command: &str,
    context: &AppContext<impl crate::platform::storage::Storage>,
    request: CliRequest,
) -> Option<CliResponse> {
    if !request.allows_command(command) {
        let output = denied_text();
        return Some(command_response(request, output));
    }

    if let Some(seconds) = command.strip_prefix("time ").map(str::trim) {
        let output = handle_time_set_command(seconds, request);
        return Some(command_response(request, output));
    }

    if let Some(config) = command.strip_prefix("get ").map(str::trim) {
        let output = handle_get_command(config, context, request).await;
        return Some(command_response(request, output));
    }

    if let Some(config) = command.strip_prefix("set ").map(str::trim) {
        let output = handle_set_command(config, context, request).await;
        return Some(command_response(request, output));
    }

    if let Some(config) = command.strip_prefix("unset ").map(str::trim) {
        let output = handle_unset_command(config, context, request).await;
        return Some(command_response(request, output));
    }

    if command == "region" || command.starts_with("region ") {
        let output = handle_region_command(command, context, request).await;
        return Some(command_response(request, output));
    }

    if command == "ota" || command.starts_with("ota ") {
        let output = handle_ota_command(command, context, request);
        return Some(command_response(request, output));
    }

    if let Some(password) = command.strip_prefix("password ").map(str::trim) {
        let output = if request.privilege.is_passworded() {
            match context
                .update_config(|config| {
                    config.set_remote_cli_password(password);
                    Ok(())
                })
                .await
            {
                Ok(()) => {
                    let password = context
                        .with_config(|config| String::from(config.remote_cli_password()))
                        .await;
                    format!("Password now: {}", password)
                }
                Err(error) => format!("Error: {}", error),
            }
        } else {
            denied_text()
        };
        return Some(command_response(request, output));
    }

    let mut post_reply = None;
    let output = match command {
        "" => return None,
        "help" | "?" => help_text(),
        "reboot" => {
            if !request.privilege.is_passworded() {
                denied_text()
            } else {
                post_reply = Some(PostReplyAction::Reboot);
                String::from("OK - rebooting")
            }
        }
        "erase config" => {
            if !request.privilege.is_passworded() {
                denied_text()
            } else {
                match context.reset_config().await {
                    Ok(()) => {
                        post_reply = Some(PostReplyAction::Reboot);
                        String::from("OK - config reset, rebooting")
                    }
                    Err(error) => format!("Error - config reset failed: {:?}", error),
                }
            }
        }
        "identity" | "id" => {
            let mut output = String::new();
            context
                .with_config(|config| {
                    let _ = writeln!(output, "Identity key source: {}", config.identity_label());
                    let _ = writeln!(output, "Node name: {}", config.node_name());
                    let _ = writeln!(
                        output,
                        "Identity latitude: {}",
                        format_optional_coordinate(config.latitude_microdegrees())
                    );
                    let _ = writeln!(
                        output,
                        "Identity longitude: {}",
                        format_optional_coordinate(config.longitude_microdegrees())
                    );
                })
                .await;
            context
                .with_identity(|identity| {
                    append_hex_line(&mut output, "Identity public key: ", identity.public_key());
                    append_hex_line(&mut output, "Identity node hash: ", identity.node_hash());
                })
                .await;
            output
        }
        "neighbours" | "neighbors" => {
            let mut output = String::new();
            context.write_neighbours_summary(&mut output).await;
            output
        }
        "advert.zerohop" => {
            if !request.privilege.is_passworded() {
                denied_text()
            } else {
                let packet = context.with_config(super::discovery::zero_hop_advert).await;
                enqueue_command_packet(context, packet, "OK - zerohop advert sent").await
            }
        }
        "advert" => {
            if !request.privilege.is_passworded() {
                denied_text()
            } else {
                let packet = context.with_config(super::discovery::flood_advert).await;
                enqueue_command_packet(context, packet, "OK - Advert sent").await
            }
        }
        "discover.neighbours" | "discover.neighbors" => {
            if !request.privilege.is_passworded() {
                denied_text()
            } else {
                let now_ms = crate::platform::now_millis();
                let tag = discovery_tag(now_ms);
                let packet = super::discovery::discover_neighbours_request(tag);
                let output = enqueue_command_packet(context, packet, "OK - Discover sent").await;
                if output.starts_with("OK") {
                    context.start_discover_neighbours(tag, now_ms);
                }
                output
            }
        }
        "radio" => {
            let (radio, default_region) = context
                .with_config(|config| {
                    (
                        config.radio(),
                        config
                            .regions()
                            .default_region()
                            .map(|region| region.name)
                            .unwrap_or_else(|| String::from("<null>")),
                    )
                })
                .await;
            format!(
                "Radio: freq={} bw={} sf={} cr=4/{} tx_power={}dBm dutycycle={}% region.default={}",
                format_scaled(radio.receive_frequency_hz, 1_000_000, 3),
                format_scaled(radio.bandwidth_hz, 1_000, 3),
                radio.spreading_factor,
                radio.coding_rate_denominator,
                radio.transmit_power_dbm,
                context
                    .with_config(|config| config.duty_cycle_percent())
                    .await,
                default_region
            )
        }
        "clock" => format!("Clock: {}", crate::platform::now_seconds()),
        "clock sync" => {
            if !request.privilege.is_passworded() {
                denied_text()
            } else if request.sender_timestamp == 0 {
                String::from("Error - no remote timestamp available")
            } else if crate::platform::set_wall_clock_if_forward(
                request.sender_timestamp.saturating_add(1),
            ) {
                format!("OK - clock set: {}", crate::platform::now_seconds())
            } else {
                String::from("Error - clock cannot go backwards")
            }
        }
        "status" => status_text(context),
        "ver" | "version" => String::from(FIRMWARE_VERSION),
        _ => {
            let mut output = format!("CLI: unknown command '{}'\n", command);
            output.push_str(&help_text());
            output
        }
    };

    let mut response = command_response(request, output);
    response.post_reply = post_reply;
    Some(response)
}

fn command_response(request: CliRequest, text: String) -> CliResponse {
    log_command_output(request, &text);
    CliResponse {
        text,
        post_reply: None,
    }
}

async fn handle_get_command(
    config: &str,
    context: &AppContext<impl crate::platform::storage::Storage>,
    request: CliRequest,
) -> String {
    if let Some(key) = config.strip_prefix("wifi.") {
        if !request.origin.is_local() {
            return denied_text();
        }
        return context
            .with_config(|config| match key {
                "ssid" => format!("> {}", config.wifi().ssid()),
                "pass" => format!("> {}", config.wifi().password()),
                "telnet" => format!("> {}", on_off_text(config.wifi().telnet())),
                _ => format!("Unknown config: wifi.{key}"),
            })
            .await;
    }
    match config {
        "name" => {
            context
                .with_config(|config| format!("> {}", config.node_name()))
                .await
        }
        "radio" => {
            let radio = context.with_config(|config| config.radio()).await;
            format!(
                "> {},{},{},{}",
                format_scaled(radio.receive_frequency_hz, 1_000_000, 3),
                format_scaled(radio.bandwidth_hz, 1_000, 3),
                radio.spreading_factor,
                radio.coding_rate_denominator
            )
        }
        "tx" => {
            context
                .with_config(|config| format!("> {}", config.radio().transmit_power_dbm))
                .await
        }
        "dutycycle" => {
            context
                .with_config(|config| format!("> {}", config.duty_cycle_percent()))
                .await
        }
        "lat" => {
            context
                .with_config(|config| {
                    format!(
                        "> {}",
                        format_optional_coordinate(config.latitude_microdegrees())
                    )
                })
                .await
        }
        "lon" => {
            context
                .with_config(|config| {
                    format!(
                        "> {}",
                        format_optional_coordinate(config.longitude_microdegrees())
                    )
                })
                .await
        }
        "freq" => {
            context
                .with_config(|config| {
                    format!(
                        "> {}",
                        format_scaled(config.radio().receive_frequency_hz, 1_000_000, 3)
                    )
                })
                .await
        }
        "public.key" => {
            let mut output = String::from("> ");
            append_hex(&mut output, &context.public_key().await);
            output
        }
        "prv.key" if request.origin.is_local() => {
            let mut output = String::from("> ");
            let seed = context.with_config(|config| *config.identity_seed()).await;
            append_hex(&mut output, &seed);
            output
        }
        "prv.key" => denied_text(),
        "password" if request.origin.is_local() => {
            context
                .with_config(|config| format!("> {}", config.remote_cli_password()))
                .await
        }
        "password" => denied_text(),
        "flood.max.unscoped" => {
            context
                .with_config(|config| format!("> {}", config.flood_max_unscoped_hops()))
                .await
        }
        "flood.max.advert" => {
            context
                .with_config(|config| format!("> {}", config.flood_max_advert_hops()))
                .await
        }
        "path.hash.mode" => {
            context
                .with_config(|config| format!("> {}", config.path_hash_mode()))
                .await
        }
        "status" => status_text(context),
        _ => format!("Unknown config: {}", config),
    }
}

async fn handle_set_command(
    config: &str,
    context: &AppContext<impl crate::platform::storage::Storage>,
    request: CliRequest,
) -> String {
    if !request.privilege.is_passworded() {
        return denied_text();
    }

    if let Some(setting) = config.strip_prefix("wifi.") {
        if !request.origin.is_local() {
            return denied_text();
        }
        let (key, value) = match setting.split_once(' ') {
            Some((key, value)) => (key.trim(), value.trim()),
            None if setting.trim() == "pass" => ("pass", ""),
            None => return String::from("Error, missing Wi-Fi value"),
        };
        let result = match key {
            "ssid" => {
                context
                    .update_config(|config| config.set_wifi_ssid(value))
                    .await
            }
            "pass" => {
                context
                    .update_config(|config| config.set_wifi_password(value))
                    .await
            }
            "telnet" => {
                let Some(enabled) = parse_bool(value) else {
                    return String::from("Error, invalid wifi.telnet value");
                };
                context
                    .update_config(|config| {
                        config.set_wifi_telnet(enabled);
                        Ok(())
                    })
                    .await
            }
            _ => return String::from("Error, unknown Wi-Fi setting"),
        };
        return match result {
            Ok(()) => String::from("OK - reboot to apply"),
            Err(error) => format!("Error: {error}"),
        };
    }

    if let Some(name) = config.strip_prefix("name ").map(str::trim) {
        return match context
            .update_config(|config| config.set_node_name(name))
            .await
        {
            Ok(()) => {
                let name = context
                    .with_config(|config| String::from(config.node_name()))
                    .await;
                format!("OK - name now: {}", name)
            }
            Err(error) => format!("Error: {}", error),
        };
    }

    if let Some(password) = config.strip_prefix("password ").map(str::trim) {
        return match context
            .update_config(|config| {
                config.set_remote_cli_password(password);
                Ok(())
            })
            .await
        {
            Ok(()) => String::from("OK"),
            Err(error) => format!("Error: {}", error),
        };
    }

    if let Some(seconds) = config.strip_prefix("time ").map(str::trim) {
        return handle_time_set_command(seconds, request);
    }

    if let Some(latitude) = config.strip_prefix("lat ").map(str::trim) {
        let Some(latitude) = crate::app::config::parse_latitude_microdegrees(latitude) else {
            return String::from("Error, invalid latitude");
        };
        return match context
            .update_config(|config| config.set_latitude_microdegrees(latitude))
            .await
        {
            Ok(()) => format!(
                "OK - latitude now: {}",
                format_optional_coordinate(
                    context
                        .with_config(|config| config.latitude_microdegrees())
                        .await
                )
            ),
            Err(error) => format!("Error: {}", error),
        };
    }

    if let Some(longitude) = config.strip_prefix("lon ").map(str::trim) {
        let Some(longitude) = crate::app::config::parse_longitude_microdegrees(longitude) else {
            return String::from("Error, invalid longitude");
        };
        return match context
            .update_config(|config| config.set_longitude_microdegrees(longitude))
            .await
        {
            Ok(()) => format!(
                "OK - longitude now: {}",
                format_optional_coordinate(
                    context
                        .with_config(|config| config.longitude_microdegrees())
                        .await
                )
            ),
            Err(error) => format!("Error: {}", error),
        };
    }

    if let Some(hex) = config.strip_prefix("prv.key ").map(str::trim) {
        let Some(seed) = parse_hex_seed(hex) else {
            return String::from("Error, bad key");
        };

        return match context
            .update_config(|config| {
                config.set_identity_seed(seed);
                Ok(())
            })
            .await
        {
            Ok(()) => {
                let new_identity = super::identity::Identity::from_private_key_seed(&seed);
                let mut output = String::from("OK, reboot to apply! New pubkey: ");
                append_hex(&mut output, new_identity.public_key());
                output
            }
            Err(error) => format!("Error: {}", error),
        };
    }

    if let Some(radio_params) = config.strip_prefix("radio ").map(str::trim) {
        let current_radio = context.with_config(|config| config.radio()).await;
        let Some(radio) = parse_radio_config(radio_params, current_radio) else {
            return String::from("Error, invalid radio params");
        };

        return match context
            .update_config(|config| config.set_radio(radio))
            .await
        {
            Ok(()) => String::from("OK - reboot to apply"),
            Err(error) => format!("Error: {}", error),
        };
    }

    if let Some(tx_power) = config.strip_prefix("tx ").map(str::trim) {
        let Ok(tx_power) = tx_power.trim().parse::<i32>() else {
            return String::from("Error, invalid tx power");
        };
        return match context
            .update_config(|config| config.set_transmit_power_dbm(tx_power))
            .await
        {
            Ok(()) => String::from("OK - reboot to apply"),
            Err(error) => format!("Error: {}", error),
        };
    }

    if let Some(percent) = config.strip_prefix("dutycycle ").map(str::trim) {
        let Ok(percent) = percent.parse::<u8>() else {
            return String::from("Error, invalid duty cycle percentage");
        };
        return match context
            .update_config(|config| config.set_duty_cycle_percent(percent))
            .await
        {
            Ok(()) => format!("OK - dutycycle now: {}%", percent),
            Err(error) => format!("Error: {}", error),
        };
    }

    if let Some(hops) = config.strip_prefix("flood.max.unscoped ").map(str::trim) {
        let Some(hops) = crate::app::config::parse_flood_max_hops(hops) else {
            return String::from("Error, invalid flood max hops");
        };
        return match context
            .update_config(|config| config.set_flood_max_unscoped_hops(hops))
            .await
        {
            Ok(()) => format!("OK - flood.max.unscoped now: {}", hops),
            Err(error) => format!("Error: {}", error),
        };
    }

    if let Some(hops) = config.strip_prefix("flood.max.advert ").map(str::trim) {
        let Some(hops) = crate::app::config::parse_flood_max_hops(hops) else {
            return String::from("Error, invalid flood max hops");
        };
        return match context
            .update_config(|config| config.set_flood_max_advert_hops(hops))
            .await
        {
            Ok(()) => format!("OK - flood.max.advert now: {}", hops),
            Err(error) => format!("Error: {}", error),
        };
    }

    if let Some(mode) = config.strip_prefix("path.hash.mode ").map(str::trim) {
        let Some(mode) = crate::app::config::parse_path_hash_mode(mode) else {
            return String::from("Error, invalid path hash mode");
        };
        return match context
            .update_config(|config| config.set_path_hash_mode(mode))
            .await
        {
            Ok(()) => format!("OK - path.hash.mode now: {}", mode),
            Err(error) => format!("Error: {}", error),
        };
    }

    if request.origin.is_local()
        && let Some(freq) = config.strip_prefix("freq ").map(str::trim)
    {
        let Some(receive_frequency_hz) = parse_decimal_scaled(freq, 1_000_000) else {
            return String::from("Error, invalid frequency");
        };
        let mut radio = context.with_config(|config| config.radio()).await;
        radio.receive_frequency_hz = receive_frequency_hz;
        return match context
            .update_config(|config| config.set_radio(radio))
            .await
        {
            Ok(()) => String::from("OK - reboot to apply"),
            Err(error) => format!("Error: {}", error),
        };
    }

    format!("Unknown config: {}", config)
}

async fn handle_unset_command(
    setting: &str,
    context: &AppContext<impl crate::platform::storage::Storage>,
    request: CliRequest,
) -> String {
    if !request.privilege.is_passworded() {
        return denied_text();
    }
    if setting.is_empty() {
        return String::from("Error, missing config name");
    }
    if (setting.starts_with("wifi.") || setting == "freq") && !request.origin.is_local() {
        return denied_text();
    }

    match context.update_config(|config| config.unset(setting)).await {
        Ok(())
            if matches!(
                setting,
                "wifi.ssid" | "wifi.pass" | "wifi.telnet" | "radio" | "freq" | "tx"
            ) =>
        {
            String::from("OK - reset to default; reboot to apply")
        }
        Ok(()) => String::from("OK - reset to default"),
        Err(super::ConfigUpdateError::Invalid(super::config::ConfigError::UnknownSetting)) => {
            format!("Unknown config: {}", setting)
        }
        Err(error) => format!("Error: {}", error),
    }
}

async fn handle_region_command(
    command: &str,
    context: &AppContext<impl crate::platform::storage::Storage>,
    request: CliRequest,
) -> String {
    let mut parts = command.split_ascii_whitespace();
    let _region = parts.next();
    let Some(action) = parts.next() else {
        let mut output = String::new();
        context
            .with_config(|config| config.regions().write_tree(&mut output))
            .await;
        return output;
    };

    match action {
        "list" => match parts.next() {
            Some("allowed") => {
                let names = context
                    .with_config(|config| config.regions().allowed_names())
                    .await;
                if names.is_empty() {
                    String::from("-none-")
                } else {
                    names
                }
            }
            Some("denied") => {
                let names = context
                    .with_config(|config| config.regions().denied_names())
                    .await;
                if names.is_empty() {
                    String::from("-none-")
                } else {
                    names
                }
            }
            _ => String::from("Err - use 'allowed' or 'denied'"),
        },
        "get" => {
            let Some(name) = parts.next() else {
                return String::from("Err - missing region");
            };
            let region = context
                .with_config(|config| {
                    config.regions().find_by_name_prefix(name).map(|region| {
                        let allows_flood = region.allows_flood();
                        (String::from(region.name.as_str()), allows_flood)
                    })
                })
                .await;
            match region {
                Some(region) => format!(" {} {}", region.0, if region.1 { "F" } else { "" }),
                None => String::from("Err - unknown region"),
            }
        }
        "default" => {
            let Some(name) = parts.next() else {
                let name = context
                    .with_config(|config| {
                        config
                            .regions()
                            .default_region()
                            .map(|region| region.name)
                            .unwrap_or_else(|| String::from("<null>"))
                    })
                    .await;
                return format!("Default scope is {}", name);
            };
            if !request.privilege.is_passworded() {
                return denied_text();
            }
            let name = (name != "<null>").then_some(name);
            match context
                .update_config(|config| config.set_default_region(name))
                .await
            {
                Ok(()) => {
                    let name = context
                        .with_config(|config| {
                            config
                                .regions()
                                .default_region()
                                .map(|region| region.name)
                                .unwrap_or_else(|| String::from("<null>"))
                        })
                        .await;
                    format!("Default scope is now {}", name)
                }
                Err(error) => format!("Error: {}", error),
            }
        }
        "capture" => {
            let Some(value) = parts.next() else {
                let enabled = context.with_config(|config| config.region_capture()).await;
                return format!("Region capture is {}", on_off_text(enabled));
            };
            if !request.privilege.is_passworded() {
                return denied_text();
            }
            let Some(enabled) = parse_bool(value) else {
                return String::from("Err - use 'on' or 'off'");
            };
            match context
                .update_config(|config| config.set_region_capture(enabled))
                .await
            {
                Ok(()) => format!("Region capture is now {}", on_off_text(enabled)),
                Err(error) => format!("Error: {}", error),
            }
        }
        "put" => {
            if !request.privilege.is_passworded() {
                return denied_text();
            }
            let Some(name) = parts.next() else {
                return String::from("Err - missing region");
            };
            match context
                .update_config(|config| config.put_region(name))
                .await
            {
                Ok(()) => String::from("OK - (flood allowed)"),
                Err(error) => format!("Error: {}", error),
            }
        }
        "remove" => {
            if !request.privilege.is_passworded() {
                return denied_text();
            }
            let Some(name) = parts.next() else {
                return String::from("Err - missing region");
            };
            match context
                .update_config(|config| config.remove_region(name))
                .await
            {
                Ok(()) => String::from("OK"),
                Err(error) => format!("Error: {}", error),
            }
        }
        "allowf" | "denyf" => {
            if !request.privilege.is_passworded() {
                return denied_text();
            }
            let Some(name) = parts.next() else {
                return String::from("Err - missing region");
            };
            let allowed = action == "allowf";
            match context
                .update_config(|config| config.set_region_flood_allowed(name, allowed))
                .await
            {
                Ok(()) => String::from("OK"),
                Err(error) => format!("Error: {}", error),
            }
        }
        "save" => String::from("OK"),
        "load" => String::from("OK - use app.conf"),
        _ => String::from("Err - ??"),
    }
}

fn handle_ota_command(
    command: &str,
    context: &AppContext<impl crate::platform::storage::Storage>,
    request: CliRequest,
) -> String {
    let mut parts = command.split_ascii_whitespace();
    let _ota = parts.next();
    match parts.next() {
        None | Some("status") => ota_status_text(),
        Some("start") => {
            if !request.privilege.is_passworded() {
                denied_text()
            } else {
                context.request_ota_start();
                String::from("OTA: start requested")
            }
        }
        Some("stop") => {
            if !request.privilege.is_passworded() {
                denied_text()
            } else {
                context.request_ota_stop();
                String::from("OTA: stop requested")
            }
        }
        _ => String::from("Err - use 'ota status', 'ota start', or 'ota stop'"),
    }
}

fn ota_status_text() -> String {
    let status = crate::platform::ota_status();
    if !status.available {
        return String::from("OTA: unavailable");
    }

    let next = status.next.unwrap_or("unknown");
    format!(
        "OTA: selected={} next={} next_size={}",
        status.selected, next, status.next_size
    )
}

fn handle_time_set_command(seconds: &str, request: CliRequest) -> String {
    if !request.privilege.is_passworded() {
        return denied_text();
    }

    let Ok(seconds) = seconds.trim().parse::<u32>() else {
        return String::from("Error - bad time");
    };

    if crate::platform::set_wall_clock_if_forward(seconds) {
        format!("OK - clock set: {}", crate::platform::now_seconds())
    } else {
        String::from("Error - clock cannot go backwards")
    }
}

fn parse_radio_config(
    input: &str,
    mut current: crate::app::config::RadioConfig,
) -> Option<crate::app::config::RadioConfig> {
    let mut parts = input.split(|byte: char| byte.is_ascii_whitespace() || byte == ',');
    let frequency = parts.next().filter(|part| !part.is_empty())?;
    let bandwidth = parts.next().filter(|part| !part.is_empty())?;
    let spreading_factor = parts.next()?.parse::<u8>().ok()?;
    let coding_rate_denominator = parts.next()?.parse::<u8>().ok()?;

    if parts.any(|part| !part.is_empty()) {
        return None;
    }

    current.receive_frequency_hz = parse_decimal_scaled(frequency, 1_000_000)?;
    current.bandwidth_hz = parse_decimal_scaled(bandwidth, 1_000)?;
    current.spreading_factor = spreading_factor;
    current.coding_rate_denominator = coding_rate_denominator;
    Some(current)
}

fn parse_decimal_scaled(input: &str, scale: u32) -> Option<u32> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    let mut whole = 0u64;
    let mut fraction = 0u64;
    let mut fraction_scale = 1u64;
    let mut seen_dot = false;

    for byte in input.bytes() {
        match byte {
            b'0'..=b'9' if !seen_dot => {
                whole = whole.checked_mul(10)?.checked_add((byte - b'0') as u64)?;
            }
            b'0'..=b'9' => {
                if fraction_scale < scale as u64 {
                    fraction = fraction
                        .checked_mul(10)?
                        .checked_add((byte - b'0') as u64)?;
                    fraction_scale *= 10;
                }
            }
            b'.' if !seen_dot => seen_dot = true,
            _ => return None,
        }
    }

    let scaled_whole = whole.checked_mul(scale as u64)?;
    let scaled_fraction = if seen_dot {
        fraction
            .checked_mul(scale as u64)?
            .checked_div(fraction_scale)?
    } else {
        0
    };
    u32::try_from(scaled_whole.checked_add(scaled_fraction)?).ok()
}

fn parse_hex_seed(input: &str) -> Option<[u8; 32]> {
    let input = input.trim();
    if input.len() != 64 {
        return None;
    }

    let mut seed = [0u8; 32];
    let input = input.as_bytes();
    for index in 0..seed.len() {
        let high = hex_value(input[index * 2])?;
        let low = hex_value(input[index * 2 + 1])?;
        seed[index] = (high << 4) | low;
    }
    Some(seed)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn format_scaled(value: u32, scale: u32, decimals: usize) -> String {
    let whole = value / scale;
    let mut fraction = value % scale;
    let mut divisor = scale / 10;
    let mut output = format!("{}.", whole);

    for _ in 0..decimals {
        let digit = fraction.checked_div(divisor).unwrap_or(0);
        output.push((b'0' + digit as u8) as char);
        if divisor != 0 {
            fraction %= divisor;
            divisor /= 10;
        }
    }

    output
}

fn format_optional_coordinate(value: Option<i32>) -> String {
    let Some(value) = value else {
        return String::from("<null>");
    };

    let value = i64::from(value);
    let sign = if value < 0 { "-" } else { "" };
    let absolute = value.abs();
    format!(
        "{}{}.{:06}",
        sign,
        absolute / 1_000_000,
        absolute % 1_000_000
    )
}

fn plaintext_body(plaintext: &[u8]) -> Option<&[u8]> {
    if plaintext.len() < 4 {
        return None;
    }
    Some(&plaintext[4..])
}

fn plaintext_timestamp(plaintext: &[u8]) -> Option<u32> {
    let timestamp = plaintext.get(..4)?;
    Some(u32::from_le_bytes(timestamp.try_into().ok()?))
}

fn login_privilege(body: &str, password: &str) -> Option<super::remote::RemotePrivilege> {
    if body.is_empty() || body == "login" {
        return Some(super::remote::RemotePrivilege::Guest);
    }

    if body == password {
        return Some(super::remote::RemotePrivilege::Admin);
    }

    let candidate = body.strip_prefix(REMOTE_LOGIN_PREFIX)?.trim();
    if candidate.is_empty() {
        Some(super::remote::RemotePrivilege::Guest)
    } else if candidate == password {
        Some(super::remote::RemotePrivilege::Admin)
    } else {
        None
    }
}

fn cli_privilege_for_remote(privilege: Option<super::remote::RemotePrivilege>) -> CliPrivilege {
    match privilege {
        Some(super::remote::RemotePrivilege::Admin) => CliPrivilege::PasswordedRemote,
        Some(super::remote::RemotePrivilege::Guest) | None => CliPrivilege::AnonymousRemote,
    }
}

fn acl_permissions_for(privilege: super::remote::RemotePrivilege) -> u8 {
    match privilege {
        super::remote::RemotePrivilege::Admin => PERM_ACL_ADMIN,
        super::remote::RemotePrivilege::Guest => PERM_ACL_GUEST,
    }
}

fn remote_privilege_name(privilege: super::remote::RemotePrivilege) -> &'static str {
    match privilege {
        super::remote::RemotePrivilege::Admin => "admin",
        super::remote::RemotePrivilege::Guest => "guest",
    }
}

fn packet_targets_this_node(packet: &Packet, node_hash: &[u8]) -> bool {
    let Some(path) = packet.normal_path() else {
        return false;
    };

    path.hop_count() == 0 || path.first_hash_matches(node_hash)
}

fn parse_bool(input: &str) -> Option<bool> {
    match input {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn on_off_text(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

fn encode_remote_cli_reply(
    shared_secret: &[u8; 32],
    requester_public_key: &[u8; 32],
    responder_public_key: &[u8; 32],
    request_timestamp: u32,
    mut output: String,
    reply_path: mcrs_protocol::Path,
) -> Option<alloc::vec::Vec<u8>> {
    truncate_to_remote_reply_len(&mut output);
    super::crypto::encode_cli_text_response(
        shared_secret,
        requester_public_key,
        responder_public_key,
        request_timestamp,
        output.as_bytes(),
        reply_path,
    )
}

fn log_command_output(request: CliRequest, output: &str) {
    match request.origin {
        CliOrigin::Serial => {
            for line in output.lines() {
                crate::platform::log_fmt(format_args!("{}", line));
            }
        }
        CliOrigin::Tcp => {}
        CliOrigin::Remote => {
            if output.starts_with("CLI: denied") {
                crate::platform::log_fmt(format_args!(
                    "Remote CLI denied: privilege={}",
                    request.privilege.name()
                ));
            } else if output.starts_with("CLI: unknown command") {
                crate::platform::log_fmt(format_args!(
                    "Remote CLI unknown command: privilege={}",
                    request.privilege.name()
                ));
            }
        }
    }
}

fn denied_text() -> String {
    String::from("CLI: denied, command requires passworded login")
}

fn help_text() -> String {
    String::from(
        "Commands: help, ver, status, identity, radio, clock, region, region list {allowed|denied}, ota status, get {name|lat|lon|radio|tx|dutycycle|freq|flood.max.unscoped|flood.max.advert|path.hash.mode|public.key|status}; Privileged: time, clock sync, set, unset, password, neighbours, advert, advert.zerohop, discover.neighbours, region {put|remove|allowf|denyf|default}, ota {start|stop}, erase config, reboot",
    )
}

fn status_text(context: &AppContext<impl crate::platform::storage::Storage>) -> String {
    let status = context.status();
    let mut output = String::new();
    let _ = writeln!(output, "Uptime: {}s", status.uptime_seconds);
    let _ = writeln!(output, "Packets received: {}", status.packets_received);
    let _ = writeln!(output, "Packets sent: {}", status.packets_sent);
    let _ = writeln!(output, "TX airtime: {}ms", status.tx_airtime_ms);
    let airtime_percent_x100 = if status.uptime_seconds == 0 {
        0
    } else {
        (status.tx_airtime_ms as u64 * 10 / status.uptime_seconds.min(u32::MAX as u64)).min(10_000)
            as u32
    };
    let _ = writeln!(
        output,
        "TX airtime percent: {}.{:02}%",
        airtime_percent_x100 / 100,
        airtime_percent_x100 % 100
    );
    let _ = writeln!(output, "Packet errors: {}", status.packet_errors);
    let _ = writeln!(output, "Outbound queue: {}", status.outbound_queue_len);
    if let Some(millivolts) = status.battery_millivolts {
        let _ = writeln!(output, "Battery: {}mV", millivolts);
        return output;
    }
    match status.battery_level_percent {
        Some(level) => {
            let _ = writeln!(output, "Battery: {}%", level);
        }
        None => {
            let _ = writeln!(output, "Battery: unknown");
        }
    }
    output
}
async fn enqueue_command_packet(
    context: &AppContext<impl crate::platform::storage::Storage>,
    packet: Option<Vec<u8>>,
    success: &str,
) -> String {
    let Some(packet) = packet else {
        return String::from("Error - packet encode failed");
    };

    let len = packet.len();
    let region = context.outbound_region_label(&packet).await;
    match context.enqueue_outbound(packet) {
        Ok(()) => {
            match region {
                Some(region) => crate::platform::log_fmt(format_args!(
                    "CLI: queued packet {} bytes region={}",
                    len, region
                )),
                None => crate::platform::log_fmt(format_args!("CLI: queued packet {} bytes", len)),
            }
            String::from(success)
        }
        Err(_) => String::from("Error - outbound queue full"),
    }
}

fn discovery_tag(now_ms: u64) -> u32 {
    let tag = (now_ms as u32) ^ ((now_ms >> 32) as u32).rotate_left(13);
    if tag == 0 { 1 } else { tag }
}

fn append_hex_line(output: &mut String, label: &str, bytes: &[u8]) {
    output.push_str(label);
    append_hex(output, bytes);
    output.push('\n');
}

fn append_hex(output: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
}

fn write_u16(out: &mut [u8], offset: &mut usize, value: u16) {
    out[*offset..*offset + 2].copy_from_slice(&value.to_le_bytes());
    *offset += 2;
}

fn write_i16(out: &mut [u8], offset: &mut usize, value: i16) {
    out[*offset..*offset + 2].copy_from_slice(&value.to_le_bytes());
    *offset += 2;
}

fn write_u32(out: &mut [u8], offset: &mut usize, value: u32) {
    out[*offset..*offset + 4].copy_from_slice(&value.to_le_bytes());
    *offset += 4;
}

fn truncate_to_remote_reply_len(output: &mut String) {
    if output.len() <= REMOTE_CLI_REPLY_MAX_LEN {
        return;
    }

    output.truncate(REMOTE_CLI_REPLY_MAX_LEN);
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct CliRequest {
    origin: CliOrigin,
    privilege: CliPrivilege,
    sender_timestamp: u32,
}

impl CliRequest {
    const fn serial() -> Self {
        Self {
            origin: CliOrigin::Serial,
            privilege: CliPrivilege::Local,
            sender_timestamp: 0,
        }
    }

    const fn remote(privilege: CliPrivilege, sender_timestamp: u32) -> Self {
        Self {
            origin: CliOrigin::Remote,
            privilege,
            sender_timestamp,
        }
    }

    const fn tcp() -> Self {
        Self {
            origin: CliOrigin::Tcp,
            privilege: CliPrivilege::Local,
            sender_timestamp: 0,
        }
    }

    fn allows_command(self, command: &str) -> bool {
        if self.origin != CliOrigin::Remote || self.privilege != CliPrivilege::AnonymousRemote {
            return true;
        }

        matches!(
            command,
            "status" | "neighbours" | "neighbors" | "get status"
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CliOrigin {
    Serial,
    Tcp,
    Remote,
}

impl CliOrigin {
    fn is_local(self) -> bool {
        matches!(self, Self::Serial | Self::Tcp)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CliPrivilege {
    Local,
    AnonymousRemote,
    PasswordedRemote,
}

impl CliPrivilege {
    fn is_passworded(self) -> bool {
        matches!(self, Self::Local | Self::PasswordedRemote)
    }

    fn name(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::AnonymousRemote => "anonymous",
            Self::PasswordedRemote => "passworded",
        }
    }
}
