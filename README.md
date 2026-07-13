# MCRS

MCRS is an alternative Meshcore repeater firmware written in Rust using Embassy.

Supported features:

* Remote management, including a subset of the CLI
  * Owner information is not yet supported
* Remote telemetry, including the neighbour list
  * Sensors are not yet supported
* Regions
  * Allow or deny regions and setting the default advert region works
  * Optional `region capture` allows unscoped flood traffic that arrives directly to the repeater to be forwarded into the default scope instead of unscoped
* Wi-Fi connectivity
  * OTA firmware updates with A/B partitions in either AP or STA mode
  * Automatic NTP clock sync in STA mode
  * Telnet CLI access with `wifi.telnet = true` in STA mode
* Press-and-hold `PRG` to send a zero-hop advert

Note: the storage configuration format is different to the original firmware, so a repeater that is switched to this firmware will need to be reconfigured. The default admin password is `meshcore`.

Note: currently defaults to the UK frequency 869.618MHz at 62.5KHz BW, SF8, CR6. Overriding this at build-time is still to be done.

Codex helped to write some, but not all, of this code. Special thanks to the https://coreprotocol.org team as their documentation made this possible. Fuzzing code is provided for the packet decoder.

### Hardware

* Heltec v3: Tested, working, with OLED and battery level reporting.
* Heltec v4: Tested, working, with OLED and battery level reporting on v4.3. Other revisions untested.
* Heltec WSL3: Untested.

### Building

For Heltec, install the ESP Rust toolchain:

```sh
cargo install espup --locked
espup install --targets esp32s3
. "$HOME/export-esp.sh"
```

Source `$HOME/export-esp.sh` in each new shell before using `cargo +esp ...`, or add it to your shell profile.

Install the flashing/image tool:

```sh
cargo install espflash --locked
```

Useful build commands:

```sh
cargo test -p mcrs-protocol
cargo +esp check-heltec-v3
make heltec-v3-build
make heltec-v3-flash
make heltec-v3-bins
```

Wi-Fi is configured persistently from the serial CLI:

```text
set wifi.ssid MyWiFi
set wifi.pass my-passphrase
set wifi.telnet true/false
reboot
```

Use `unset NAME` to restore a setting to its firmware default and remove its
override from the next config write, for example `unset wifi.telnet`.

When configured, the firmware joins that network in station mode and `ota start`
serves the OTA page on port 80 of its assigned address. If `wifi.ssid` is empty,
`ota start` instead creates the original open OTA access point at
`192.168.4.1/24`.

Station mode also synchronizes the retained wall clock from `pool.ntp.org`
after DHCP completes and refreshes it periodically.

When `wifi.telnet` is enabled, station mode exposes the privileged CLI on TCP
port 23. This interface has the same authority as the serial CLI and has no
additional authentication, so only enable it on a trusted network.

For Heltec v4 and WSL3, use the corresponding `heltec-v4-*` and `heltec-wsl3-*` Make targets.

Fuzzing uses `cargo-fuzz`, which requires nightly Rust:

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
cargo +nightly fuzz run protocol_packet
cargo +nightly fuzz run protocol_payloads
```

### Opinions

The firmware has some opinionated defaults when compared to the official repeater firmware:

* Flood adverts are not sent automatically and cannot be configured to do so
* Zero-hop adverts are sent every 4 hours to populate neighbour lists and is not configurable
* The default flood max for flood adverts, both scoped and unscoped, is 3 hops
* The default flood max for unscoped traffic is 5 hops
* There is no default flood max for scoped traffic
* Guest telemetry access is always allowed without a password

### Warranty

None whatsoever! Do what you want with it, don't yell at me if it doesn't work.

### Licence

MIT. Copyright © 2026 Neil Alexander.
