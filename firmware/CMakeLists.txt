cmake_minimum_required(VERSION 3.13)

include(pico_sdk_import.cmake)

project(comrade-uart-bridge C CXX ASM)
set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)

pico_sdk_init()

# ---- Common sources and libraries ----

set(COMMON_SOURCES src/main.c)

set(COMMON_LIBS
    pico_stdlib
    hardware_pio
    hardware_flash
    hardware_sync
    hardware_watchdog
)

# ---- Target 1: Standard RP2040 (USB CDC only) ----

add_executable(uart-bridge ${COMMON_SOURCES})

pico_generate_pio_header(uart-bridge ${CMAKE_CURRENT_LIST_DIR}/src/uart_tx.pio)
pico_generate_pio_header(uart-bridge ${CMAKE_CURRENT_LIST_DIR}/src/uart_rx.pio)

target_link_libraries(uart-bridge PRIVATE ${COMMON_LIBS})

pico_enable_stdio_usb(uart-bridge 1)
pico_enable_stdio_uart(uart-bridge 0)

pico_add_extra_outputs(uart-bridge)

# ---- Target 2: Pico W (USB CDC + BLE NUS) ----
# Only built when PICO_BOARD is pico_w (has CYW43 Bluetooth chip).

if(PICO_BOARD STREQUAL "pico_w")
    add_executable(uart-bridge-w ${COMMON_SOURCES} src/ble_nus.c)

    pico_generate_pio_header(uart-bridge-w ${CMAKE_CURRENT_LIST_DIR}/src/uart_tx.pio)
    pico_generate_pio_header(uart-bridge-w ${CMAKE_CURRENT_LIST_DIR}/src/uart_rx.pio)

    # Compile the GATT database from the .gatt file.
    pico_btstack_make_gatt_header(uart-bridge-w PRIVATE ${CMAKE_CURRENT_LIST_DIR}/src/nus.gatt)

    # btstack_config.h is in src/
    target_include_directories(uart-bridge-w PRIVATE ${CMAKE_CURRENT_LIST_DIR}/src)

    target_compile_definitions(uart-bridge-w PRIVATE BLE_ENABLED=1)

    target_link_libraries(uart-bridge-w PRIVATE
        ${COMMON_LIBS}
        pico_btstack_ble
        pico_btstack_cyw43
        pico_cyw43_arch_threadsafe_background
    )

    pico_enable_stdio_usb(uart-bridge-w 1)
    pico_enable_stdio_uart(uart-bridge-w 0)

    pico_add_extra_outputs(uart-bridge-w)
endif()
