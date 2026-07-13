# MCRS

MCRS is an alternative and somewhat opinionated MeshCore repeater firmware written in
Rust using Embassy. It is designed to be fully protocol-compatible with the existing
MeshCore network and devices, but with several notable improvements over the official
firmware:

* **Responsive:** The asynchronous task design handles radio, hardware and network
  activity without polling and without allowing one interface to hold up other interfaces
  or unrelated tasks. With no busy main loop, the CPU can idle between events, reducing
  power use and extending battery life.
* **Hardened:** Extensive error handling and validation, particularly in the packet decoder.
  Malformed, truncated and deliberately crafted malicious packets are caught and dropped
  before they can affect repeater state. The packet decoder has been fuzzed extensively
  to avoid unexpected crashes or side effects.
* **Considerate:** Channel Activity Detection (CAD) support for avoiding transmitting
  when the channel is busy, with bounded random backoffs and queued sends to smooth out
  contention. Accurate airtime calculations feed a rolling budget that enforces the
  correct duty cycle when configured.
* **Predictable:** Buffers, queues and internal state have explicit capacities and
  every access is bounds-checked. Rust is memory-safe by default and prevents situations
  where one memory region can overflow into another, avoiding silent memory corruption.

MCRS supports the following features:

* Remote management, including CLI access:
  * Some features, such as owner information, are not yet supported.
* Remote telemetry, including the neighbour list:
  * Sensors are not yet supported.
* Regions:
  * Allow or deny regions and setting the default advert region works using standard `region put`, `region allowf`, `region denyf`, `region save` CLI commands.
  * Supports `region default` for scoping adverts to a default region.
  * Supports `region capture` for redirecting unscoped flood traffic that arrives directly to the repeater into the default region scope before repeating.
* Wi-Fi connectivity:
  * OTA firmware updates with A/B partitions in either AP or STA mode.
  * Automatic NTP clock sync in STA mode every hour, avoiding the need for manual clock sync.
  * Telnet CLI access with `wifi.telnet = true` in STA mode, for easier remote management on trusted networks.
* Hardware shortcut for sending zero-hop adverts by pressing-and-holding the `PRG`/`USER` button.

Note: The storage format is different to the official MeshCore firmware, so a repeater that is switched to this firmware will come up with a fresh configuration and will need to be reconfigured. The default admin password is `meshcore`. 

Note: MCRS currently defaults to the UK frequency 869.618MHz at 62.5KHz BW, SF8, CR6. Overriding this at build-time is still to be done.

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

Useful build commands for e.g. the Heltec v3:

```sh
cargo test -p mcrs-protocol
cargo +esp check-heltec-v3
make heltec-v3-build
make heltec-v3-flash
make heltec-v3-bins
```

For Heltec v4 and WSL3, use the corresponding `heltec-v4-*` and `heltec-wsl3-*` Make targets.

### Configuration

Remote management can be used to configure various settings as normal. Additionally,
the CLI is accessible over remote management and the USB serial console (where available),
supporting many of the same commands as the official firmware.

Wi-Fi can be configured persistently from the serial CLI:

```text
set wifi.ssid MyWiFi
set wifi.pass my-passphrase
set wifi.telnet true/false
reboot
```

When configured, the firmware joins that network in station mode and `ota start`
serves the OTA page on port 80 of its assigned address. If `wifi.ssid` is empty,
`ota start` instead creates the original open OTA access point at
`192.168.4.1/24`.

Station mode also synchronizes the retained wall clock from `pool.ntp.org`
after DHCP completes and refreshes it periodically.

When `wifi.telnet` is enabled, station mode exposes the privileged CLI on TCP
port 23. This interface has the same authority as the serial CLI and has no
additional authentication, so only enable it on a trusted network.

### Fuzzing

Fuzzing uses `cargo-fuzz`, which requires nightly Rust:

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
cargo +nightly fuzz run protocol_packet
cargo +nightly fuzz run protocol_payloads
```

### Opinions

The firmware has some opinionated defaults when compared to the official repeater firmware:

* Flood adverts are not sent automatically and cannot be configured to do so. Flooded adverts
  are extremely wasteful and take up a lot of airtime across long distances, so should be 
  avoided wherever possible. 
* Zero-hop adverts are sent every 4 hours to populate neighbour lists, this is not currently
  configurable. It is useful to know about repeaters nearby and to be able to see neighbours
  in repeater telemetry, but we avoid sending information about them too often.
* The default flood max for flood adverts, both scoped and unscoped, is 3 hops. This is configurable
  but the default should prevent the spread of flood adverts from going too far.
* The default flood max for unscoped traffic is 5 hops.
* There is no default flood max for scoped traffic. We want to encourage regions being used to
  sufficiently contain flood traffic, as this will be essential to avoid mesh scaling issues.
* Guest telemetry access is always allowed without a password. Everyone likes being able to see
  which repeaters can hear which other repeaters, so that anyone can help to improve coverage
  when needed.

### Licence

MIT. Copyright © 2026 Neil Alexander.
