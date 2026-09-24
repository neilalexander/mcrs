# Configuration

MCRS uses a different configuration format to the official MeshCore repeater firmware, therefore if you are converting an existing repeater to MCRS, the configuration will not be migrated. You will have to set up the repeater again as below.

When MCRS is first installed, the repeater will have a default name `Repeater-XXXXXX`. It will not send out an advert by default. To send a zero-hop advert, hold the `PRG` button until the display shows `Zero-hop advert sent`. 

To perform initial configuration, either:

- Connect the repeater to your computer using USB and access the CLI using the USB serial port interface;
- Hold the `PRG` button to send a zero-hop advert to your companion device, and then log in using the remote management using the default password `meshcore`.

The CLI is privileged by default when accessed via USB serial port or Telnet. Meshcore remote management requires the correct admin password in order to make privileged changes to the configuration. You should change the password as soon as possible after setup to prevent others from making unauthorised changes to your repeater:

```
set password <password>
```

To view the currently stored configuration, when connected to the USB serial port CLI or Telnet interface:

```
export config
```

## Radio settings

Match the settings used by your local mesh network. For example, for 869.618MHz at 62.5KHz bandwidth, spreading factor 8, coding rate 6, 14dBm transmit power and 10% duty cycle:

```text
set radio 869.618,62.5,8,6
set tx 14
set dutycycle 10
reboot
```

Changes to the radio settings do not take effect until after a reboot.

## Device identity

Each device generates a private key automatically on the first boot and it is stored in the config. This is used to generate the repeater's public key identity. Most of the time this is sufficient to be left as-is, however if you want to reuse the private key from a different firmware or repeater, you can override it manually:

```text
set prv.key <private-key>
reboot
```

`<private-key>` can either be a 64-character hexadecimal seed, or it can be a 128-character hexadecimal keypair as used by the official MeshCore repeater firmware.

## Name and location

To customise the information that other people can see about the repeater, you can set the name, optional owner information and optional latitude/longitude location. This information is visible to anyone.

You can use a `|` pipe character to add new lines in the owner information. Coordinates use decimal degrees, for example `set lat 51.5074`. For example:

```
set name My Repeater
set owner.info My Name|myemailaddress@somewhere.com
set lat 51.494720
set lon -0.135278
```

## ACLs

In addition to the password login method, ACLs can be set to allow certain companion devices admin access with or without the password, for convenience. Similarly, you can deny login altogether to certain companion devices using their public key.

Use `unset acl <pubkey>` to remove an access rule, which restores the normal behaviour of the user requiring the password to log in. After changing the rule for a given user, they will have to log in again if they were already logged in. For example:

```
set acl D2AA1D73C8741626E74BA70C48E4CE38AA4186FCB356A7866A08AB30C3D6C7A1 admin
set acl D2AA1D73C8741626E74BA70C48E4CE38AA4186FCB356A7866A08AB30C3D6C7A1 deny
unset acl D2AA1D73C8741626E74BA70C48E4CE38AA4186FCB356A7866A08AB30C3D6C7A1
```

## Regions

MCRS supports region scopes. Configure them as normal. For example, with the `eng-ne` region:

```
region put my-region
region allowf my-region
region default my-region
```

Alternatively, flood for specific regions can be denied, for example:
```
region put another-region
region denyf another-region
```

Region capture is an MCRS-specific option that automatically scopes non-region traffic that is received directly by the repeater (not forwarded from another repeater) into the default scope before forwarding. This can help in situations where new users may not know about regions yet. However, this can result in messages being duplicated if the message is also heard by another repeater that does not also perform region capture and forwards it unscoped. Enable with:

```
region capture true
```

Regions can be removed, for example:
```
region remove another-region
```

## Wi-Fi

MCRS can connect to Wi-Fi for Telnet administration, automatic time sync and OTA updates.

Wi-Fi changes do not take effect until after a reboot, so issue a `reboot` command once done. For example:

```
set wifi.ssid My-Network
set wifi.pass mypassword
set wifi.telnet true
reboot
```

Telnet has the same privilege level as the serial console and requires no additional login, therefore you should only enable it only on a trusted network.

## Additional options

### Forwarding limits

| Command | Default | Accepted values | What it does |
| --- | --- | --- | --- |
| `set flood.max.unscoped <hops>` | `5` | Integer `0`–`63` | Maximum hops for messages being flooded without a region. |
| `set flood.max.advert <hops>` | `3` | Integer `0`–`63` | Maximum hops for flooded node advertisements. |
| `set path.hash.mode <mode>` | `2` | `0` = 1 byte, `1` = 2 bytes, `2` = 3 bytes | Size of node identifiers in this repeater’s advert paths. Larger identifiers reduce collisions but use more space. |
| `set loop.detect <mode>` | `minimal` | `off`, `minimal`, `moderate`, `strict` | Stops forwarding packets that appear to have visited this repeater already. `strict` rejects on the first match; `minimal` tolerates more matches with short identifiers. |

## MQTT

MQTT publishes received and transmitted packet information to a server. It requires an MQTT-enabled firmware image and a Wi-Fi connection.

You can configure up to three servers. Replace `<n>` below with `1`, `2` or `3`. Each has the same defaults. Use `set`, `get` and `unset` with the setting name as normal:

| Setting | Default | Accepted values | What it does |
| --- | --- | --- | --- |
| `mqtt.<n>.host` | Empty (disabled) | Hostname/IPv4 address, optionally with a port, or a URL using `mqtt://`, `mqtts://`, `ws://`, `wss://`, `http://`, `https://` | Broker endpoint. Empty disables this broker. WebSocket paths are supported; the default path is `/mqtt`. IPv6 and embedded URL credentials are not supported. |
| `mqtt.<n>.port` | `1883` | Port `1`–`65535` for a usable endpoint | Port for a bare host without its own port. URL schemes instead default to 1883 (MQTT), 8883 (MQTTS), 80 (WS/HTTP), or 443 (WSS/HTTPS); an explicit endpoint port overrides these. |
| `mqtt.<n>.username` | Empty | Text | Username for `password` authentication. |
| `mqtt.<n>.password` | Empty | Text | Password for `password` authentication. |
| `mqtt.<n>.auth` | `password` | `password` or `device` (case-insensitive) | Use configured credentials, or device-identity authentication with a signed token. Device mode requires a valid wall clock. |
| `mqtt.<n>.auth.audience` | Empty (use broker hostname) | Text | Audience in the device-authentication token. `mqtt.<n>.audience` is an accepted alias. |
| `mqtt.<n>.topic.root` | `meshcore/{IATA}/{PUBLIC_KEY}/packets` | Text; `{IATA}` and `{PUBLIC_KEY}` placeholders, also accepted as `<IATA>` and `<PUBLIC_KEY>` | Packet publication topic. Placeholders expand to the configured IATA value and the node's full public key. |
| `mqtt.<n>.iata` | `XXX` | Text | Value substituted for the IATA placeholder; intended as a location code. |

To disable an MQTT server, for example server `1`, do `unset mqtt.1` and then `mqtt restart`.
