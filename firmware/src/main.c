/**
 * COMrade UART Bridge
 *
 * Bridges USB CDC ↔ UART via PIO on any RP2040 board.
 * Auto-detects which GPIO pair has UART wired to it and which pin is
 * TX vs RX.  Just plug two adjacent GPIOs + GND into the target device
 * and the firmware figures out the rest.
 *
 * Configuration commands can be sent over USB CDC with the $CB: prefix.
 * Type $CB:help for a list of commands.  Settings persist across reboots.
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
    if (pin >= 0) gpio_put(pin, on);
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

static void cmd_respond(const char *msg) {
    printf("[bridge] %s\n", msg);
}

static void cmd_status(void) {
    const char *mode = (cfg.pin_a < 0) ? "auto" : "manual";
    printf("[bridge] mode=%s baud=%lu", mode, cfg.baud);
    if (cfg.pin_a >= 0) {
        printf(" pin_a=GPIO%d pin_b=GPIO%d", cfg.pin_a, cfg.pin_b);
    }
    if (bridge_active) {
        printf(" rx=GPIO%d tx=GPIO%d", current_pin_rx, current_pin_tx);
    }
    int led = active_led_pin();
    if (led >= 0) {
        printf(" led=GPIO%d", led);
    } else {
        printf(" led=off");
    }
    printf("\n");
}

static void cmd_help(void) {
    printf("[bridge] Commands:\n");
    printf("[bridge]   $CB:status          Show current config\n");
    printf("[bridge]   $CB:pins <a> <b>    Set manual pin pair\n");
    printf("[bridge]   $CB:auto            Switch to auto-detect\n");
    printf("[bridge]   $CB:baud <rate>     Set baud rate\n");
    printf("[bridge]   $CB:led <pin>       Set LED pin (-1 off)\n");
    printf("[bridge]   $CB:save            Save config to flash\n");
    printf("[bridge]   $CB:reset           Factory reset\n");
    printf("[bridge]   $CB:reboot          Reboot the bridge\n");
    printf("[bridge]   $CB:help            Show this help\n");
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
            printf("[bridge] Pins set to GPIO%d, GPIO%d (reboot to apply)\n", a, b);
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
            printf("[bridge] Baud set to %lu (reboot to apply)\n", rate);
        } else {
            cmd_respond("Invalid baud. Range: 300–921600");
        }
    } else if (strncmp(cmd, "led ", 4) == 0) {
        int pin = atoi(cmd + 4);
        if (pin >= -1 && pin < NUM_GPIO) {
            cfg.led_pin = (int8_t)pin;
            printf("[bridge] LED pin set to %d (reboot to apply)\n", pin);
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
        printf("[bridge] Unknown command: %s\n", cmd);
        cmd_respond("Type $CB:help for commands");
    }

    return true;
}

// ---- Main ----

int main() {
    stdio_init_all();
    config_load();
    led_init();

    while (!stdio_usb_connected())
        sleep_ms(50);

    printf("[bridge] COMrade UART Bridge @ %lu baud\n", cfg.baud);

    uint pin_rx, pin_tx;

    if (cfg.pin_a >= 0 && cfg.pin_b >= 0) {
        // Manual pin assignment — still auto-detect TX/RX orientation.
        printf("[bridge] Manual pins: GPIO%d, GPIO%d — detecting orientation...\n",
               cfg.pin_a, cfg.pin_b);

        // Set up just these two pins for detection.
        int led = active_led_pin();
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
    char cmd_buf[CMD_BUF_SIZE];
    uint cmd_len = 0;
    bool in_cmd = false;  // True once we've seen "$CB:" prefix bytes.

    while (true) {
        // PIO UART RX → USB CDC
        while (!pio_sm_is_rx_fifo_empty(pio_inst, sm_rx)) {
            char c = (char)(pio_inst->rxf[sm_rx] >> 24);
            putchar_raw(c);
        }

        // USB CDC → check for commands or forward to PIO UART TX
        int ch;
        while ((ch = getchar_timeout_us(0)) != PICO_ERROR_TIMEOUT) {
            char c = (char)ch;

            if (c == '\n' || c == '\r') {
                if (in_cmd && cmd_len > 0) {
                    cmd_buf[cmd_len] = '\0';
                    handle_command(cmd_buf);
                }
                cmd_len = 0;
                in_cmd = false;
                if (!in_cmd && c == '\n') {
                    // Not a command — but we already forwarded chars.
                    // The newline itself should go to UART too.
                }
                continue;
            }

            // Accumulate into buffer to check for prefix.
            if (cmd_len < CMD_BUF_SIZE - 1) {
                cmd_buf[cmd_len++] = c;
            }

            // Check if we're building a command.
            if (cmd_len <= CMD_PREFIX_LEN) {
                // Still accumulating — check partial prefix match.
                if (strncmp(cmd_buf, CMD_PREFIX, cmd_len) == 0) {
                    if (cmd_len == CMD_PREFIX_LEN) {
                        in_cmd = true;
                    }
                    continue;  // Don't forward yet.
                } else {
                    // Not a command — flush buffered bytes to UART.
                    for (uint i = 0; i < cmd_len; i++) {
                        uart_tx_program_putc(pio_inst, sm_tx, cmd_buf[i]);
                    }
                    cmd_len = 0;
                    in_cmd = false;
                }
            } else if (!in_cmd) {
                // Past prefix length and not a command — forward directly.
                uart_tx_program_putc(pio_inst, sm_tx, c);
            }
        }

        tight_loop_contents();
    }

    return 0;
}
