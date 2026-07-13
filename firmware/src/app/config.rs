extern crate alloc;

use alloc::{string::String, vec::Vec};
use core::fmt::{self, Write};

use super::{
    identity::{self, Identity},
    regions::{RegionError, RegionMap},
};

const APP_CONFIG_KEY: &str = "app.conf";
const APP_CONFIG_TEXT_VERSION: u8 = 1;
const APP_CONFIG_MAX_LEN: usize = 4096;
const MAX_REMOTE_PASSWORD_LEN: usize = 64;
const MAX_WIFI_SSID_LEN: usize = 32;
const MIN_WIFI_PASSWORD_LEN: usize = 8;
const MAX_WIFI_PASSWORD_LEN: usize = 63;
const MAX_NODE_NAME_LEN: usize = 31;
const DEFAULT_FLOOD_MAX_UNSCOPED_HOPS: u8 = 5;
const DEFAULT_FLOOD_MAX_ADVERT_HOPS: u8 = 3;
const DEFAULT_PATH_HASH_MODE: u8 = 2;
const DEFAULT_DUTY_CYCLE_PERCENT: u8 = 10;
const UNPROVISIONED_CONFIG_TEXT: &[u8] = b"# MCRS app.conf\nversion=1\n";
const COORDINATE_SCALE: i32 = 1_000_000;
const MIN_LATITUDE_MICRODEGREES: i32 = -90 * COORDINATE_SCALE;
const MAX_LATITUDE_MICRODEGREES: i32 = 90 * COORDINATE_SCALE;
const MIN_LONGITUDE_MICRODEGREES: i32 = -180 * COORDINATE_SCALE;
const MAX_LONGITUDE_MICRODEGREES: i32 = 180 * COORDINATE_SCALE;

pub struct AppConfig {
    identity: Identity,
    identity_seed: [u8; 32],
    identity_label: &'static str,
    latitude_microdegrees: Option<i32>,
    longitude_microdegrees: Option<i32>,
    node_name: String,
    remote_cli_password: String,
    wifi: WifiConfig,
    radio: RadioConfig,
    regions: RegionMap,
    region_capture: bool,
    flood_max_unscoped_hops: u8,
    flood_max_advert_hops: u8,
    path_hash_mode: u8,
    duty_cycle_percent: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WifiConfig {
    ssid: String,
    password: String,
    telnet: bool,
}

impl WifiConfig {
    pub fn ssid(&self) -> &str {
        &self.ssid
    }
    pub fn password(&self) -> &str {
        &self.password
    }
    pub fn telnet(&self) -> bool {
        self.telnet
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RadioConfig {
    pub receive_frequency_hz: u32,
    pub spreading_factor: u8,
    pub bandwidth_hz: u32,
    pub coding_rate_denominator: u8,
    pub transmit_power_dbm: i32,
}

impl AppConfig {
    pub fn generated_defaults(identity_seed: [u8; 32]) -> Self {
        Self::from_stored(
            StoredAppConfig::default_with_identity_seed(identity_seed),
            "generated",
        )
    }

    pub fn load_or_create<S>(storage: &mut S, defaults: Self) -> Self
    where
        S: crate::platform::storage::Storage,
    {
        let default_identity_label = defaults.identity_label;
        let default_stored = StoredAppConfig::from_app_config(&defaults);

        let (stored, identity_label) = match load_config_file(storage, &default_stored) {
            Some((stored, needs_rewrite)) => {
                if needs_rewrite {
                    let _ = write_config_file(storage, &stored);
                }
                (stored, "storage")
            }
            None => {
                let _ = write_config_file(storage, &default_stored);
                (default_stored, default_identity_label)
            }
        };

        Self::from_stored(stored, identity_label)
    }

    fn from_stored(stored: StoredAppConfig, identity_label: &'static str) -> Self {
        Self {
            identity: Identity::from_private_key_seed(&stored.identity_seed),
            identity_seed: stored.identity_seed,
            identity_label,
            latitude_microdegrees: stored.latitude_microdegrees,
            longitude_microdegrees: stored.longitude_microdegrees,
            node_name: stored.node_name,
            remote_cli_password: stored.remote_cli_password,
            wifi: stored.wifi,
            radio: stored.radio,
            regions: stored.regions,
            region_capture: stored.region_capture,
            flood_max_unscoped_hops: stored.flood_max_unscoped_hops,
            flood_max_advert_hops: stored.flood_max_advert_hops,
            path_hash_mode: stored.path_hash_mode,
            duty_cycle_percent: stored.duty_cycle_percent,
        }
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub fn identity_label(&self) -> &'static str {
        self.identity_label
    }

    pub fn identity_seed(&self) -> &[u8; 32] {
        &self.identity_seed
    }

    pub fn latitude_microdegrees(&self) -> Option<i32> {
        self.latitude_microdegrees
    }

    pub fn longitude_microdegrees(&self) -> Option<i32> {
        self.longitude_microdegrees
    }

    pub fn location(&self) -> Option<(i32, i32)> {
        Some((self.latitude_microdegrees?, self.longitude_microdegrees?))
    }

    pub fn node_name(&self) -> &str {
        &self.node_name
    }

    pub fn remote_cli_password(&self) -> &str {
        &self.remote_cli_password
    }

    pub fn wifi(&self) -> &WifiConfig {
        &self.wifi
    }

    pub fn set_wifi_ssid(&mut self, value: &str) -> Result<(), ConfigError> {
        if value.len() > MAX_WIFI_SSID_LEN {
            return Err(ConfigError::InvalidWifiConfig);
        }
        self.wifi.ssid = value.into();
        Ok(())
    }

    pub fn set_wifi_password(&mut self, value: &str) -> Result<(), ConfigError> {
        if !(value.is_empty()
            || (MIN_WIFI_PASSWORD_LEN..=MAX_WIFI_PASSWORD_LEN).contains(&value.len()))
        {
            return Err(ConfigError::InvalidWifiConfig);
        }
        self.wifi.password = value.into();
        Ok(())
    }

    pub fn set_wifi_telnet(&mut self, enabled: bool) {
        self.wifi.telnet = enabled;
    }

    pub fn radio(&self) -> RadioConfig {
        self.radio
    }

    pub fn regions(&self) -> &RegionMap {
        &self.regions
    }

    pub fn region_capture(&self) -> bool {
        self.region_capture
    }

    pub fn flood_max_unscoped_hops(&self) -> u8 {
        self.flood_max_unscoped_hops
    }

    pub fn flood_max_advert_hops(&self) -> u8 {
        self.flood_max_advert_hops
    }

    pub fn path_hash_mode(&self) -> u8 {
        self.path_hash_mode
    }

    pub fn duty_cycle_percent(&self) -> u8 {
        self.duty_cycle_percent
    }

    pub fn set_node_name(&mut self, node_name: &str) -> Result<(), ConfigError> {
        self.node_name = fit_node_name(node_name)?;
        Ok(())
    }

    pub fn set_remote_cli_password(&mut self, password: &str) {
        self.remote_cli_password = fit_password(password);
    }

    pub fn set_identity_seed(&mut self, identity_seed: [u8; 32]) {
        self.identity_seed = identity_seed;
    }

    pub fn set_latitude_microdegrees(
        &mut self,
        latitude_microdegrees: Option<i32>,
    ) -> Result<(), ConfigError> {
        if let Some(latitude) = latitude_microdegrees {
            validate_latitude(latitude)?;
        }
        self.latitude_microdegrees = latitude_microdegrees;
        Ok(())
    }

    pub fn set_longitude_microdegrees(
        &mut self,
        longitude_microdegrees: Option<i32>,
    ) -> Result<(), ConfigError> {
        if let Some(longitude) = longitude_microdegrees {
            validate_longitude(longitude)?;
        }
        self.longitude_microdegrees = longitude_microdegrees;
        Ok(())
    }

    pub fn set_radio(&mut self, radio: RadioConfig) -> Result<(), ConfigError> {
        radio.validate()?;
        self.radio = radio;
        Ok(())
    }

    pub fn set_transmit_power_dbm(&mut self, transmit_power_dbm: i32) -> Result<(), ConfigError> {
        self.radio.transmit_power_dbm = transmit_power_dbm;
        self.radio.validate()
    }

    pub fn set_duty_cycle_percent(&mut self, percent: u8) -> Result<(), ConfigError> {
        if !(1..=100).contains(&percent) {
            return Err(ConfigError::InvalidDutyCycle);
        }
        self.duty_cycle_percent = percent;
        Ok(())
    }

    pub fn put_region(&mut self, name: &str) -> Result<(), ConfigError> {
        Ok(self.regions.put_region(name)?)
    }

    pub fn remove_region(&mut self, name: &str) -> Result<(), ConfigError> {
        Ok(self.regions.remove_region(name)?)
    }

    pub fn set_region_flood_allowed(
        &mut self,
        name: &str,
        allowed: bool,
    ) -> Result<(), ConfigError> {
        Ok(self.regions.set_flood_allowed(name, allowed)?)
    }

    pub fn set_default_region(&mut self, name: Option<&str>) -> Result<(), ConfigError> {
        if let Some(name) = name
            && name != "*"
            && self.regions.find_by_name_prefix(name).is_none()
        {
            self.regions.put_region(name)?;
        }
        Ok(self.regions.set_default(name)?)
    }

    pub fn set_region_capture(&mut self, enabled: bool) -> Result<(), ConfigError> {
        self.region_capture = enabled;
        Ok(())
    }

    pub fn set_flood_max_unscoped_hops(&mut self, hops: u8) -> Result<(), ConfigError> {
        validate_flood_max_hops(hops)?;
        self.flood_max_unscoped_hops = hops;
        Ok(())
    }

    pub fn set_flood_max_advert_hops(&mut self, hops: u8) -> Result<(), ConfigError> {
        validate_flood_max_hops(hops)?;
        self.flood_max_advert_hops = hops;
        Ok(())
    }

    pub fn set_path_hash_mode(&mut self, mode: u8) -> Result<(), ConfigError> {
        validate_path_hash_mode(mode)?;
        self.path_hash_mode = mode;
        Ok(())
    }

    pub fn save<S>(&self, storage: &mut S) -> Result<(), crate::platform::storage::Error>
    where
        S: crate::platform::storage::Storage,
    {
        write_config_file(storage, &StoredAppConfig::from_app_config(self))
    }

    pub fn save_unprovisioned<S>(storage: &mut S) -> Result<(), crate::platform::storage::Error>
    where
        S: crate::platform::storage::Storage,
    {
        storage.write_atomic(APP_CONFIG_KEY, UNPROVISIONED_CONFIG_TEXT)
    }

    pub fn write_effective_config(&self, out: &mut String) {
        let rendered =
            encode_full_config_text_redacted(&StoredAppConfig::from_app_config(self), true);
        if let Ok(text) = core::str::from_utf8(&rendered) {
            out.push_str(text);
        }
    }
}

impl RadioConfig {
    pub fn packet_airtime_ms(self, payload_len: usize) -> u32 {
        let symbol_time_us = self.symbol_time_us();
        let low_data_rate_optimize = symbol_time_us >= 16_000;
        let payload_symbols = payload_symbol_count(
            payload_len,
            self.spreading_factor,
            self.coding_rate_denominator,
            low_data_rate_optimize,
        );
        let preamble_symbols_quarters =
            crate::modules::sx1262::RECEIVE_PREAMBLE_LENGTH as u64 * 4 + 17;
        let preamble_us = div_ceil_u64(symbol_time_us * preamble_symbols_quarters, 4);
        let payload_us = payload_symbols as u64 * symbol_time_us;

        div_ceil_u64(preamble_us.saturating_add(payload_us), 1_000).min(u32::MAX as u64) as u32
    }

    pub fn symbol_time_ms(self) -> u32 {
        div_ceil_u64(self.symbol_time_us(), 1_000).min(u32::MAX as u64) as u32
    }

    fn symbol_time_us(self) -> u64 {
        let chips_per_symbol = 1u64 << self.spreading_factor.min(31);
        div_ceil_u64(
            chips_per_symbol * 1_000_000,
            self.bandwidth_hz.max(1) as u64,
        )
    }

    pub fn validate(self) -> Result<(), ConfigError> {
        if !(150_000_000..=2_500_000_000).contains(&self.receive_frequency_hz) {
            return Err(ConfigError::InvalidFrequency);
        }
        if !(7_000..=500_000).contains(&self.bandwidth_hz) {
            return Err(ConfigError::InvalidBandwidth);
        }
        if !(5..=12).contains(&self.spreading_factor) {
            return Err(ConfigError::InvalidSpreadingFactor);
        }
        if !(5..=8).contains(&self.coding_rate_denominator) {
            return Err(ConfigError::InvalidCodingRate);
        }
        if !(-9..=22).contains(&self.transmit_power_dbm) {
            return Err(ConfigError::InvalidTransmitPower);
        }
        Ok(())
    }

    pub fn receive_config(self) -> crate::modules::sx1262::ReceiveConfig {
        crate::modules::sx1262::ReceiveConfig {
            frequency_hz: self.receive_frequency_hz,
            spreading_factor: spreading_factor(self.spreading_factor),
            bandwidth: bandwidth(self.bandwidth_hz),
            coding_rate: coding_rate(self.coding_rate_denominator),
            preamble_length: crate::modules::sx1262::RECEIVE_PREAMBLE_LENGTH,
            max_payload_len: crate::modules::sx1262::RECEIVE_MAX_PAYLOAD_LEN,
            crc_on: true,
            iq_inverted: false,
            transmit_output_power_dbm: self.transmit_power_dbm,
        }
    }
}

fn payload_symbol_count(
    payload_len: usize,
    spreading_factor: u8,
    coding_rate_denominator: u8,
    low_data_rate_optimize: bool,
) -> u32 {
    let sf = spreading_factor as i32;
    let crc_on = 1;
    let implicit_header = 0;
    let de = i32::from(low_data_rate_optimize);
    let numerator = 8 * payload_len as i32 - 4 * sf + 28 + 16 * crc_on - 20 * implicit_header;
    let denominator = 4 * (sf - 2 * de);
    let coded_symbols = if numerator <= 0 {
        0
    } else {
        ((numerator + denominator - 1) / denominator) as u32 * coding_rate_denominator as u32
    };

    8 + coded_symbols
}

fn div_ceil_u64(numerator: u64, denominator: u64) -> u64 {
    numerator / denominator + u64::from(!numerator.is_multiple_of(denominator))
}

fn generated_node_name(identity_seed: &[u8; 32]) -> String {
    let identity = Identity::from_private_key_seed(identity_seed);
    let mut node_name = String::from("Repeater-");
    for byte in &identity.public_key()[..3] {
        push_hex_byte(&mut node_name, *byte);
    }
    node_name
}

#[derive(Clone)]
struct StoredAppConfig {
    identity_seed: [u8; 32],
    latitude_microdegrees: Option<i32>,
    longitude_microdegrees: Option<i32>,
    node_name: String,
    remote_cli_password: String,
    wifi: WifiConfig,
    radio: RadioConfig,
    regions: RegionMap,
    region_capture: bool,
    flood_max_unscoped_hops: u8,
    flood_max_advert_hops: u8,
    path_hash_mode: u8,
    duty_cycle_percent: u8,
}

impl StoredAppConfig {
    fn default_with_identity_seed(identity_seed: [u8; 32]) -> Self {
        Self {
            identity_seed,
            latitude_microdegrees: None,
            longitude_microdegrees: None,
            node_name: fit_node_name(&generated_node_name(&identity_seed))
                .unwrap_or_else(|_| String::from("Repeater")),
            remote_cli_password: fit_password(identity::REMOTE_CLI_PASSWORD),
            wifi: WifiConfig::default(),
            radio: RadioConfig {
                receive_frequency_hz: 869_618_000,
                spreading_factor: 8,
                bandwidth_hz: 62_500,
                coding_rate_denominator: 6,
                transmit_power_dbm: 14,
            },
            regions: RegionMap::new(),
            region_capture: false,
            flood_max_unscoped_hops: DEFAULT_FLOOD_MAX_UNSCOPED_HOPS,
            flood_max_advert_hops: DEFAULT_FLOOD_MAX_ADVERT_HOPS,
            path_hash_mode: DEFAULT_PATH_HASH_MODE,
            duty_cycle_percent: DEFAULT_DUTY_CYCLE_PERCENT,
        }
    }

    fn from_app_config(config: &AppConfig) -> Self {
        Self {
            identity_seed: config.identity_seed,
            latitude_microdegrees: config.latitude_microdegrees,
            longitude_microdegrees: config.longitude_microdegrees,
            node_name: fit_node_name(config.node_name())
                .unwrap_or_else(|_| String::from("Repeater")),
            remote_cli_password: fit_password(config.remote_cli_password()),
            wifi: config.wifi.clone(),
            radio: config.radio,
            regions: config.regions.clone(),
            region_capture: config.region_capture,
            flood_max_unscoped_hops: config.flood_max_unscoped_hops,
            flood_max_advert_hops: config.flood_max_advert_hops,
            path_hash_mode: config.path_hash_mode,
            duty_cycle_percent: config.duty_cycle_percent,
        }
    }
}

fn load_config_file<S>(
    storage: &mut S,
    defaults: &StoredAppConfig,
) -> Option<(StoredAppConfig, bool)>
where
    S: crate::platform::storage::Storage,
{
    let mut buffer = [0u8; APP_CONFIG_MAX_LEN];
    let len = storage.read(APP_CONFIG_KEY, &mut buffer).ok()?;
    let data = buffer.get(..len)?;

    let config = decode_config_text(data, defaults)?;
    let encoded = encode_config_text(&config);
    Some((config, data != encoded.as_slice()))
}

fn write_config_file<S>(
    storage: &mut S,
    config: &StoredAppConfig,
) -> Result<(), crate::platform::storage::Error>
where
    S: crate::platform::storage::Storage,
{
    let data = encode_config_text(config);
    storage.write_atomic(APP_CONFIG_KEY, &data)
}

fn decode_config_text(data: &[u8], defaults: &StoredAppConfig) -> Option<StoredAppConfig> {
    let text = core::str::from_utf8(data).ok()?;
    let mut config = defaults.clone();
    let mut saw_key = false;
    let mut saw_node_name = false;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (key, value) = line.split_once('=')?;
        let key = key.trim();
        let value = unescape_value(value.trim()).ok()?;

        match key {
            "version" => {
                let _ = value.parse::<u8>().ok()?;
            }
            "identity.seed" => {
                config.identity_seed = parse_identity_seed_hex(&value)?;
            }
            "identity.lat" => {
                config.latitude_microdegrees = parse_optional_coordinate(
                    &value,
                    MIN_LATITUDE_MICRODEGREES,
                    MAX_LATITUDE_MICRODEGREES,
                )?;
            }
            "identity.lon" => {
                config.longitude_microdegrees = parse_optional_coordinate(
                    &value,
                    MIN_LONGITUDE_MICRODEGREES,
                    MAX_LONGITUDE_MICRODEGREES,
                )?;
            }
            "node.name" => {
                config.node_name = fit_node_name(&value).ok()?;
                saw_node_name = true;
            }
            "remote.password" => {
                config.remote_cli_password = fit_password(&value);
            }
            "wifi.ssid" => config.wifi.ssid = value,
            "wifi.pass" => config.wifi.password = value,
            "wifi.telnet" => config.wifi.telnet = parse_bool(&value)?,
            "radio.frequency_hz" => {
                config.radio.receive_frequency_hz = value.parse::<u32>().ok()?;
            }
            "radio.bandwidth_hz" => {
                config.radio.bandwidth_hz = value.parse::<u32>().ok()?;
            }
            "radio.spreading_factor" => {
                config.radio.spreading_factor = value.parse::<u8>().ok()?;
            }
            "radio.coding_rate" | "radio.coding_rate_denominator" => {
                config.radio.coding_rate_denominator = value.parse::<u8>().ok()?;
            }
            "radio.tx_power_dbm" => {
                config.radio.transmit_power_dbm = value.parse::<i32>().ok()?;
            }
            "radio.duty_cycle_percent" => {
                config.duty_cycle_percent = value.parse::<u8>().ok()?;
            }
            "region.default" => {
                config.regions.set_default_from_config(&value).ok()?;
            }
            "region.capture" => {
                config.region_capture = parse_bool(&value)?;
            }
            "flood.max.unscoped" => {
                config.flood_max_unscoped_hops = parse_flood_max_hops(&value)?;
            }
            "flood.max.advert" => {
                config.flood_max_advert_hops = parse_flood_max_hops(&value)?;
            }
            "path.hash.mode" => {
                config.path_hash_mode = parse_path_hash_mode(&value)?;
            }
            key if key.starts_with("region.") => {
                let name = key.strip_prefix("region.")?;
                let allowed = parse_bool(&value)?;
                config.regions.set_region_from_config(name, allowed).ok()?;
            }
            _ => {}
        }
        saw_key = true;
    }

    if !saw_key {
        return None;
    }
    if !saw_node_name {
        config.node_name = fit_node_name(&generated_node_name(&config.identity_seed)).ok()?;
    }
    config.radio.validate().ok()?;
    if !(1..=100).contains(&config.duty_cycle_percent) {
        return None;
    }
    validate_flood_max_hops(config.flood_max_unscoped_hops).ok()?;
    validate_flood_max_hops(config.flood_max_advert_hops).ok()?;
    validate_path_hash_mode(config.path_hash_mode).ok()?;
    validate_wifi_config(&config.wifi).ok()?;
    Some(config)
}

fn validate_wifi_config(wifi: &WifiConfig) -> Result<(), ConfigError> {
    if wifi.ssid.len() > MAX_WIFI_SSID_LEN
        || !(wifi.password.is_empty()
            || (MIN_WIFI_PASSWORD_LEN..=MAX_WIFI_PASSWORD_LEN).contains(&wifi.password.len()))
    {
        return Err(ConfigError::InvalidWifiConfig);
    }
    Ok(())
}

fn encode_config_text(config: &StoredAppConfig) -> Vec<u8> {
    let defaults = StoredAppConfig::default_with_identity_seed(config.identity_seed);
    encode_sparse_config_text(config, &defaults)
}

fn encode_sparse_config_text(config: &StoredAppConfig, defaults: &StoredAppConfig) -> Vec<u8> {
    let mut out = String::new();
    let _ = writeln!(&mut out, "# MCRS app.conf");
    let _ = writeln!(&mut out, "version={}", APP_CONFIG_TEXT_VERSION);

    out.push_str("identity.seed=");
    for byte in config.identity_seed {
        push_hex_byte(&mut out, byte);
    }
    out.push('\n');

    if config.latitude_microdegrees != defaults.latitude_microdegrees {
        out.push_str("identity.lat=");
        write_optional_coordinate(&mut out, config.latitude_microdegrees);
        out.push('\n');
    }

    if config.longitude_microdegrees != defaults.longitude_microdegrees {
        out.push_str("identity.lon=");
        write_optional_coordinate(&mut out, config.longitude_microdegrees);
        out.push('\n');
    }

    if config.node_name != defaults.node_name {
        out.push_str("node.name=");
        write_escaped_value(&mut out, &config.node_name);
        out.push('\n');
    }

    if config.remote_cli_password != defaults.remote_cli_password {
        out.push_str("remote.password=");
        write_escaped_value(&mut out, &config.remote_cli_password);
        out.push('\n');
    }

    if config.wifi.ssid != defaults.wifi.ssid {
        out.push_str("wifi.ssid=");
        write_escaped_value(&mut out, &config.wifi.ssid);
        out.push('\n');
    }
    if config.wifi.password != defaults.wifi.password {
        out.push_str("wifi.pass=");
        write_escaped_value(&mut out, &config.wifi.password);
        out.push('\n');
    }
    if config.wifi.telnet != defaults.wifi.telnet {
        let _ = writeln!(&mut out, "wifi.telnet={}", bool_text(config.wifi.telnet));
    }

    if config.radio.receive_frequency_hz != defaults.radio.receive_frequency_hz {
        let _ = writeln!(
            &mut out,
            "radio.frequency_hz={}",
            config.radio.receive_frequency_hz
        );
    }
    if config.radio.bandwidth_hz != defaults.radio.bandwidth_hz {
        let _ = writeln!(&mut out, "radio.bandwidth_hz={}", config.radio.bandwidth_hz);
    }
    if config.radio.spreading_factor != defaults.radio.spreading_factor {
        let _ = writeln!(
            &mut out,
            "radio.spreading_factor={}",
            config.radio.spreading_factor
        );
    }
    if config.radio.coding_rate_denominator != defaults.radio.coding_rate_denominator {
        let _ = writeln!(
            &mut out,
            "radio.coding_rate={}",
            config.radio.coding_rate_denominator
        );
    }
    if config.radio.transmit_power_dbm != defaults.radio.transmit_power_dbm {
        let _ = writeln!(
            &mut out,
            "radio.tx_power_dbm={}",
            config.radio.transmit_power_dbm
        );
    }
    if config.duty_cycle_percent != defaults.duty_cycle_percent {
        let _ = writeln!(
            &mut out,
            "radio.duty_cycle_percent={}",
            config.duty_cycle_percent
        );
    }
    config
        .regions
        .write_config_lines_changed(&defaults.regions, &mut out);
    if config.region_capture != defaults.region_capture {
        let _ = writeln!(
            &mut out,
            "region.capture={}",
            bool_text(config.region_capture)
        );
    }
    if config.flood_max_unscoped_hops != defaults.flood_max_unscoped_hops {
        let _ = writeln!(
            &mut out,
            "flood.max.unscoped={}",
            config.flood_max_unscoped_hops
        );
    }
    if config.flood_max_advert_hops != defaults.flood_max_advert_hops {
        let _ = writeln!(
            &mut out,
            "flood.max.advert={}",
            config.flood_max_advert_hops
        );
    }
    if config.path_hash_mode != defaults.path_hash_mode {
        let _ = writeln!(&mut out, "path.hash.mode={}", config.path_hash_mode);
    }

    out.into_bytes()
}

fn encode_full_config_text_redacted(config: &StoredAppConfig, redact_secrets: bool) -> Vec<u8> {
    let mut out = String::new();
    let _ = writeln!(&mut out, "# MCRS app.conf");
    let _ = writeln!(&mut out, "version={}", APP_CONFIG_TEXT_VERSION);

    if redact_secrets {
        out.push_str("identity.seed=<redacted>");
    } else {
        out.push_str("identity.seed=");
        for byte in config.identity_seed {
            push_hex_byte(&mut out, byte);
        }
    }
    out.push('\n');

    out.push_str("wifi.ssid=");
    write_escaped_value(&mut out, &config.wifi.ssid);
    out.push('\n');
    if redact_secrets && !config.wifi.password.is_empty() {
        out.push_str("wifi.pass=<redacted>\n");
    } else {
        out.push_str("wifi.pass=");
        write_escaped_value(&mut out, &config.wifi.password);
        out.push('\n');
    }
    let _ = writeln!(&mut out, "wifi.telnet={}", bool_text(config.wifi.telnet));

    out.push_str("identity.lat=");
    write_optional_coordinate(&mut out, config.latitude_microdegrees);
    out.push('\n');

    out.push_str("identity.lon=");
    write_optional_coordinate(&mut out, config.longitude_microdegrees);
    out.push('\n');

    out.push_str("node.name=");
    write_escaped_value(&mut out, &config.node_name);
    out.push('\n');

    if redact_secrets {
        out.push_str("remote.password=<redacted>");
    } else {
        out.push_str("remote.password=");
        write_escaped_value(&mut out, &config.remote_cli_password);
    }
    out.push('\n');

    let _ = writeln!(
        &mut out,
        "radio.frequency_hz={}",
        config.radio.receive_frequency_hz
    );
    let _ = writeln!(&mut out, "radio.bandwidth_hz={}", config.radio.bandwidth_hz);
    let _ = writeln!(
        &mut out,
        "radio.spreading_factor={}",
        config.radio.spreading_factor
    );
    let _ = writeln!(
        &mut out,
        "radio.coding_rate={}",
        config.radio.coding_rate_denominator
    );
    let _ = writeln!(
        &mut out,
        "radio.tx_power_dbm={}",
        config.radio.transmit_power_dbm
    );
    let _ = writeln!(
        &mut out,
        "radio.duty_cycle_percent={}",
        config.duty_cycle_percent
    );
    config.regions.write_config_lines(&mut out);
    let _ = writeln!(
        &mut out,
        "region.capture={}",
        bool_text(config.region_capture)
    );
    let _ = writeln!(
        &mut out,
        "flood.max.unscoped={}",
        config.flood_max_unscoped_hops
    );
    let _ = writeln!(
        &mut out,
        "flood.max.advert={}",
        config.flood_max_advert_hops
    );
    let _ = writeln!(&mut out, "path.hash.mode={}", config.path_hash_mode);

    out.into_bytes()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    InvalidName,
    InvalidLatitude,
    InvalidLongitude,
    InvalidFrequency,
    InvalidBandwidth,
    InvalidSpreadingFactor,
    InvalidCodingRate,
    InvalidTransmitPower,
    InvalidDutyCycle,
    InvalidFloodMaxHops,
    InvalidPathHashMode,
    InvalidWifiConfig,
    Region(RegionError),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::InvalidName => f.write_str("invalid name"),
            ConfigError::InvalidLatitude => f.write_str("invalid latitude"),
            ConfigError::InvalidLongitude => f.write_str("invalid longitude"),
            ConfigError::InvalidFrequency => f.write_str("invalid frequency"),
            ConfigError::InvalidBandwidth => f.write_str("invalid bandwidth"),
            ConfigError::InvalidSpreadingFactor => f.write_str("invalid spreading factor"),
            ConfigError::InvalidCodingRate => f.write_str("invalid coding rate"),
            ConfigError::InvalidTransmitPower => f.write_str("invalid transmit power"),
            ConfigError::InvalidDutyCycle => f.write_str("invalid duty cycle"),
            ConfigError::InvalidFloodMaxHops => f.write_str("invalid flood max hops"),
            ConfigError::InvalidPathHashMode => f.write_str("invalid path hash mode"),
            ConfigError::InvalidWifiConfig => f.write_str("invalid Wi-Fi setting"),
            ConfigError::Region(error) => write!(f, "region: {}", error),
        }
    }
}

impl From<RegionError> for ConfigError {
    fn from(error: RegionError) -> Self {
        Self::Region(error)
    }
}

fn fit_node_name(input: &str) -> Result<String, ConfigError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ConfigError::InvalidName);
    }

    let mut output = String::new();
    for character in input.chars() {
        if matches!(character, '[' | ']' | '\\' | ':' | ',' | '?' | '*') {
            return Err(ConfigError::InvalidName);
        }
        if output.len() + character.len_utf8() > MAX_NODE_NAME_LEN {
            break;
        }
        output.push(character);
    }

    if output.is_empty() {
        Err(ConfigError::InvalidName)
    } else {
        Ok(output)
    }
}

fn fit_password(input: &str) -> String {
    let mut output = String::new();

    for character in input.chars() {
        if output.len() + character.len_utf8() > MAX_REMOTE_PASSWORD_LEN {
            break;
        }
        output.push(character);
    }

    if output.is_empty() {
        output.push_str("meshcore");
    }

    output
}

fn write_escaped_value(out: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(character),
        }
    }
}

fn unescape_value(input: &str) -> Result<String, ()> {
    let mut output = String::new();
    let mut escaped = false;

    for character in input.chars() {
        if escaped {
            match character {
                '\\' => output.push('\\'),
                'n' => output.push('\n'),
                'r' => output.push('\r'),
                _ => return Err(()),
            }
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            output.push(character);
        }
    }

    if escaped { Err(()) } else { Ok(output) }
}

fn parse_identity_seed_hex(input: &str) -> Option<[u8; 32]> {
    let bytes = input.as_bytes();
    if bytes.len() != 64 {
        return None;
    }

    let mut seed = [0u8; 32];
    for index in 0..seed.len() {
        let high = hex_nibble(bytes[index * 2])?;
        let low = hex_nibble(bytes[index * 2 + 1])?;
        seed[index] = high << 4 | low;
    }
    Some(seed)
}

fn push_hex_byte(out: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(HEX[(byte >> 4) as usize] as char);
    out.push(HEX[(byte & 0x0f) as usize] as char);
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_bool(input: &str) -> Option<bool> {
    match input.trim() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

pub fn parse_flood_max_hops(input: &str) -> Option<u8> {
    let hops = input.trim().parse::<u8>().ok()?;
    validate_flood_max_hops(hops).ok()?;
    Some(hops)
}

pub fn parse_path_hash_mode(input: &str) -> Option<u8> {
    let mode = input.trim().parse::<u8>().ok()?;
    validate_path_hash_mode(mode).ok()?;
    Some(mode)
}

fn bool_text(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

pub fn parse_latitude_microdegrees(input: &str) -> Option<Option<i32>> {
    parse_optional_coordinate(input, MIN_LATITUDE_MICRODEGREES, MAX_LATITUDE_MICRODEGREES)
}

pub fn parse_longitude_microdegrees(input: &str) -> Option<Option<i32>> {
    parse_optional_coordinate(
        input,
        MIN_LONGITUDE_MICRODEGREES,
        MAX_LONGITUDE_MICRODEGREES,
    )
}

fn parse_optional_coordinate(input: &str, min: i32, max: i32) -> Option<Option<i32>> {
    let input = input.trim();
    if input.is_empty() || matches!(input, "none" | "null" | "<null>") {
        return Some(None);
    }

    let value = parse_coordinate_microdegrees(input)?;
    if (min..=max).contains(&value) {
        Some(Some(value))
    } else {
        None
    }
}

fn parse_coordinate_microdegrees(input: &str) -> Option<i32> {
    let (negative, input) = match input.as_bytes().first().copied() {
        Some(b'-') => (true, &input[1..]),
        Some(b'+') => (false, &input[1..]),
        _ => (false, input),
    };
    if input.is_empty() {
        return None;
    }

    let mut whole = 0i64;
    let mut fraction = 0i64;
    let mut fraction_digits = 0usize;
    let mut seen_dot = false;
    let mut saw_digit = false;

    for byte in input.bytes() {
        match byte {
            b'0'..=b'9' if !seen_dot => {
                saw_digit = true;
                whole = whole.checked_mul(10)?.checked_add(i64::from(byte - b'0'))?;
            }
            b'0'..=b'9' => {
                saw_digit = true;
                if fraction_digits < 6 {
                    fraction = fraction
                        .checked_mul(10)?
                        .checked_add(i64::from(byte - b'0'))?;
                    fraction_digits += 1;
                }
            }
            b'.' if !seen_dot => {
                seen_dot = true;
            }
            _ => return None,
        }
    }
    if !saw_digit {
        return None;
    }

    while fraction_digits < 6 {
        fraction = fraction.checked_mul(10)?;
        fraction_digits += 1;
    }

    let value = whole
        .checked_mul(i64::from(COORDINATE_SCALE))?
        .checked_add(fraction)?;
    let value = if negative { -value } else { value };
    i32::try_from(value).ok()
}

fn validate_latitude(latitude_microdegrees: i32) -> Result<(), ConfigError> {
    if (MIN_LATITUDE_MICRODEGREES..=MAX_LATITUDE_MICRODEGREES).contains(&latitude_microdegrees) {
        Ok(())
    } else {
        Err(ConfigError::InvalidLatitude)
    }
}

fn validate_longitude(longitude_microdegrees: i32) -> Result<(), ConfigError> {
    if (MIN_LONGITUDE_MICRODEGREES..=MAX_LONGITUDE_MICRODEGREES).contains(&longitude_microdegrees) {
        Ok(())
    } else {
        Err(ConfigError::InvalidLongitude)
    }
}

fn validate_flood_max_hops(hops: u8) -> Result<(), ConfigError> {
    if hops <= 63 {
        Ok(())
    } else {
        Err(ConfigError::InvalidFloodMaxHops)
    }
}

fn validate_path_hash_mode(mode: u8) -> Result<(), ConfigError> {
    if mode <= 2 {
        Ok(())
    } else {
        Err(ConfigError::InvalidPathHashMode)
    }
}

fn write_optional_coordinate(out: &mut String, coordinate_microdegrees: Option<i32>) {
    let Some(coordinate) = coordinate_microdegrees else {
        return;
    };

    let coordinate = i64::from(coordinate);
    if coordinate < 0 {
        out.push('-');
    }
    let absolute = coordinate.abs();
    let whole = absolute / i64::from(COORDINATE_SCALE);
    let fraction = absolute % i64::from(COORDINATE_SCALE);
    let _ = write!(out, "{}.{:06}", whole, fraction);
}

fn spreading_factor(value: u8) -> lora_phy::mod_params::SpreadingFactor {
    match value {
        5 => lora_phy::mod_params::SpreadingFactor::_5,
        6 => lora_phy::mod_params::SpreadingFactor::_6,
        7 => lora_phy::mod_params::SpreadingFactor::_7,
        9 => lora_phy::mod_params::SpreadingFactor::_9,
        10 => lora_phy::mod_params::SpreadingFactor::_10,
        11 => lora_phy::mod_params::SpreadingFactor::_11,
        12 => lora_phy::mod_params::SpreadingFactor::_12,
        _ => lora_phy::mod_params::SpreadingFactor::_8,
    }
}

fn bandwidth(value: u32) -> lora_phy::mod_params::Bandwidth {
    match value {
        7_810 => lora_phy::mod_params::Bandwidth::_7KHz,
        10_420 => lora_phy::mod_params::Bandwidth::_10KHz,
        15_630 => lora_phy::mod_params::Bandwidth::_15KHz,
        20_830 => lora_phy::mod_params::Bandwidth::_20KHz,
        31_250 => lora_phy::mod_params::Bandwidth::_31KHz,
        41_670 => lora_phy::mod_params::Bandwidth::_41KHz,
        125_000 => lora_phy::mod_params::Bandwidth::_125KHz,
        250_000 => lora_phy::mod_params::Bandwidth::_250KHz,
        500_000 => lora_phy::mod_params::Bandwidth::_500KHz,
        _ => lora_phy::mod_params::Bandwidth::_62KHz,
    }
}

fn coding_rate(value: u8) -> lora_phy::mod_params::CodingRate {
    match value {
        5 => lora_phy::mod_params::CodingRate::_4_5,
        6 => lora_phy::mod_params::CodingRate::_4_6,
        7 => lora_phy::mod_params::CodingRate::_4_7,
        _ => lora_phy::mod_params::CodingRate::_4_8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> StoredAppConfig {
        StoredAppConfig::default_with_identity_seed([7; 32])
    }

    #[test]
    fn wifi_config_round_trips_escaped_values() {
        let mut config = defaults();
        config.wifi.ssid = String::from("lab\\network\nwest");
        config.wifi.password = String::from("password\\with\\slashes");
        let encoded = encode_config_text(&config);
        let decoded = decode_config_text(&encoded, &defaults()).expect("valid config");
        assert_eq!(decoded.wifi, config.wifi);
    }

    #[test]
    fn wifi_password_is_redacted_from_effective_config() {
        let mut config = defaults();
        config.wifi.ssid = String::from("lab");
        config.wifi.password = String::from("supersecret");
        let rendered = encode_full_config_text_redacted(&config, true);
        let rendered = core::str::from_utf8(&rendered).expect("UTF-8 config");
        assert!(rendered.contains("wifi.pass=<redacted>"));
        assert!(!rendered.contains("supersecret"));
    }

    #[test]
    fn empty_wifi_password_round_trips() {
        let mut config = defaults();
        config.wifi.ssid = String::from("open-network");
        let encoded = encode_config_text(&config);
        let decoded = decode_config_text(&encoded, &defaults()).expect("valid config");
        assert_eq!(decoded.wifi.password(), "");
    }

    #[test]
    fn wifi_telnet_round_trips() {
        let mut config = defaults();
        config.wifi.telnet = true;
        let encoded = encode_config_text(&config);
        let decoded = decode_config_text(&encoded, &defaults()).expect("valid config");
        assert!(decoded.wifi.telnet());
    }

    #[test]
    fn wifi_lengths_are_validated() {
        let mut config = AppConfig::generated_defaults([9; 32]);
        assert!(config.set_wifi_ssid(&"x".repeat(MAX_WIFI_SSID_LEN)).is_ok());
        assert!(
            config
                .set_wifi_ssid(&"x".repeat(MAX_WIFI_SSID_LEN + 1))
                .is_err()
        );
        assert!(config.set_wifi_password("").is_ok());
        assert!(config.set_wifi_password("12345678").is_ok());
        assert!(config.set_wifi_password("short").is_err());
        assert!(
            config
                .set_wifi_password(&"x".repeat(MAX_WIFI_PASSWORD_LEN + 1))
                .is_err()
        );
    }
}
