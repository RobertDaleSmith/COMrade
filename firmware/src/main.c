/**
 * COMrade UART Bridge
 *
 * Bridges USB CDC ↔ UART via PIO on any RP2040 board.
 * Auto-detects which GPIO pair has UART wired to it and which pin is
 * TX vs RX.  Just plug two adjacent GPIOs + GND into the target device
 * and the firmware figures out the rest.
 *
 * Baud rate is set at build time (default 115200).
 */

#include <stdio.h>
#include <string.h>
#include "pico/stdlib.h"
#include "hardware/pio.h"
#include "hardware/gpio.h"
#include "hardware/clocks.h"

#include "uart_tx.pio.h"
#include "uart_rx.pio.h"

// ---- Configuration ----

#ifndef BRIDGE_BAUD
#define BRIDGE_BAUD 115200
#endif

// Number of usable GPIOs to scan (RP2040 has GPIO 0–29).
#define NUM_GPIO 30

// ---- LED ----

#ifndef BRIDGE_LED_PIN
#ifdef PICO_DEFAULT_LED_PIN
#define BRIDGE_LED_PIN PICO_DEFAULT_LED_PIN
#else
#define BRIDGE_LED_PIN -1
#endif
#endif

// ---- Auto-detect UART pin pair ----

/**
 * Scan all GPIOs for UART activity (start bits).
 *
 * Sets every GPIO as input with pull-up, then watches for falling edges.
 * UART idle is HIGH; only the pin connected to the remote device's TX will
 * pulse LOW.  Once detected, the adjacent GPIO is assumed to be TX.
 *
 * To avoid false triggers from noise on floating pins, we require a full
 * UART frame: start bit LOW, then the line must return HIGH (stop bit)
 * within one character time (~87 µs at 115200 baud for 10 bits).
 *
 * Blinks LED while scanning.  Sets pin_rx and pin_tx on success.
 */
static void detect_pins(uint *pin_rx, uint *pin_tx) {
    // Init all GPIOs as inputs with pull-up.
    for (uint i = 0; i < NUM_GPIO; i++) {
        // Skip the LED pin.
        if ((int)i == BRIDGE_LED_PIN) continue;
        gpio_init(i);
        gpio_set_dir(i, GPIO_IN);
        gpio_pull_up(i);
    }

    // One character time in µs: 10 bits (start + 8 data + stop) at BRIDGE_BAUD.
    const uint32_t char_time_us = (10 * 1000000) / BRIDGE_BAUD;

    bool led_on = false;
    absolute_time_t next_blink = make_timeout_time_ms(250);

    while (true) {
        for (uint i = 0; i < NUM_GPIO; i++) {
            if ((int)i == BRIDGE_LED_PIN) continue;
            if (gpio_get(i)) continue;

            // Saw a LOW — potential start bit.  Wait one character time
            // and check that the line returns HIGH (valid stop bit).
            sleep_us(char_time_us);
            if (!gpio_get(i)) continue;  // Still LOW — not a UART frame, skip.

            // Valid frame detected on GPIO i.  That's our RX.
            *pin_rx = i;

            // TX is the adjacent pin.  Prefer i+1; fall back to i-1.
            if (i + 1 < NUM_GPIO && (int)(i + 1) != BRIDGE_LED_PIN) {
                *pin_tx = i + 1;
            } else if (i > 0 && (int)(i - 1) != BRIDGE_LED_PIN) {
                *pin_tx = i - 1;
            } else {
                // Edge case: no valid neighbor.  Shouldn't happen in practice.
                *pin_tx = (i + 1) % NUM_GPIO;
            }
            return;
        }

        // Blink LED while scanning.
        if (BRIDGE_LED_PIN >= 0 && time_reached(next_blink)) {
            led_on = !led_on;
            gpio_put(BRIDGE_LED_PIN, led_on);
            next_blink = make_timeout_time_ms(250);
        }

        tight_loop_contents();
    }
}

// ---- PIO UART bridge ----

static PIO pio = pio0;
static uint sm_tx;
static uint sm_rx;

static void bridge_init(uint pin_tx, uint pin_rx, uint baud) {
    sm_tx = pio_claim_unused_sm(pio, true);
    sm_rx = pio_claim_unused_sm(pio, true);

    uint offset_tx = pio_add_program(pio, &uart_tx_program);
    uint offset_rx = pio_add_program(pio, &uart_rx_program);

    uart_tx_program_init(pio, sm_tx, offset_tx, pin_tx, baud);
    uart_rx_program_init(pio, sm_rx, offset_rx, pin_rx, baud);
}

// ---- Main ----

int main() {
    stdio_init_all();

    if (BRIDGE_LED_PIN >= 0) {
        gpio_init(BRIDGE_LED_PIN);
        gpio_set_dir(BRIDGE_LED_PIN, GPIO_OUT);
        gpio_put(BRIDGE_LED_PIN, 0);
    }

    while (!stdio_usb_connected())
        sleep_ms(50);

    printf("[bridge] COMrade UART Bridge @ %d baud\n", BRIDGE_BAUD);
    printf("[bridge] Scanning all GPIOs for UART activity...\n");

    uint pin_rx, pin_tx;
    detect_pins(&pin_rx, &pin_tx);

    printf("[bridge] Detected: RX=GPIO%d  TX=GPIO%d\n", pin_rx, pin_tx);

    // Solid LED = bridge active.
    if (BRIDGE_LED_PIN >= 0)
        gpio_put(BRIDGE_LED_PIN, 1);

    bridge_init(pin_tx, pin_rx, BRIDGE_BAUD);

    // Bridge loop.
    while (true) {
        // PIO UART RX → USB CDC
        while (!pio_sm_is_rx_fifo_empty(pio, sm_rx)) {
            char c = (char)(pio->rxf[sm_rx] >> 24);
            putchar_raw(c);
        }

        // USB CDC → PIO UART TX
        int ch;
        while ((ch = getchar_timeout_us(0)) != PICO_ERROR_TIMEOUT) {
            uart_tx_program_putc(pio, sm_tx, (char)ch);
        }

        tight_loop_contents();
    }

    return 0;
}
