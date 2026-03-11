/**
 * BLE NUS (Nordic UART Service) for COMrade UART Bridge.
 *
 * Provides a wireless serial interface over BLE using the standard NUS
 * profile.  Data received from the BLE client is forwarded to the UART;
 * data from the UART is sent to the BLE client as TX notifications.
 */

#include "ble_nus.h"

#include <stdio.h>
#include <string.h>

#include "btstack.h"
#include "nus.h"  // Generated from nus.gatt by pico_btstack_make_gatt_header.

// ---- TX ring buffer ----
// UART data is produced faster than BLE can send, so we buffer it.

#define TX_BUF_SIZE 512

static uint8_t  tx_buf[TX_BUF_SIZE];
static uint16_t tx_head;  // Write position.
static uint16_t tx_tail;  // Read position.

static uint16_t tx_buf_used(void) {
    return (uint16_t)((tx_head - tx_tail) % TX_BUF_SIZE);
}

// ---- State ----

static hci_con_handle_t con_handle = HCI_CON_HANDLE_INVALID;
static bool             tx_notify_enabled;
static ble_nus_rx_cb_t  rx_callback;

static btstack_context_callback_registration_t tx_request;

// ---- Advertising data ----

// Flags: General Discoverable + BR/EDR Not Supported.
static const uint8_t adv_data[] = {
    0x02, 0x01, 0x06,                    // Flags
    0x11, 0x07,                           // Complete list of 128-bit service UUIDs
    // NUS Service UUID (little-endian)
    0x9e, 0xca, 0xdc, 0x24, 0x0e, 0xe5,
    0xa9, 0xe0, 0x93, 0xf3, 0xa3, 0xb5,
    0x01, 0x00, 0x40, 0x6e,
    0x0f, 0x09,                           // Complete Local Name
    'C','O','M','r','a','d','e',' ',
    'B','r','i','d','g','e',
};

// ---- TX notification callback ----

static void tx_send_callback(void *context) {
    (void)context;

    if (con_handle == HCI_CON_HANDLE_INVALID || !tx_notify_enabled)
        return;

    uint16_t avail = tx_buf_used();
    if (avail == 0) return;

    // BLE ATT MTU limits payload; use a safe default.
    uint16_t mtu = att_server_get_mtu(con_handle);
    uint16_t max_payload = (mtu > 3) ? (mtu - 3) : 20;
    if (max_payload > avail) max_payload = avail;

    // Build a contiguous chunk from the ring buffer.
    uint8_t chunk[244];  // Max ATT payload.
    if (max_payload > sizeof(chunk)) max_payload = sizeof(chunk);

    for (uint16_t i = 0; i < max_payload; i++) {
        chunk[i] = tx_buf[(tx_tail + i) % TX_BUF_SIZE];
    }

    att_server_notify(con_handle,
                      ATT_CHARACTERISTIC_6E400003_B5A3_F393_E0A9_E50E24DCCA9E_01_VALUE_HANDLE,
                      chunk, max_payload);

    tx_tail = (tx_tail + max_payload) % TX_BUF_SIZE;

    // If there's more data, request another send slot.
    if (tx_buf_used() > 0) {
        att_server_request_to_send_notification(&tx_request, con_handle);
    }
}

// ---- ATT read/write callbacks ----

static uint16_t att_read_callback(hci_con_handle_t connection_handle,
                                  uint16_t att_handle, uint16_t offset,
                                  uint8_t *buffer, uint16_t buffer_size) {
    (void)connection_handle;
    (void)att_handle;
    (void)offset;
    (void)buffer;
    (void)buffer_size;
    return 0;
}

static int att_write_callback(hci_con_handle_t connection_handle,
                              uint16_t att_handle, uint16_t transaction_mode,
                              uint16_t offset, uint8_t *buffer,
                              uint16_t buffer_size) {
    (void)transaction_mode;
    (void)offset;

    // Client wrote to RX characteristic — forward to UART.
    if (att_handle == ATT_CHARACTERISTIC_6E400002_B5A3_F393_E0A9_E50E24DCCA9E_01_VALUE_HANDLE) {
        if (rx_callback && buffer_size > 0) {
            rx_callback(buffer, buffer_size);
        }
        return 0;
    }

    // Client toggled TX notifications (CCCD write).
    if (att_handle == ATT_CHARACTERISTIC_6E400003_B5A3_F393_E0A9_E50E24DCCA9E_01_CLIENT_CONFIGURATION_HANDLE) {
        tx_notify_enabled = little_endian_read_16(buffer, 0) == GATT_CLIENT_CHARACTERISTICS_CONFIGURATION_NOTIFICATION;
        con_handle = connection_handle;
        return 0;
    }

    return 0;
}

// ---- HCI event handler ----

static btstack_packet_callback_registration_t hci_event_cb;

static void hci_event_handler(uint8_t packet_type, uint16_t channel,
                              uint8_t *packet, uint16_t size) {
    (void)channel;
    (void)size;

    if (packet_type != HCI_EVENT_PACKET) return;

    uint8_t event = hci_event_packet_get_type(packet);

    switch (event) {
        case BTSTACK_EVENT_STATE:
            if (btstack_event_state_get_state(packet) == HCI_STATE_WORKING) {
                printf("[ble] BTstack running, advertising...\n");
            }
            break;

        case HCI_EVENT_DISCONNECTION_COMPLETE:
            con_handle = HCI_CON_HANDLE_INVALID;
            tx_notify_enabled = false;
            // Flush TX buffer on disconnect.
            tx_head = 0;
            tx_tail = 0;
            printf("[ble] Disconnected, re-advertising\n");
            gap_advertisements_enable(1);
            break;

        case HCI_EVENT_LE_META:
            if (hci_event_le_meta_get_subevent_code(packet) == HCI_SUBEVENT_LE_CONNECTION_COMPLETE) {
                con_handle = hci_subevent_le_connection_complete_get_connection_handle(packet);
                printf("[ble] Connected\n");
            }
            break;

        default:
            break;
    }
}

// ---- Public API ----

void ble_nus_init(ble_nus_rx_cb_t rx_cb) {
    rx_callback = rx_cb;
    tx_head = 0;
    tx_tail = 0;
    tx_notify_enabled = false;
    con_handle = HCI_CON_HANDLE_INVALID;

    // Initialize L2CAP.
    l2cap_init();

    // Initialize Security Manager (required even if no pairing).
    sm_init();

    // Initialize ATT server with the compiled GATT database.
    att_server_init(profile_data, att_read_callback, att_write_callback);

    // Register HCI event handler.
    hci_event_cb.callback = hci_event_handler;
    hci_add_event_handler(&hci_event_cb);

    // Register ATT server packet handler (for CAN_SEND_NOW events).
    att_server_register_packet_handler(hci_event_handler);

    // Set advertising data.
    gap_advertisements_set_data(sizeof(adv_data), (uint8_t *)adv_data);

    // Advertising parameters: 100ms interval, connectable undirected.
    gap_advertisements_set_params(0x00A0, 0x00A0, 0, 0, NULL, 0x07, 0x00);

    // TX notification callback.
    tx_request.callback = tx_send_callback;
}

void ble_nus_start(void) {
    // Enable advertising.
    gap_advertisements_enable(1);

    // Power on the Bluetooth controller.
    hci_power_control(HCI_POWER_ON);
}

uint16_t ble_nus_send(const uint8_t *data, uint16_t len) {
    if (!tx_notify_enabled || con_handle == HCI_CON_HANDLE_INVALID)
        return 0;

    uint16_t free = TX_BUF_SIZE - 1 - tx_buf_used();
    if (len > free) len = free;

    for (uint16_t i = 0; i < len; i++) {
        tx_buf[tx_head] = data[i];
        tx_head = (tx_head + 1) % TX_BUF_SIZE;
    }

    // Kick the notification send chain.
    att_server_request_to_send_notification(&tx_request, con_handle);

    return len;
}

bool ble_nus_connected(void) {
    return con_handle != HCI_CON_HANDLE_INVALID && tx_notify_enabled;
}
