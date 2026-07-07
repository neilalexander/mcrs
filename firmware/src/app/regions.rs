extern crate alloc;

use alloc::{string::String, vec::Vec};
use core::fmt::Write;

use hmac::{Hmac, Mac};
use mcrs_protocol::{Packet, RouteType, TransportCodes};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

const MAX_REGIONS: usize = 32;
const MAX_REGION_NAME_LEN: usize = 30;

pub const REGION_DENY_FLOOD: u8 = 0x01;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionEntry {
    pub id: u16,
    pub flags: u8,
    pub name: String,
}

impl RegionEntry {
    pub fn is_wildcard(&self) -> bool {
        self.id == 0
    }

    pub fn allows_flood(&self) -> bool {
        self.flags & REGION_DENY_FLOOD == 0
    }

    pub fn display_name(&self) -> &str {
        display_name(&self.name)
    }
}

#[derive(Clone)]
pub struct RegionMap {
    next_id: u16,
    default_id: Option<u16>,
    wildcard_flags: u8,
    entries: Vec<RegionEntry>,
}

impl RegionMap {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            default_id: None,
            wildcard_flags: 0,
            entries: Vec::new(),
        }
    }

    pub fn put_region(&mut self, name: &str) -> Result<(), RegionError> {
        let name = normalize_region_name(name)?;
        if name == "*" {
            return Err(RegionError::InvalidName);
        }

        if let Some(index) = self.entry_index_by_name(&name) {
            self.entries[index].flags = 0;
            return Ok(());
        }

        if self.entries.len() >= MAX_REGIONS {
            return Err(RegionError::Full);
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1).max(id.saturating_add(1));
        self.entries.push(RegionEntry { id, flags: 0, name });
        Ok(())
    }

    pub fn remove_region(&mut self, name: &str) -> Result<(), RegionError> {
        let Some(index) = self.entry_index_by_name(name) else {
            return Err(RegionError::NotFound);
        };
        let id = self.entries[index].id;
        self.entries.remove(index);
        if self.default_id == Some(id) {
            self.default_id = None;
        }
        Ok(())
    }

    pub fn set_region_from_config(&mut self, name: &str, allowed: bool) -> Result<(), RegionError> {
        if name == "default" {
            return Err(RegionError::InvalidName);
        }
        if name == "*" {
            self.set_flood_allowed(name, allowed)?;
            return Ok(());
        }
        self.put_region(name)?;
        self.set_flood_allowed(name, allowed)
    }

    pub fn set_flood_allowed(&mut self, name: &str, allowed: bool) -> Result<(), RegionError> {
        if name == "*" {
            set_flag(&mut self.wildcard_flags, REGION_DENY_FLOOD, !allowed);
            return Ok(());
        }

        let Some(index) = self.entry_index_by_name(name) else {
            return Err(RegionError::NotFound);
        };
        set_flag(&mut self.entries[index].flags, REGION_DENY_FLOOD, !allowed);
        Ok(())
    }

    pub fn set_default(&mut self, name: Option<&str>) -> Result<(), RegionError> {
        let Some(name) = name else {
            self.default_id = None;
            return Ok(());
        };
        let Some(region) = self.find_by_name_prefix(name) else {
            return Err(RegionError::NotFound);
        };
        if region.is_wildcard() {
            self.default_id = None;
            return Ok(());
        }
        let id = region.id;
        self.set_flood_allowed(name, true)?;
        self.default_id = Some(id);
        Ok(())
    }

    pub fn find_by_name_prefix(&self, prefix: &str) -> Option<RegionEntry> {
        if prefix == "*" {
            return Some(self.wildcard());
        }

        let prefix = strip_hash(prefix);
        let mut partial = None;
        for entry in &self.entries {
            let name = strip_hash(&entry.name);
            if name == prefix {
                return Some(entry.clone());
            }
            if name.starts_with(prefix) {
                partial = Some(entry.clone());
            }
        }
        partial
    }

    pub fn default_region(&self) -> Option<RegionEntry> {
        self.default_id.and_then(|id| self.find_by_id(id).cloned())
    }

    pub fn match_flood_region(&self, packet: &Packet) -> Option<RegionEntry> {
        match packet.route_type {
            RouteType::TransportFlood => self.match_transport_region(packet, REGION_DENY_FLOOD),
            RouteType::Flood if self.wildcard().allows_flood() => Some(self.wildcard()),
            RouteType::Flood => None,
            _ => None,
        }
    }

    pub fn apply_default_scope(&self, packet: &mut Packet) -> Result<bool, RegionError> {
        let Some(region) = self.default_region() else {
            return Ok(false);
        };
        let code = self.transport_code_for(&region, packet)?;
        packet.route_type = RouteType::TransportFlood;
        packet.transport_codes = Some(TransportCodes::new(code));
        Ok(true)
    }

    pub fn write_tree(&self, out: &mut String) {
        let wildcard = self.wildcard();
        self.write_region_line(&wildcard, out);
        for region in &self.entries {
            self.write_region_line(region, out);
        }
    }

    pub fn allowed_names(&self) -> String {
        self.names_matching(REGION_DENY_FLOOD, false)
    }

    pub fn denied_names(&self) -> String {
        self.names_matching(REGION_DENY_FLOOD, true)
    }

    fn match_transport_region(&self, packet: &Packet, deny_mask: u8) -> Option<RegionEntry> {
        let codes = packet.transport_codes?;
        for entry in &self.entries {
            if entry.flags & deny_mask != 0 {
                continue;
            }
            if self.transport_code_for(entry, packet).ok()? == codes.primary {
                return Some(entry.clone());
            }
        }
        None
    }

    fn transport_code_for(
        &self,
        region: &RegionEntry,
        packet: &Packet,
    ) -> Result<u16, RegionError> {
        let key = transport_key_for(region).ok_or(RegionError::PrivateRegionUnsupported)?;
        let payload = packet
            .payload
            .encode()
            .map_err(|_| RegionError::PacketEncode)?;
        let mut mac = HmacSha256::new_from_slice(&key).map_err(|_| RegionError::PacketEncode)?;
        mac.update(&[packet.payload.kind().to_nibble()]);
        mac.update(&payload);
        let digest = mac.finalize().into_bytes();
        let mut code = u16::from_le_bytes([digest[0], digest[1]]);
        if code == 0 {
            code = 1;
        } else if code == u16::MAX {
            code = u16::MAX - 1;
        }
        Ok(code)
    }

    fn names_matching(&self, mask: u8, invert: bool) -> String {
        let mut out = String::new();
        let wildcard_matches = if invert {
            self.wildcard_flags & mask != 0
        } else {
            self.wildcard_flags & mask == 0
        };
        if wildcard_matches {
            out.push('*');
        }

        for entry in &self.entries {
            let matches = if invert {
                entry.flags & mask != 0
            } else {
                entry.flags & mask == 0
            };
            if !matches {
                continue;
            }
            if !out.is_empty() {
                out.push(',');
            }
            out.push_str(entry.display_name());
        }
        out
    }

    pub fn write_config_lines(&self, out: &mut String) {
        if let Some(default) = self.default_region() {
            let _ = writeln!(out, "region.default={}", default.display_name());
        }
        let _ = writeln!(
            out,
            "region.*={}",
            bool_text(self.wildcard().allows_flood())
        );
        for region in &self.entries {
            if region.display_name() == "default" {
                continue;
            }
            let _ = writeln!(
                out,
                "region.{}={}",
                region.display_name(),
                bool_text(region.allows_flood())
            );
        }
    }

    pub fn write_config_lines_changed(&self, defaults: &Self, out: &mut String) {
        if self.default_region_name() != defaults.default_region_name() {
            match self.default_region() {
                Some(default) => {
                    let _ = writeln!(out, "region.default={}", default.display_name());
                }
                None => {
                    let _ = writeln!(out, "region.default=<null>");
                }
            }
        }

        if self.wildcard().allows_flood() != defaults.wildcard().allows_flood() {
            let _ = writeln!(
                out,
                "region.*={}",
                bool_text(self.wildcard().allows_flood())
            );
        }

        for region in &self.entries {
            if region.display_name() == "default" {
                continue;
            }
            let _ = writeln!(
                out,
                "region.{}={}",
                region.display_name(),
                bool_text(region.allows_flood())
            );
        }
    }

    fn write_region_line(&self, region: &RegionEntry, out: &mut String) {
        out.push_str(region.display_name());
        if region.allows_flood() {
            out.push_str(" F");
        }
        out.push('\n');
    }

    fn wildcard(&self) -> RegionEntry {
        RegionEntry {
            id: 0,
            flags: self.wildcard_flags,
            name: String::from("*"),
        }
    }

    fn find_by_id(&self, id: u16) -> Option<&RegionEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    fn default_region_name(&self) -> Option<String> {
        self.default_region()
            .map(|region| String::from(region.display_name()))
    }

    fn entry_index_by_name(&self, name: &str) -> Option<usize> {
        if name == "*" {
            return None;
        }
        let name = strip_hash(name);
        self.entries
            .iter()
            .position(|entry| strip_hash(&entry.name) == name)
    }

    pub fn set_default_from_config(&mut self, name: &str) -> Result<(), RegionError> {
        if name == "<null>" || name.is_empty() {
            return self.set_default(None);
        }
        if self.find_by_name_prefix(name).is_none() {
            self.put_region(name)?;
        }
        self.set_default(Some(name))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionError {
    EmptyName,
    InvalidName,
    NameTooLong,
    Full,
    NotFound,
    PacketEncode,
    PrivateRegionUnsupported,
}

impl core::fmt::Display for RegionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyName => f.write_str("empty region name"),
            Self::InvalidName => f.write_str("invalid region name"),
            Self::NameTooLong => f.write_str("region name too long"),
            Self::Full => f.write_str("region table full"),
            Self::NotFound => f.write_str("unknown region"),
            Self::PacketEncode => f.write_str("packet encode failed"),
            Self::PrivateRegionUnsupported => f.write_str("private region keys are unsupported"),
        }
    }
}

fn transport_key_for(region: &RegionEntry) -> Option<[u8; 16]> {
    if region.name.starts_with('$') {
        return None;
    }

    let mut hasher = Sha256::new();
    if region.name.starts_with('#') {
        hasher.update(region.name.as_bytes());
    } else {
        hasher.update(b"#");
        hasher.update(region.name.as_bytes());
    }
    let digest = hasher.finalize();
    let mut key = [0; 16];
    key.copy_from_slice(&digest[..16]);
    Some(key)
}

fn normalize_region_name(name: &str) -> Result<String, RegionError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(RegionError::EmptyName);
    }
    if name.len() > MAX_REGION_NAME_LEN {
        return Err(RegionError::NameTooLong);
    }
    if name == "*" {
        return Ok(String::from("*"));
    }
    if !name.bytes().all(is_name_char) {
        return Err(RegionError::InvalidName);
    }
    Ok(String::from(name))
}

fn is_name_char(byte: u8) -> bool {
    byte == b'-' || byte == b'$' || byte == b'#' || byte.is_ascii_digit() || byte >= b'A'
}

fn strip_hash(name: &str) -> &str {
    name.strip_prefix('#').unwrap_or(name)
}

fn display_name(name: &str) -> &str {
    strip_hash(name)
}

fn set_flag(flags: &mut u8, flag: u8, enabled: bool) {
    if enabled {
        *flags |= flag;
    } else {
        *flags &= !flag;
    }
}

fn bool_text(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}
