# COMrade UART Bridge Firmware

RP2040 firmware that bridges USB CDC ↔ UART via PIO. Turns any Pico-based board
into a USB-to-TTL serial adapter for use with COMrade.

**Auto-detect TX/RX** — plug the two UART wires in either order. The firmware
detects which pin is connected to the remote device's TX and configures itself.

## Wiring

Connect **two wires** + GND between your Pico and the target device:

| Pico (default)  | KB2040 (default) | Target device |
|------------------|------------------|---------------|
| GPIO 0           | GPIO 12          | TX _or_ RX    |
| GPIO 1           | GPIO 13          | RX _or_ TX    |
| GND              | GND              | GND           |

Order of the signal wires doesn't matter — the firmware figures it out.

## Building

Requires the [Pico SDK](https://github.com/raspberrypi/pico-sdk).

```bash
# Set SDK path
export PICO_SDK_PATH=~/pico-sdk

# Pi Pico (GPIO 0, 1)
mkdir build && cd build
cmake .. -DBRIDGE_BOARD=pico
make -j

# Adafruit KB2040 (GPIO 12, 13)
mkdir build && cd build
cmake .. -DBRIDGE_BOARD=kb2040

# Custom pins
cmake .. -DBRIDGE_PIN_A=4 -DBRIDGE_PIN_B=5

# Custom baud rate (default 115200)
cmake .. -DBRIDGE_BOARD=pico -DBRIDGE_BAUD=9600
```

## Flashing

1. Hold BOOTSEL on the Pico and plug in USB
2. Drag `build/uart-bridge.uf2` to the RPI-RP2 drive
3. Board reboots and appears as a USB serial port

## LED Status

- **Blinking** — waiting for UART activity (detecting pin orientation)
- **Solid** — bridge active, forwarding data

## How auto-detect works

UART lines idle HIGH. At boot, both GPIO pins are configured as inputs with
pull-ups. The firmware watches for the first falling edge (start bit) — that
pin must be connected to the remote device's TX, so it becomes our RX. The
other pin becomes TX. Detection requires the remote device to send at least
one byte.
