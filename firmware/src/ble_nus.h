#ifndef BLE_NUS_H
#define BLE_NUS_H

#include <stdint.h>
#include <stdbool.h>

/// Callback invoked when the BLE client writes data to the NUS RX
/// characteristic (i.e. data coming FROM the wireless host).
typedef void (*ble_nus_rx_cb_t)(const uint8_t *data, uint16_t len);

/// Initialize BLE NUS.  Call once at startup (after cyw43_arch_init).
/// `rx_cb` is called from the BTstack context when the client sends data.
void ble_nus_init(ble_nus_rx_cb_t rx_cb);

/// Start BLE — call after ble_nus_init().
void ble_nus_start(void);

/// Queue UART data for transmission to the BLE client via NUS TX
/// notifications.  Safe to call at any time; data is buffered and sent
/// when the BLE stack is ready.  Returns the number of bytes accepted.
uint16_t ble_nus_send(const uint8_t *data, uint16_t len);

/// Returns true if a BLE client is connected and has enabled TX
/// notifications (i.e. is listening for UART data).
bool ble_nus_connected(void);

#endif
