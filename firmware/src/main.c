/**
 * COMrade UART Bridge
 *
 * Bridges USB CDC ↔ UART via PIO on any two GPIOs.
 * Auto-detects TX/RX orientation: plug the two wires in either way and it
 * figures out which pin is connected to the remote device's TX line.
 *
 * Pin pair is set at build time via BRIDGE_PIN_A / BRIDGE_PIN_B.
 * Defaults: Pico = 0,1 — KB2040 = 12,13
 */

#include <stdio.h>
#include <string.h>
#include "pico/stdlib.h"
#include "hardware/pio.h"
#include "hardware/gpio.h"
#include "hardware/clocks.h"

#include "uart_tx.pio.h"
#include "uart_rx.pio.h"

// ---- Build-time pin configuration ----

#ifndef BRIDGE_PIN_A
#define BRIDGE_PIN_A 0
#endif

#ifndef BRIDGE_PIN_B
#define BRIDGE_PIN_B 1
#endif

#ifndef BRIDGE_BAUD
#define BRIDGE_BAUD 115200
#endif

// ---- LED (optional status indicator) ----

#ifndef BRIDGE_LED_PIN
#ifdef PICO_DEFAULT_LED_PIN
#define BRIDGE_LED_PIN PICO_DEFAULT_LED_PIN
#else
#define BRIDGE_LED_PIN -1
#endif
#endif

// ---- Auto-detect which pin is RX (remote device's TX) ----

/**
 * Monitor both pins for UART start bits (falling edges).
 * UART idle = HIGH. The pin connected to the remote TX will go LOW first.
 * Returns the pin that saw activity (= our RX pin).
 * Blinks LED while waiting.
 */
static int detect_rx_pin(uint pin_a, uint pin_b) {
    // Configure both as inputs with pull-ups (UART idle is HIGH).
    gpio_init(pin_a);
    gpio_set_dir(pin_a, GPIO_IN);
    gpio_pull_up(pin_a);

    gpio_init(pin_b);
    gpio_set_dir(pin_b, GPIO_IN);
    gpio_pull_up(pin_b);

    bool led_on = false;
    absolute_time_t next_blink = make_timeout_time_ms(250);

    while (true) {
        // Check for LOW (start bit) on either pin.
        if (!gpio_get(pin_a)) return pin_a;
        if (!gpio_get(pin_b)) return pin_b;

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
    // Claim two state machines.
    sm_tx = pio_claim_unused_sm(pio, true);
    sm_rx = pio_claim_unused_sm(pio, true);

    // Load PIO programs.
    uint offset_tx = pio_add_program(pio, &uart_tx_program);
    uint offset_rx = pio_add_program(pio, &uart_rx_program);

    // Initialize TX and RX.
    uart_tx_program_init(pio, sm_tx, offset_tx, pin_tx, baud);
    uart_rx_program_init(pio, sm_rx, offset_rx, pin_rx, baud);
}

// ---- Main ----

int main() {
    // Init USB CDC stdio.
    stdio_init_all();

    // Init LED if available.
    if (BRIDGE_LED_PIN >= 0) {
        gpio_init(BRIDGE_LED_PIN);
        gpio_set_dir(BRIDGE_LED_PIN, GPIO_OUT);
        gpio_put(BRIDGE_LED_PIN, 0);
    }

    // Wait for USB host to connect.
    while (!stdio_usb_connected())
        sleep_ms(50);

    printf("[bridge] COMrade UART Bridge\n");
    printf("[bridge] Pins: %d, %d @ %d baud\n", BRIDGE_PIN_A, BRIDGE_PIN_B, BRIDGE_BAUD);
    printf("[bridge] Detecting TX/RX orientation...\n");

    // Auto-detect which pin is RX.
    uint pin_rx = detect_rx_pin(BRIDGE_PIN_A, BRIDGE_PIN_B);
    uint pin_tx = (pin_rx == BRIDGE_PIN_A) ? BRIDGE_PIN_B : BRIDGE_PIN_A;

    printf("[bridge] Detected: RX=GPIO%d  TX=GPIO%d\n", pin_rx, pin_tx);

    // Solid LED = connected and bridging.
    if (BRIDGE_LED_PIN >= 0)
        gpio_put(BRIDGE_LED_PIN, 1);

    // Start PIO UART.
    bridge_init(pin_tx, pin_rx, BRIDGE_BAUD);

    // Bridge loop: forward bytes in both directions.
    while (true) {
        // PIO UART RX → USB CDC TX
        while (!pio_sm_is_rx_fifo_empty(pio, sm_rx)) {
            char c = (char)(pio->rxf[sm_rx] >> 24);
            putchar_raw(c);
        }

        // USB CDC RX → PIO UART TX
        int ch;
        while ((ch = getchar_timeout_us(0)) != PICO_ERROR_TIMEOUT) {
            uart_tx_program_putc(pio, sm_tx, (char)ch);
        }

        // Yield briefly to avoid busy-spinning at 100% CPU.
        tight_loop_contents();
    }

    return 0;
}
