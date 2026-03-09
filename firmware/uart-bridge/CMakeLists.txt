cmake_minimum_required(VERSION 3.13)

include(pico_sdk_import.cmake)

project(comrade-uart-bridge C CXX ASM)
set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)

pico_sdk_init()

# ---- Board presets ----
# Override with: cmake -DBRIDGE_BOARD=kb2040

if(NOT DEFINED BRIDGE_BOARD)
    set(BRIDGE_BOARD "pico")
endif()

if(BRIDGE_BOARD STREQUAL "pico")
    set(BRIDGE_PIN_A 0  CACHE STRING "First GPIO pin")
    set(BRIDGE_PIN_B 1  CACHE STRING "Second GPIO pin")
elseif(BRIDGE_BOARD STREQUAL "kb2040")
    set(BRIDGE_PIN_A 12 CACHE STRING "First GPIO pin")
    set(BRIDGE_PIN_B 13 CACHE STRING "Second GPIO pin")
else()
    message(FATAL_ERROR "Unknown board '${BRIDGE_BOARD}'. Use pico, kb2040, or set BRIDGE_PIN_A/B manually.")
endif()

# Allow direct override: cmake -DBRIDGE_PIN_A=4 -DBRIDGE_PIN_B=5
# BRIDGE_BAUD defaults to 115200 in source if not set.

message(STATUS "Board: ${BRIDGE_BOARD}  Pins: ${BRIDGE_PIN_A}, ${BRIDGE_PIN_B}")

# ---- Firmware target ----

add_executable(uart-bridge src/main.c)

pico_generate_pio_header(uart-bridge ${CMAKE_CURRENT_LIST_DIR}/src/uart_tx.pio)
pico_generate_pio_header(uart-bridge ${CMAKE_CURRENT_LIST_DIR}/src/uart_rx.pio)

target_compile_definitions(uart-bridge PRIVATE
    BRIDGE_PIN_A=${BRIDGE_PIN_A}
    BRIDGE_PIN_B=${BRIDGE_PIN_B}
    $<$<BOOL:${BRIDGE_BAUD}>:BRIDGE_BAUD=${BRIDGE_BAUD}>
)

target_link_libraries(uart-bridge PRIVATE
    pico_stdlib
    hardware_pio
)

# USB CDC for serial, disable hardware UART stdio.
pico_enable_stdio_usb(uart-bridge 1)
pico_enable_stdio_uart(uart-bridge 0)

# Generate .uf2 for drag-and-drop flashing.
pico_add_extra_outputs(uart-bridge)
