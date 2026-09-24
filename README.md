# xmip-core-sdk

Simulators and emulators: the media a module is tested on, in process, with no
hardware and no network. And, for now, the local ACME server the identity tests
obtain certificates from ([ADR-0061](https://github.com/IlleNilsson/Xmip/blob/main/doc/decision/ADR-0061-a-provider-builds-against-the-sdk.md)).

A protocol is proved on the medium it rides: addresses, silence, collisions,
turnaround, lost characters. A medium belongs to no one protocol, so it lives
here rather than in any technology. Core's technologies are proved on these,
and so is a provider's. An end user may select one too, where a line is to be
simulated rather than wired.

The traits a module implements are not here. Each belongs to its capability:
a contract implements `xmip-core-contract`'s, a transport
`xmip-core-transport`'s.

## What is in it today

| Module | Simulates | Used by |
| --- | --- | --- |
| `sdk::serial` | a multi-drop serial bus: addressed devices, silence, collision, turnaround, unsolicited frames, a lost character, a break | M-Bus meters, HART field devices |

On the way, in open problems 24 and 27: the CAN bus, the radios a wireless
protocol speaks over, and the local ACME server.

## A device on the serial bus

A device hears every frame on the bus and answers the ones addressed to it:

```rust
use sdk::serial::{Bus, Device};

struct Meter;                       // impl Device for Meter { fn hear(..) }

let bus = Bus::new("rs485");
bus.attach(Arc::new(Meter));
// A master writes to the bus as it would to a port: the bus is a Line.
```

`xmip-core-transport-m-bus` and `xmip-core-transport-hart` in the estate are
complete examples, with two devices on one bus.

## Toolchain

`rust-toolchain.toml` is the estate's toolchain; rustup reads it.
