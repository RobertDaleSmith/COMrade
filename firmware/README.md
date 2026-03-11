# COMrade UART Bridge Firmware

RP2040 firmware that bridges USB CDC ↔ UART via PIO. Turns any Pico-based board
into a USB-to-TTL serial adapter for use with COMrade.

**Fully automatic** — plug any two adjacent GPIOs + GND into the target device.
The firmware scans all GPIOs to find which pin has UART data, detects TX/RX
orientation, and starts bridging. No configuration needed.

## Two Build Variants

| Build | Board | Transport | UF2 |
|-------|-------|-----------|-----|
| `make firmware` | Any RP2040 (Pico, KB2040, etc.) | USB CDC only | `build/uart-bridge.uf2` |
| `make firmware-w` | Pi Pico W | USB CDC + BLE NUS | `build-w/uart-bridge-w.uf2` |

The Pico W build adds **wireless serial** via BLE Nordic UART Service (NUS).
Connect over USB as usual, or wirelessly from COMrade's BLE device list.
Both transports bridge to the same UART — they can be used simultaneously.

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

# Standard RP2040 (any board)
make firmware        # builds firmware/build/uart-bridge.uf2

# Pi Pico W (USB + BLE)
make firmware-w      # builds firmware/build-w/uart-bridge-w.uf2
```

## Flashing

1. Hold BOOTSEL on the Pico and plug in USB
2. Drag the `.uf2` file to the RPI-RP2 drive
3. Board reboots and appears as a USB serial port

## Configuration

Send commands over USB CDC (or BLE NUS) with the `$CB:` prefix. These are
intercepted by the bridge and never forwarded to UART.

| Command | Description |
|---------|-------------|
| `$CB:status` | Show current pins, baud, mode, LED, BLE status |
| `$CB:pins <a> <b>` | Set manual pin pair (reboot to apply) |
| `$CB:auto` | Switch to auto-detect mode |
| `$CB:baud <rate>` | Set baud rate (300–921600) |
| `$CB:led <pin>` | Set LED pin (-1 to disable) |
| `$CB:save` | Persist current config to flash |
| `$CB:reset` | Factory reset and reboot |
| `$CB:reboot` | Reboot the bridge |
| `$CB:help` | Show command list |

Settings take effect after reboot. Use `$CB:save` to persist across
power cycles. Commands work identically over USB CDC and BLE NUS.

## LED Status

On boards with a default LED (e.g. Pi Pico GPIO 25, Pico W CYW43 LED):

- **Blinking** — scanning GPIOs for UART activity
- **Solid** — bridge active, forwarding data

The LED pin can be changed with `$CB:led <pin>`.

## BLE NUS (Pico W only)

The Pico W build advertises as **"COMrade Bridge"** with the Nordic UART
Service. Any BLE NUS client can connect — COMrade's app discovers it
automatically in the device list.

Data flow:
- UART RX → USB CDC **and** BLE NUS TX (both get the data)
- USB CDC input → UART TX
- BLE NUS RX → UART TX
- `$CB:` commands work from either transport

## How auto-detect works

1. All GPIOs are set as inputs with pull-ups
2. The firmware scans for a falling edge (UART start bit) on any pin
3. It validates a full frame (line returns HIGH after one character time)
4. The pin with valid UART data becomes RX; the adjacent pin becomes TX
5. PIO UART is configured and bridging begins

If pins are manually configured, auto-detect is limited to just those
two pins (still detects TX/RX orientation automatically).
