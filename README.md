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

* Remote management:
  * Remote configuration and CLI access via remote repeater management.
  * CLI access also available via serial port and, optionally, via telnet over Wi-Fi.
* Remote telemetry, including the neighbour list:
  * Sensors are not yet supported.
* Regions:
  * Allow or deny regions and setting the default advert region works using standard `region put`, `region allowf`, `region denyf`, `region save` CLI commands.
  * Supports `region default` for scoping adverts to a default region.
  * Supports `region capture` for redirecting unscoped flood traffic that arrives directly to the repeater into the default region scope before repeating.
* Loop detection:
  * Four modes: `minimal`, `moderate`, `strict` and `off`.
* Wi-Fi connectivity:
  * OTA firmware updates with A/B partitions in either AP or STA mode.
  * Automatic NTP clock sync in STA mode every hour, avoiding the need for manual clock sync.
  * Telnet CLI access with `wifi.telnet = true` in STA mode, for easier remote management on trusted networks.
* Hardware shortcut for sending zero-hop adverts by pressing-and-holding the `PRG`/`USER` button.
* Optional MQTT packet reporting to up to three brokers over Wi-Fi. This is only
  present in firmware compiled with the Cargo feature `mqtt`.

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
source "$HOME/export-esp.sh"
```

Source `$HOME/export-esp.sh` in each new shell before using `cargo +esp ...`, or add it to your shell profile.

Install the flashing/image tool:

```sh
cargo install espflash --locked
```

Useful build commands for e.g. the Heltec v3:

```sh
cargo test -p mcrs-protocol
cargo test -p mcrs-firmware --lib --features mqtt
cargo +esp check-heltec-v3
cargo +esp check-heltec-v3-mqtt
make heltec-v3-build
make heltec-v3-flash
make heltec-v3-bins
```

For Heltec v4 and WSL3, use the corresponding `heltec-v4-*` and `heltec-wsl3-*` Make targets.

The `*-bins` targets and GitHub Actions workflow produce two images:

- `*-upgrade.bin`: application-only image to upload through the OTA update page.
- `*-full.bin`: bootloader, partition table and application for initial USB/serial
  installation at flash address `0x0`.

Use `make heltec-v3-upgrade-bin` or `make heltec-v3-full-bin` to generate just one
image type. Both are written to `dist/`.

The standard `make *-build`, `make *-flash`, and `make *-bins` targets build
firmware without MQTT. Set `MQTT=1` to include it in any board build, flash, or
binary target; for example:

```sh
make MQTT=1 heltec-v3-build
make MQTT=1 heltec-v3-flash
make MQTT=1 heltec-v3-bins
```

### Importing an existing identity

To keep an existing MeshCore identity, copy the output of its local `get prv.key`
command and use `set prv.key <hex>` on MCRS, then reboot. The command accepts
both 64-character seeds and 128-character MeshCore expanded private keys.
It reports the new public key so you can compare it before rebooting.

Setting a key replaces the previous stored key. `get prv.key` returns the stored
format; configuration files use `identity.seed` or `identity.expanded`, respectively.
The running identity changes on reboot.

### Configuration

Standard MeshCore remote management can be used to configure various settings as normal.
The default remote management password is `meshcore` and can be changed in the normal
way.

Additionally, the CLI can be accessed over the USB serial console (where available), as
well as optionally via Telnet over Wi-Fi. Many of the remote management commands are
the same as the official firmware.

Many configuration options can be returned to their default value using `unset`
instead of `set`.

Wi-Fi can be configured from the serial CLI:

```text
set wifi.ssid MyWiFi
set wifi.pass my-passphrase
set wifi.telnet true/false
reboot
```

When configured, the firmware joins that Wi-Fi network. When connected to Wi-Fi,
clock sync using the `pool.ntp.org` NTP pool enabled automatically.

When `wifi.telnet` is enabled, station mode exposes the privileged CLI on TCP
port 23. This interface has the same authority as the serial CLI and has no
additional authentication, so only enable it on a trusted network.

### OTA updates

OTA updates can be performed over Wi-Fi on supported boards with the `ota start`
command on the management CLI, which serves the OTA upload page on port 80.

Only `-upgrade.bin` images should be uploaded for OTA.

If connected to an existing Wi-Fi network, the OTA page will become available
over plain HTTP on port 80 on the connected Wi-Fi network. If Wi-Fi is not
configured, the repeater will instead start its own Wi-Fi access point and assigns
itself the IP address `192.168.4.1/24`.

### Optional MQTT

MQTT support, including all `mqtt.*` settings and commands, only exists in a
firmware image built with the `mqtt` feature. On other builds those settings and
commands are unavailable. MQTT also requires Wi-Fi station mode to be configured.

On an MQTT-enabled build, configure brokers 1–3 on the local CLI, then restart
MQTT:

```text
set mqtt.1.host mqtt.example.net
set mqtt.1.port 1883
set mqtt.1.username repeater
set mqtt.1.password secret
set mqtt.1.iata XXX
set mqtt.1.topic.root meshcore/{IATA}/{PUBLIC_KEY}/packets
mqtt restart
```

Repeat with `mqtt.2.*` and `mqtt.3.*` for additional brokers. Empty hosts disable
unused entries. Use `get mqtt.<number>.<key>` to inspect settings. Use `unset mqtt.1`
(or `.2`/`.3`) to clear all settings for one broker and restart MQTT. Use
`mqtt status` to show whether each broker is disabled, disconnected, connecting,
or connected.

The host setting also accepts broker URLs:

| Host value | Transport | Default port |
| --- | --- | --- |
| `mqtt.example.net` | MQTT over TCP, using `mqtt.N.port` | 1883 |
| `mqtt://mqtt.example.net` | MQTT over TCP | 1883 |
| `mqtts://mqtt.example.net` | MQTT over TLS | 8883 |
| `ws://mqtt.example.net/mqtt` | MQTT over WebSocket | 80 |
| `wss://mqtt.example.net/mqtt` | MQTT over secure WebSocket | 443 |

`http://` and `https://` are aliases for `ws://` and `wss://`. URLs can include
a custom port and path, such as `wss://mqtt.example.net:8443/custom/path`.
WebSocket paths default to `/mqtt`. The separate `mqtt.N.port` setting applies
only to bare hostnames.

For example:

```text
set mqtt.1.host https://mqtt.example.net/mqtt
mqtt restart
```

Secure connections require TLS 1.3. Certificates are not verified, so traffic is
encrypted but the broker is not authenticated. No certificate setup is needed.

For brokers using MeshCore device authentication (such as LetsMesh), enable
it for that broker using its configured host:

```text
set mqtt.1.auth device
mqtt restart
```

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
* Flood loop detection defaults to `minimal`, which drops at 4/2/1 matches for 1/2/3-byte paths.
* Guest telemetry access is always allowed without a password. Everyone likes being able to see
  which repeaters can hear which other repeaters, so that anyone can help to improve coverage
  when needed.

### Licence

MIT. Copyright © 2026 Neil Alexander.
