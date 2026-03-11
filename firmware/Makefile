.PHONY: build build-w clean

# Standard RP2040 build (USB CDC only).
build:
	@mkdir -p build
	cd build && cmake .. && make -j uart-bridge

# Pico W build (USB CDC + BLE NUS).
build-w:
	@mkdir -p build-w
	cd build-w && cmake -DPICO_BOARD=pico_w .. && make -j uart-bridge-w

clean:
	rm -rf build build-w
