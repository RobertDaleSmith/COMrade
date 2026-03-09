# COMrade UART Bridge Firmware

RP2040 firmware that bridges USB CDC ↔ UART via PIO. Turns any Pico-based board
into a USB-to-TTL serial adapter for use with COMrade.

**Fully automatic** — plug any two adjacent GPIOs + GND into the target device.
The firmware scans all GPIOs to find which pin has UART data, detects TX/RX
orientation, and starts bridging. No configuration needed.

## Wiring

Connect **two adjacent GPIO pins** + GND between your Pico and the target:

| Pico side       | Target device |
|-----------------|---------------|
| Any GPIO (N)    | TX _or_ RX    |
| Adjacent (N±1)  | RX _or_ TX    |
| GND             | GND           |

Pin order and orientation don't matter — the firmware figures it out.

## Building

Requires the [Pico SDK](https://github.com/raspberrypi/pico-sdk).

```bash
export PICO_SDK_PATH=~/pico-sdk

mkdir build && cd build
cmake ..
make -j

# Custom baud rate (default 115200)
cmake .. -DBRIDGE_BAUD=9600
```

## Flashing

1. Hold BOOTSEL on the Pico and plug in USB
2. Drag `build/uart-bridge.uf2` to the RPI-RP2 drive
3. Board reboots and appears as a USB serial port

One UF2 works on any RP2040 board (Pico, KB2040, etc.).

## LED Status

- **Blinking** — scanning GPIOs for UART activity
- **Solid** — bridge active, forwarding data

## How auto-detect works

1. All GPIOs are set as inputs with pull-ups
2. The firmware scans for a falling edge (UART start bit) on any pin
3. It validates a full frame (line returns HIGH after one character time)
4. The pin with valid UART data becomes RX; the adjacent pin becomes TX
5. PIO UART is configured and bridging begins
