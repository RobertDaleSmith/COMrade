/**
 * COMrade UART Bridge
 *
 * Bridges USB CDC ↔ UART via PIO on any RP2040 board.
 * On Pico W builds (BLE_ENABLED), also bridges BLE NUS ↔ UART.
 *
 * Auto-detects which GPIO pair has UART wired to it and which pin is
 * TX vs RX.  Just plug two adjacent GPIOs + GND into the target device
 * and the firmware figures out the rest.
 *
 * Configuration commands can be sent over USB CDC (or BLE NUS) with the
 * $CB: prefix.  Type $CB:help for a list of commands.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "pico/stdlib.h"
#include "hardware/pio.h"
#include "hardware/gpio.h"
#include "hardware/clocks.h"
#include "hardware/flash.h"
#include "hardware/sync.h"
#include "hardware/watchdog.h"

#include "uart_tx.pio.h"
#include "uart_rx.pio.h"

#ifdef BLE_ENABLED
#include "pico/cyw43_arch.h"
#include "ble_nus.h"
#endif

// ---- Constants ----

#define NUM_GPIO       30
#define CMD_PREFIX     "$CB:"
#define CMD_PREFIX_LEN 4
#define CMD_BUF_SIZE   64

// ---- Persistent config (stored in last flash sector) ----

#define CONFIG_MAGIC   0xCB01  // Bump to invalidate old configs.
#define CONFIG_OFFSET  (PICO_FLASH_SIZE_BYTES - FLASH_SECTOR_SIZE)

typedef struct {
    uint16_t magic;
    uint32_t baud;
    int8_t   pin_a;     // -1 = auto-detect
    int8_t   pin_b;     // -1 = auto-detect
    int8_t   led_pin;   // -1 = disabled, -2 = use board default
} config_t;

static config_t cfg;

static void config_defaults(void) {
    cfg.magic   = CONFIG_MAGIC;
    cfg.baud    = 115200;
    cfg.pin_a   = -1;
    cfg.pin_b   = -1;
    cfg.led_pin = -2;  // Board default.
}

static void config_load(void) {
    const config_t *stored = (const config_t *)(XIP_BASE + CONFIG_OFFSET);
    if (stored->magic == CONFIG_MAGIC) {
        cfg = *stored;
    } else {
        config_defaults();
    }
}

static void config_save(void) {
    cfg.magic = CONFIG_MAGIC;
    // Flash write must be done with interrupts disabled.
    uint32_t ints = save_and_disable_interrupts();
    flash_range_erase(CONFIG_OFFSET, FLASH_SECTOR_SIZE);
    flash_range_program(CONFIG_OFFSET, (const uint8_t *)&cfg, FLASH_PAGE_SIZE);
    restore_interrupts(ints);
}

// ---- LED ----

static int active_led_pin(void) {
    if (cfg.led_pin == -2) {
#ifdef PICO_DEFAULT_LED_PIN
        return PICO_DEFAULT_LED_PIN;
#else
        return -1;
#endif
    }
    return cfg.led_pin;
}

static void led_init(void) {
    int pin = active_led_pin();
    if (pin >= 0) {
        gpio_init(pin);
        gpio_set_dir(pin, GPIO_OUT);
        gpio_put(pin, 0);
    }
}

static void led_set(bool on) {
    int pin = active_led_pin();
    if (pin >= 0) {
#ifdef BLE_ENABLED
        // Pico W LED is on the CYW43 chip, not a GPIO.
        if (pin == CYW43_WL_GPIO_LED_PIN) {
            cyw43_arch_gpio_put(pin, on);
            return;
        }
#endif
        gpio_put(pin, on);
    }
}

// ---- Auto-detect UART pin pair ----

static void detect_pins(uint *pin_rx, uint *pin_tx) {
    int led = active_led_pin();

    for (uint i = 0; i < NUM_GPIO; i++) {
        if ((int)i == led) continue;
        gpio_init(i);
        gpio_set_dir(i, GPIO_IN);
        gpio_pull_up(i);
    }

    const uint32_t char_time_us = (10 * 1000000) / cfg.baud;

    bool led_on = false;
    absolute_time_t next_blink = make_timeout_time_ms(250);

    while (true) {
        for (uint i = 0; i < NUM_GPIO; i++) {
            if ((int)i == led) continue;
            if (gpio_get(i)) continue;

            sleep_us(char_time_us);
            if (!gpio_get(i)) continue;

            *pin_rx = i;
            if (i + 1 < NUM_GPIO && (int)(i + 1) != led) {
                *pin_tx = i + 1;
            } else {
                *pin_tx = i - 1;
            }
            return;
        }

        if (time_reached(next_blink)) {
            led_on = !led_on;
            led_set(led_on);
            next_blink = make_timeout_time_ms(250);
        }

        tight_loop_contents();
    }
}

// ---- PIO UART bridge ----

static PIO pio_inst = pio0;
static uint sm_tx;
static uint sm_rx;
static bool bridge_active = false;
static uint current_pin_tx;
static uint current_pin_rx;

static void bridge_init(uint pin_tx, uint pin_rx) {
    sm_tx = pio_claim_unused_sm(pio_inst, true);
    sm_rx = pio_claim_unused_sm(pio_inst, true);

    uint offset_tx = pio_add_program(pio_inst, &uart_tx_program);
    uint offset_rx = pio_add_program(pio_inst, &uart_rx_program);

    uart_tx_program_init(pio_inst, sm_tx, offset_tx, pin_tx, cfg.baud);
    uart_rx_program_init(pio_inst, sm_rx, offset_rx, pin_rx, cfg.baud);

    current_pin_tx = pin_tx;
    current_pin_rx = pin_rx;
    bridge_active = true;
}

// ---- Command handler ----

// Response output: sends to USB CDC (printf) and optionally BLE NUS.
static void cmd_respond(const char *msg) {
    printf("[bridge] %s\n", msg);
#ifdef BLE_ENABLED
    if (ble_nus_connected()) {
        char buf[128];
        int n = snprintf(buf, sizeof(buf), "[bridge] %s\n", msg);
        if (n > 0) ble_nus_send((const uint8_t *)buf, (uint16_t)n);
    }
#endif
}

static void cmd_status(void) {
    const char *mode = (cfg.pin_a < 0) ? "auto" : "manual";
    char buf[256];
    int pos = snprintf(buf, sizeof(buf), "[bridge] mode=%s baud=%lu", mode, cfg.baud);
    if (cfg.pin_a >= 0) {
        pos += snprintf(buf + pos, sizeof(buf) - pos,
                        " pin_a=GPIO%d pin_b=GPIO%d", cfg.pin_a, cfg.pin_b);
    }
    if (bridge_active) {
        pos += snprintf(buf + pos, sizeof(buf) - pos,
                        " rx=GPIO%d tx=GPIO%d", current_pin_rx, current_pin_tx);
    }
    int led = active_led_pin();
    if (led >= 0) {
        pos += snprintf(buf + pos, sizeof(buf) - pos, " led=GPIO%d", led);
    } else {
        pos += snprintf(buf + pos, sizeof(buf) - pos, " led=off");
    }
#ifdef BLE_ENABLED
    pos += snprintf(buf + pos, sizeof(buf) - pos,
                    " ble=%s", ble_nus_connected() ? "connected" : "advertising");
#endif
    snprintf(buf + pos, sizeof(buf) - pos, "\n");
    printf("%s", buf);
#ifdef BLE_ENABLED
    if (ble_nus_connected()) {
        ble_nus_send((const uint8_t *)buf, (uint16_t)strlen(buf));
    }
#endif
}

static void cmd_help(void) {
    cmd_respond("Commands:");
    cmd_respond("  $CB:status          Show current config");
    cmd_respond("  $CB:pins <a> <b>    Set manual pin pair");
    cmd_respond("  $CB:auto            Switch to auto-detect");
    cmd_respond("  $CB:baud <rate>     Set baud rate");
    cmd_respond("  $CB:led <pin>       Set LED pin (-1 off)");
    cmd_respond("  $CB:save            Save config to flash");
    cmd_respond("  $CB:reset           Factory reset");
    cmd_respond("  $CB:reboot          Reboot the bridge");
    cmd_respond("  $CB:help            Show this help");
}

// Returns true if the line was a command (consumed), false if passthrough.
static bool handle_command(const char *line) {
    if (strncmp(line, CMD_PREFIX, CMD_PREFIX_LEN) != 0)
        return false;

    const char *cmd = line + CMD_PREFIX_LEN;

    if (strcmp(cmd, "status") == 0) {
        cmd_status();
    } else if (strcmp(cmd, "help") == 0) {
        cmd_help();
    } else if (strncmp(cmd, "pins ", 5) == 0) {
        int a, b;
        if (sscanf(cmd + 5, "%d %d", &a, &b) == 2 &&
            a >= 0 && a < NUM_GPIO && b >= 0 && b < NUM_GPIO && a != b) {
            cfg.pin_a = (int8_t)a;
            cfg.pin_b = (int8_t)b;
            char msg[64];
            snprintf(msg, sizeof(msg), "Pins set to GPIO%d, GPIO%d (reboot to apply)", a, b);
            cmd_respond(msg);
        } else {
            cmd_respond("Invalid pins. Usage: $CB:pins <a> <b>");
        }
    } else if (strcmp(cmd, "auto") == 0) {
        cfg.pin_a = -1;
        cfg.pin_b = -1;
        cmd_respond("Switched to auto-detect (reboot to apply)");
    } else if (strncmp(cmd, "baud ", 5) == 0) {
        uint32_t rate = (uint32_t)atoi(cmd + 5);
        if (rate >= 300 && rate <= 921600) {
            cfg.baud = rate;
            char msg[64];
            snprintf(msg, sizeof(msg), "Baud set to %lu (reboot to apply)", rate);
            cmd_respond(msg);
        } else {
            cmd_respond("Invalid baud. Range: 300-921600");
        }
    } else if (strncmp(cmd, "led ", 4) == 0) {
        int pin = atoi(cmd + 4);
        if (pin >= -1 && pin < NUM_GPIO) {
            cfg.led_pin = (int8_t)pin;
            char msg[64];
            snprintf(msg, sizeof(msg), "LED pin set to %d (reboot to apply)", pin);
            cmd_respond(msg);
        } else {
            cmd_respond("Invalid LED pin. Use -1 to disable.");
        }
    } else if (strcmp(cmd, "save") == 0) {
        config_save();
        cmd_respond("Config saved to flash");
    } else if (strcmp(cmd, "reset") == 0) {
        config_defaults();
        config_save();
        cmd_respond("Factory reset. Rebooting...");
        sleep_ms(100);
        watchdog_reboot(0, 0, 0);
    } else if (strcmp(cmd, "reboot") == 0) {
        cmd_respond("Rebooting...");
        sleep_ms(100);
        watchdog_reboot(0, 0, 0);
    } else {
        char msg[64];
        snprintf(msg, sizeof(msg), "Unknown command: %s", cmd);
        cmd_respond(msg);
        cmd_respond("Type $CB:help for commands");
    }

    return true;
}

// ---- Command parser (reused for both CDC and BLE input) ----

typedef struct {
    char buf[CMD_BUF_SIZE];
    uint len;
    bool in_cmd;
} cmd_parser_t;

static void cmd_parser_init(cmd_parser_t *p) {
    p->len = 0;
    p->in_cmd = false;
}

/// Feed a byte from a host interface (CDC or BLE).
/// Returns true if the byte was consumed (part of a $CB: command).
/// Returns false if the byte should be forwarded to UART.
static bool cmd_parser_feed(cmd_parser_t *p, char c) {
    if (c == '\n' || c == '\r') {
        if (p->in_cmd && p->len > 0) {
            p->buf[p->len] = '\0';
            handle_command(p->buf);
        }
        p->len = 0;
        p->in_cmd = false;
        return p->in_cmd;  // Newlines after commands are consumed.
    }

    if (p->len < CMD_BUF_SIZE - 1) {
        p->buf[p->len++] = c;
    }

    if (p->len <= CMD_PREFIX_LEN) {
        if (strncmp(p->buf, CMD_PREFIX, p->len) == 0) {
            if (p->len == CMD_PREFIX_LEN) {
                p->in_cmd = true;
            }
            return true;  // Buffering prefix — don't forward yet.
        } else {
            // Not a command — caller should flush buffered bytes.
            return false;
        }
    }

    return p->in_cmd;
}

// ---- BLE NUS RX callback ----

#ifdef BLE_ENABLED
static cmd_parser_t ble_parser;

static void ble_rx_callback(const uint8_t *data, uint16_t len) {
    for (uint16_t i = 0; i < len; i++) {
        char c = (char)data[i];
        bool consumed = cmd_parser_feed(&ble_parser, c);

        if (!consumed && bridge_active) {
            // Not a command — forward to UART.
            if (ble_parser.len <= CMD_PREFIX_LEN && ble_parser.len > 0) {
                // Flush buffered prefix bytes that turned out to not be a command.
                for (uint j = 0; j < ble_parser.len; j++) {
                    uart_tx_program_putc(pio_inst, sm_tx, ble_parser.buf[j]);
                }
                ble_parser.len = 0;
                ble_parser.in_cmd = false;
            } else {
                uart_tx_program_putc(pio_inst, sm_tx, c);
            }
        }
    }
}
#endif

// ---- Main ----

int main() {
#ifdef BLE_ENABLED
    // CYW43 must init before stdio to avoid USB hang.
    if (cyw43_arch_init()) {
        // Fall through — USB CDC still works without BLE.
    }
#endif

    stdio_init_all();
    config_load();
    led_init();

#ifdef BLE_ENABLED
    cmd_parser_init(&ble_parser);
    ble_nus_init(ble_rx_callback);
    ble_nus_start();
#endif

    while (!stdio_usb_connected())
        sleep_ms(50);

    printf("[bridge] COMrade UART Bridge @ %lu baud", cfg.baud);
#ifdef BLE_ENABLED
    printf(" (BLE enabled)");
#endif
    printf("\n");

    uint pin_rx, pin_tx;

    if (cfg.pin_a >= 0 && cfg.pin_b >= 0) {
        // Manual pin assignment — still auto-detect TX/RX orientation.
        printf("[bridge] Manual pins: GPIO%d, GPIO%d — detecting orientation...\n",
               cfg.pin_a, cfg.pin_b);

        // Set up just these two pins for detection.
        gpio_init(cfg.pin_a);
        gpio_set_dir(cfg.pin_a, GPIO_IN);
        gpio_pull_up(cfg.pin_a);
        gpio_init(cfg.pin_b);
        gpio_set_dir(cfg.pin_b, GPIO_IN);
        gpio_pull_up(cfg.pin_b);

        const uint32_t char_time_us = (10 * 1000000) / cfg.baud;
        bool led_on = false;
        absolute_time_t next_blink = make_timeout_time_ms(250);

        while (true) {
            if (!gpio_get(cfg.pin_a)) {
                sleep_us(char_time_us);
                if (gpio_get(cfg.pin_a)) { pin_rx = cfg.pin_a; pin_tx = cfg.pin_b; break; }
            }
            if (!gpio_get(cfg.pin_b)) {
                sleep_us(char_time_us);
                if (gpio_get(cfg.pin_b)) { pin_rx = cfg.pin_b; pin_tx = cfg.pin_a; break; }
            }
            if (time_reached(next_blink)) {
                led_on = !led_on;
                led_set(led_on);
                next_blink = make_timeout_time_ms(250);
            }
            tight_loop_contents();
        }
    } else {
        printf("[bridge] Scanning all GPIOs for UART activity...\n");
        detect_pins(&pin_rx, &pin_tx);
    }

    printf("[bridge] Detected: RX=GPIO%d  TX=GPIO%d\n", pin_rx, pin_tx);

    led_set(true);
    bridge_init(pin_tx, pin_rx);

    // Bridge loop with command interception.
    cmd_parser_t cdc_parser;
    cmd_parser_init(&cdc_parser);

    while (true) {
        // PIO UART RX → USB CDC + BLE NUS
        while (!pio_sm_is_rx_fifo_empty(pio_inst, sm_rx)) {
            char c = (char)(pio_inst->rxf[sm_rx] >> 24);
            putchar_raw(c);
#ifdef BLE_ENABLED
            ble_nus_send((const uint8_t *)&c, 1);
#endif
        }

        // USB CDC → check for commands or forward to PIO UART TX
        int ch;
        while ((ch = getchar_timeout_us(0)) != PICO_ERROR_TIMEOUT) {
            char c = (char)ch;
            bool consumed = cmd_parser_feed(&cdc_parser, c);

            if (!consumed && bridge_active) {
                if (cdc_parser.len <= CMD_PREFIX_LEN && cdc_parser.len > 0) {
                    // Flush buffered prefix bytes.
                    for (uint i = 0; i < cdc_parser.len; i++) {
                        uart_tx_program_putc(pio_inst, sm_tx, cdc_parser.buf[i]);
                    }
                    cdc_parser.len = 0;
                    cdc_parser.in_cmd = false;
                } else {
                    uart_tx_program_putc(pio_inst, sm_tx, c);
                }
            }
        }

        tight_loop_contents();
    }

    return 0;
}
