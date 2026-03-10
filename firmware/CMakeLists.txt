cmake_minimum_required(VERSION 3.13)

include(pico_sdk_import.cmake)

project(comrade-uart-bridge C CXX ASM)
set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)

pico_sdk_init()

add_executable(uart-bridge src/main.c)

pico_generate_pio_header(uart-bridge ${CMAKE_CURRENT_LIST_DIR}/src/uart_tx.pio)
pico_generate_pio_header(uart-bridge ${CMAKE_CURRENT_LIST_DIR}/src/uart_rx.pio)

target_link_libraries(uart-bridge PRIVATE
    pico_stdlib
    hardware_pio
    hardware_flash
    hardware_sync
    hardware_watchdog
)

# USB CDC for serial, disable hardware UART stdio.
pico_enable_stdio_usb(uart-bridge 1)
pico_enable_stdio_uart(uart-bridge 0)

# Generate .uf2 for drag-and-drop flashing.
pico_add_extra_outputs(uart-bridge)
